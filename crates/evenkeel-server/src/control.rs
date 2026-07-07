//! The control loop — one serialized tick (ADR-2): poll → snapshot → persist
//! → classify → publish (dashboard state + metrics). Phase 1 ends the tick
//! there; plan/execute stages join in Phase 2 behind the same serialization.

use std::collections::BTreeMap;
use std::sync::Arc;

use evenkeel_core::{classify, ChannelHealth, ChannelSnapshot};
use evenkeel_node::{FiberRpc, ListChannelsParams};
use evenkeel_store::{ActionRecord, Store};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::executor::{Approvals, Executor, PolicyState, SharedPolicy};
use crate::metrics::Metrics;
use crate::state::{
    ActionView, ChannelView, Dashboard, HistoryPoint, LedgerView, NodeStatus, SharedDashboard,
};

/// Milliseconds since the UNIX epoch, from the wall clock. The core never
/// reads a clock; the loop stamps time at the boundary.
fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Run the poll loop forever. Errors degrade (stale data + `rpc_up 0`),
/// they never kill the loop (§7). One tick = poll → classify → publish →
/// executor step (plan/price/confirm), strictly serialized (ADR-2).
pub async fn run(
    config: Config,
    node: Arc<dyn FiberRpc>,
    store: Store,
    dashboard: SharedDashboard,
    metrics: Arc<Metrics>,
    approvals: Approvals,
    policy: SharedPolicy,
) {
    let mut node_pubkey = String::new();
    let mut node_version = String::new();
    let mut executor: Option<Executor> = None;
    let mut ticker = tokio::time::interval(config.poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        // Identity is fetched lazily and remembered; it also doubles as the
        // liveness probe on ticks where we don't yet know the node.
        if node_pubkey.is_empty() {
            match node.node_info().await {
                Ok(info) => {
                    info!(pubkey = %info.pubkey, version = %info.version, "connected to node");
                    node_pubkey = info.pubkey;
                    node_version = info.version;
                }
                Err(e) => {
                    warn!(error = %e, "node_info failed; will retry next tick");
                    publish_outage(&dashboard, &metrics, &store, &node_pubkey, &node_version, &config).await;
                    continue;
                }
            }
        }

        // The executor exists from the moment identity is known; startup
        // reconciliation (§7) runs exactly once, before any planning. The
        // operator's persisted policy (keyed by the pubkey we just learned)
        // replaces the boot defaults first — saved bounds and the autopilot
        // flag survive restarts. No row → run defaults, write nothing.
        if executor.is_none() {
            match store.load_policy(&node_pubkey).await {
                Ok(Some((p, autopilot))) => {
                    info!(autopilot, "loaded persisted policy");
                    let mut ps = policy.lock().unwrap_or_else(|e| e.into_inner());
                    *ps = PolicyState { policy: p, autopilot };
                }
                Ok(None) => info!("no persisted policy; running defaults (advisory)"),
                Err(e) => {
                    error!(error = %e, "cannot load persisted policy; running defaults (advisory)");
                }
            }
            let mut ex = Executor::new(
                node.clone(),
                store.clone(),
                policy.clone(),
                node_pubkey.clone(),
                approvals.clone(),
                metrics.clone(),
            );
            ex.reconcile_on_startup(now_ms()).await;
            executor = Some(ex);
        }

        let rpc_result = node.list_channels(ListChannelsParams::default()).await;
        let channels = match rpc_result {
            Ok(chs) => chs,
            Err(e) => {
                warn!(error = %e, "list_channels failed; serving stale data");
                publish_outage(&dashboard, &metrics, &store, &node_pubkey, &node_version, &config).await;
                continue;
            }
        };

        let at_ms = now_ms();
        let snapshots: Vec<ChannelSnapshot> =
            channels.iter().map(|c| c.to_snapshot(at_ms)).collect();

        if let Err(e) = store.insert_snapshots(&node_pubkey, &snapshots).await {
            // The node answered — RPC is up — but history is broken; classify
            // from what the DB has plus keep serving. Loud, not fatal.
            error!(error = %e, "failed to persist snapshots");
        }

        let window_start = at_ms.saturating_sub(config.drift_window.as_millis() as u64);
        let window = match store.window_since(&node_pubkey, window_start).await {
            Ok(w) => w,
            Err(e) => {
                error!(error = %e, "failed to read drift window; classifying from this tick only");
                snapshots.clone()
            }
        };

        // Group the flat window into per-channel ascending series.
        let mut by_channel: BTreeMap<String, Vec<ChannelSnapshot>> = BTreeMap::new();
        for s in window {
            by_channel.entry(s.channel_id.clone()).or_default().push(s);
        }

        // One coherent policy read per tick — the API may change thresholds
        // or budgets between ticks and they apply live, without a restart.
        let live = policy.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let mut views = Vec::new();
        let mut healths: Vec<ChannelHealth> = Vec::new();
        metrics.channels_by_state.reset();
        for (channel_id, series) in &by_channel {
            let Some(newest) = series.last() else { continue };
            // Channels that have left the node's list stay in history but out
            // of the dashboard: only channels present this tick are shown.
            let Some(current) = snapshots.iter().find(|s| &s.channel_id == channel_id) else {
                continue;
            };
            let Some(health) = classify(series, &live.policy.thresholds) else { continue };

            let asset = match &current.asset {
                evenkeel_core::Asset::Ckb => "ckb".to_string(),
                evenkeel_core::Asset::Udt(s) => format!("udt:{s}"),
            };

            if let Some(bp) = health.usable_ratio_bp {
                metrics
                    .usable_ratio
                    .with_label_values(&[channel_id, &asset])
                    .set(bp as f64 / 10_000.0);
            }
            if let Some(slope) = health.drift_bp_per_hour {
                metrics.drift_slope.with_label_values(&[channel_id]).set(slope as f64);
            }
            metrics
                .channels_by_state
                .with_label_values(&[class_label(health.class)])
                .inc();

            views.push(ChannelView {
                channel_id: channel_id.clone(),
                peer: newest.peer.clone(),
                asset,
                local_balance: current.local_balance.to_string(),
                remote_balance: current.remote_balance.to_string(),
                usable_out: current.usable_out().to_string(),
                usable_in: current.usable_in().to_string(),
                history: series
                    .iter()
                    .map(|s| HistoryPoint { at_ms: s.at_ms, usable_ratio_bp: s.usable_ratio_bp() })
                    .collect(),
                health: health.clone(),
            });
            healths.push(health);
        }

        metrics.rpc_up.set(1);
        metrics.snapshot_age_seconds.set(0);

        // The money half of the tick: at most one action progresses (ADR-2).
        let (actions, ledger) = if let Some(ex) = executor.as_mut() {
            ex.tick(&snapshots, &healths, at_ms).await;
            let spent = ex.spent_today(at_ms).await;
            let daily_budget = live.policy.max_fee_per_day;
            metrics
                .budget_remaining
                .set(daily_budget.saturating_sub(spent).min(i64::MAX as u128) as i64);
            let actions = store
                .recent_actions(&node_pubkey, 25)
                .await
                .map(|list| list.into_iter().map(to_action_view).collect())
                .unwrap_or_default();
            let ledger = LedgerView {
                spent_today: spent.to_string(),
                daily_budget: daily_budget.to_string(),
            };
            (actions, ledger)
        } else {
            (Vec::new(), LedgerView::default())
        };

        let mut d = dashboard.write().await;
        *d = Dashboard {
            status: NodeStatus {
                node_pubkey: node_pubkey.clone(),
                node_version: node_version.clone(),
                rpc_up: true,
                last_snapshot_ms: Some(at_ms),
                stale: false,
            },
            channels: views,
            actions,
            ledger,
        };
    }
}

/// Project a store row into the dashboard shape (money as decimal strings).
fn to_action_view(a: ActionRecord) -> ActionView {
    ActionView {
        intent_id: a.intent_id,
        asset: match &a.asset {
            evenkeel_core::Asset::Ckb => "ckb".to_string(),
            evenkeel_core::Asset::Udt(s) => format!("udt:{s}"),
        },
        source_channel: a.source_channel,
        sink_channel: a.sink_channel,
        amount: a.amount.to_string(),
        benefit_bp: a.benefit_bp,
        state: a.state.as_str().to_string(),
        mode: a.mode,
        quoted_fee: a.quoted_fee.map(|f| f.to_string()),
        actual_fee: a.actual_fee.map(|f| f.to_string()),
        payment_hash: a.payment_hash,
        reason: a.reason,
        created_at_ms: a.created_at_ms,
        updated_at_ms: a.updated_at_ms,
    }
}

/// Publish the outage picture: keep the last channel views, mark RPC down,
/// recompute staleness from the store's newest snapshot.
async fn publish_outage(
    dashboard: &SharedDashboard,
    metrics: &Metrics,
    store: &Store,
    node_pubkey: &str,
    node_version: &str,
    config: &Config,
) {
    metrics.rpc_up.set(0);
    let last = if node_pubkey.is_empty() {
        None
    } else {
        store.latest_at_ms(node_pubkey).await.unwrap_or(None)
    };
    let age_ms = last.map(|t| now_ms().saturating_sub(t));
    if let Some(age) = age_ms {
        metrics.snapshot_age_seconds.set((age / 1_000) as i64);
    }

    let mut d = dashboard.write().await;
    d.status = NodeStatus {
        node_pubkey: node_pubkey.to_string(),
        node_version: node_version.to_string(),
        rpc_up: false,
        last_snapshot_ms: last,
        stale: age_ms.map(|a| a > config.stale_after.as_millis() as u64).unwrap_or(true),
    };
}

fn class_label(class: evenkeel_core::HealthClass) -> &'static str {
    match class {
        evenkeel_core::HealthClass::Depleted => "depleted",
        evenkeel_core::HealthClass::Depleting => "depleting",
        evenkeel_core::HealthClass::Healthy => "healthy",
        evenkeel_core::HealthClass::Filling => "filling",
        evenkeel_core::HealthClass::Saturated => "saturated",
    }
}
