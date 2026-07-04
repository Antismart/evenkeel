//! The control loop — one serialized tick (ADR-2): poll → snapshot → persist
//! → classify → publish (dashboard state + metrics). Phase 1 ends the tick
//! there; plan/execute stages join in Phase 2 behind the same serialization.

use std::collections::BTreeMap;
use std::sync::Arc;

use evenkeel_core::{classify, ChannelSnapshot, HealthThresholds};
use evenkeel_node::{FiberRpc, ListChannelsParams};
use evenkeel_store::Store;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::metrics::Metrics;
use crate::state::{ChannelView, Dashboard, HistoryPoint, NodeStatus, SharedDashboard};

/// Milliseconds since the UNIX epoch, from the wall clock. The core never
/// reads a clock; the loop stamps time at the boundary.
fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Run the poll loop forever. Errors degrade (stale data + `rpc_up 0`),
/// they never kill the loop (§7).
pub async fn run(
    config: Config,
    node: Arc<dyn FiberRpc>,
    store: Store,
    dashboard: SharedDashboard,
    metrics: Arc<Metrics>,
) {
    let thresholds = HealthThresholds::default();
    let mut node_pubkey = String::new();
    let mut node_version = String::new();
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

        let mut views = Vec::new();
        metrics.channels_by_state.reset();
        for (channel_id, series) in &by_channel {
            let Some(newest) = series.last() else { continue };
            // Channels that have left the node's list stay in history but out
            // of the dashboard: only channels present this tick are shown.
            let Some(current) = snapshots.iter().find(|s| &s.channel_id == channel_id) else {
                continue;
            };
            let Some(health) = classify(series, &thresholds) else { continue };

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
                health,
            });
        }

        metrics.rpc_up.set(1);
        metrics.snapshot_age_seconds.set(0);

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
        };
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
