//! The 24h deterministic simulation harness (architecture §8.3).
//!
//! Replays a synthetic day of traffic against the [`MockNode`] and — in the
//! managed run — the **real** code paths: the same snapshot → window →
//! classify → executor sequence `control.rs` performs, with the real
//! [`Executor`] writing through the real [`Store`]. The unmanaged baseline
//! runs the identical polling with no executor. The delta between the two
//! trajectories is Even Keel's measurable effect on a day of traffic.
//!
//! Everything is deterministic: a fixed tick cadence (one tick = 5 simulated
//! minutes, 288 ticks = 24h), a simulated clock starting from a constant
//! epoch, scripted balances, and the MockNode's fixed fee schedule. No wall
//! clock, no randomness. Running the same scenario twice produces the same
//! [`SimReport`], byte for byte — which is what makes this the demo
//! centerpiece *and* the substrate for the §8 property test ("for any policy,
//! total fees spent ≤ daily cap AND net imbalance does not increase").
//!
//! Money stays `u128` Shannons and ratios integer basis points throughout
//! (ADR-7); report fields serialize Shannons as decimal strings.

use std::collections::BTreeMap;
use std::sync::Arc;

use evenkeel_core::{
    classify, BasisPoints, ChannelHealth, ChannelSnapshot, Policy, Shannons, BP_SCALE,
    SHANNONS_PER_CKB,
};
use evenkeel_node::{BalanceScript, FiberRpc, MockBalances, MockChannelSpec, MockNode};
use evenkeel_store::{ActionState, Store};
use serde::Serialize;
use tracing::info;

use crate::executor::{Approvals, Executor};
use crate::metrics::Metrics;

/// One simulated tick = 5 minutes, matching a realistic poll cadence.
pub const TICK_MS: u64 = 300_000;

/// 288 five-minute ticks = exactly 24 simulated hours.
pub const TICKS_PER_DAY: u32 = 288;

/// The simulated clock's constant origin: `86_400_000 × 20_000` ms — exactly
/// midnight UTC, so the executor's daily fee-ledger window aligns with the
/// simulated day. Never the wall clock.
pub const SIM_EPOCH_MS: u64 = 1_728_000_000_000;

/// Drift window fed to classification, mirroring the control loop's default
/// (`EVENKEEL_DRIFT_WINDOW_SECS` = 3600).
const DRIFT_WINDOW_MS: u64 = 3_600_000;

/// Errors from a simulation run. The harness drives real adapters, so it
/// inherits their failure modes; nothing here is swallowed.
#[derive(Debug, thiserror::Error)]
pub enum SimError {
    /// Store failure (snapshots, actions, or the fee ledger).
    #[error("store error: {0}")]
    Store(#[from] evenkeel_store::StoreError),
    /// MockNode RPC failure (impossible without fault injection, but the
    /// trait is honest about it).
    #[error("node error: {0}")]
    Node(#[from] evenkeel_node::NodeError),
    /// Metrics registry construction failure.
    #[error("metrics error: {0}")]
    Metrics(#[from] prometheus::Error),
}

/// A named, scripted set of MockNode channels whose balances evolve over the
/// simulated day (§8: steady drain, burst traffic, oscillating).
#[derive(Debug, Clone)]
pub struct TrafficScenario {
    /// Stable machine name (`steady_drain`, `burst`, `oscillating`).
    pub name: &'static str,
    /// One-line human description for reports.
    pub description: &'static str,
    /// The scripted channels, in display order.
    pub channels: Vec<MockChannelSpec>,
}

impl TrafficScenario {
    /// Channel IDs in scenario order (the report's display order).
    pub fn channel_ids(&self) -> Vec<String> {
        self.channels.iter().map(|c| c.channel_id.clone()).collect()
    }
}

/// Constant rebalance-source channel shared by the scenarios: parked
/// saturated, the surplus every correction draws from. Capacity matters —
/// the planner only draws from SATURATED/FILLING channels, so a source stops
/// donating once it falls out of the saturated band; a big hub channel
/// sustains a whole day of corrections, a small one exhausts early.
fn source_channel(local_ckb: u128, capacity_ckb: u128) -> MockChannelSpec {
    MockChannelSpec {
        channel_id: "0xsim_source".into(),
        peer: "02src".into(),
        script: BalanceScript::Constant(MockBalances::simple(
            local_ckb * SHANNONS_PER_CKB,
            (capacity_ckb - local_ckb) * SHANNONS_PER_CKB,
        )),
        ready: true,
    }
}

/// Constant healthy channel: a control that no scenario should disturb.
fn steady_channel() -> MockChannelSpec {
    MockChannelSpec {
        channel_id: "0xsim_steady".into(),
        peer: "02std".into(),
        script: BalanceScript::Constant(MockBalances::simple(
            500 * SHANNONS_PER_CKB,
            500 * SHANNONS_PER_CKB,
        )),
        ready: true,
    }
}

/// §8 pattern 1: one channel bleeding outbound all day (5 CKB per tick on a
/// 1000 CKB channel — 600 bp/h, fast enough that drift detection fires before
/// the depleted threshold does). Unmanaged, it flatlines at zero.
pub fn steady_drain() -> TrafficScenario {
    TrafficScenario {
        name: "steady_drain",
        description: "one channel bleeds outbound all day; unmanaged it flatlines at zero",
        channels: vec![
            MockChannelSpec {
                channel_id: "0xsim_drain".into(),
                peer: "02drn".into(),
                script: BalanceScript::Drain {
                    start: MockBalances::simple(800 * SHANNONS_PER_CKB, 200 * SHANNONS_PER_CKB),
                    per_tick: 5 * SHANNONS_PER_CKB,
                },
                ready: true,
            },
            source_channel(9_500, 10_000),
            steady_channel(),
        ],
    }
}

/// §8 pattern 2: calm → a heavy 4h drain window (ticks 96–144, 15 CKB/tick)
/// → calm. The burst outruns the channel's balance; the interesting question
/// is how fast management catches it.
pub fn burst() -> TrafficScenario {
    let mut steps = Vec::with_capacity(145);
    for tick in 0u128..145 {
        let local_ckb: u128 = if tick < 96 {
            600
        } else {
            600u128.saturating_sub(15 * (tick - 96))
        };
        steps.push(MockBalances::simple(
            local_ckb * SHANNONS_PER_CKB,
            (1_000 - local_ckb) * SHANNONS_PER_CKB,
        ));
    }
    TrafficScenario {
        name: "burst",
        description: "calm, then a heavy 4h drain window that outruns the channel, then calm",
        channels: vec![
            MockChannelSpec {
                channel_id: "0xsim_burst".into(),
                peer: "02bst".into(),
                script: BalanceScript::Steps(steps),
                ready: true,
            },
            source_channel(9_500, 10_000),
            steady_channel(),
        ],
    }
}

/// §8 pattern 3: a channel swinging 75% → 27% → 75% on an 8h period (three
/// full cycles a day, 10 CKB per tick). Hysteresis is on trial here: the
/// manager must damp the swings without oscillating fees away.
pub fn oscillating() -> TrafficScenario {
    let mut steps = Vec::with_capacity(TICKS_PER_DAY as usize);
    for tick in 0u128..TICKS_PER_DAY as u128 {
        let phase = tick % 96;
        let local_ckb = if phase < 48 { 750 - 10 * phase } else { 270 + 10 * (phase - 48) };
        steps.push(MockBalances::simple(
            local_ckb * SHANNONS_PER_CKB,
            (1_000 - local_ckb) * SHANNONS_PER_CKB,
        ));
    }
    TrafficScenario {
        name: "oscillating",
        description: "traffic swings a channel between 27% and 75% three times over the day",
        channels: vec![
            MockChannelSpec {
                channel_id: "0xsim_osc".into(),
                peer: "02osc".into(),
                script: BalanceScript::Steps(steps),
                ready: true,
            },
            // Deliberately small: the interesting question for a swinging
            // channel is whether hysteresis and a finite surplus stop the
            // manager from chasing the wave and churning fees.
            source_channel(880, 1_000),
            steady_channel(),
        ],
    }
}

/// All shipped scenarios, in report order.
pub fn all_scenarios() -> Vec<TrafficScenario> {
    vec![steady_drain(), burst(), oscillating()]
}

/// Serialize `u128` money/metric values as decimal strings (the repo-wide
/// convention for JSON boundaries — no 64-bit truncation, ADR-7).
mod u128_string {
    use serde::Serializer;

    /// Serialize the value as its decimal-string form.
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
}

/// The outcome of one simulated day for one run (managed or baseline).
#[derive(Debug, Clone, Serialize)]
pub struct SimRun {
    /// Whether the real executor ran (true) or this is the baseline (false).
    pub managed: bool,
    /// The unique node identity this run wrote DB rows under. Excluded from
    /// serialization so reports stay byte-identical across invocations.
    #[serde(skip)]
    pub node_id: String,
    /// Per-channel, per-tick `usable_ratio_bp` trajectory (`None` for a
    /// zero-capacity tick). Key order (BTreeMap) keeps JSON deterministic.
    pub trajectories: BTreeMap<String, Vec<Option<BasisPoints>>>,
    /// Total settled fees over the day, from the action ledger's
    /// `actual_fee` — never the dry-run quote.
    #[serde(with = "u128_string")]
    pub total_fee: Shannons,
    /// Actions that reached SETTLED.
    pub actions_settled: u32,
    /// Actions that reached FAILED.
    pub actions_failed: u32,
    /// Actions that reached REJECTED (policy, price expiry, or dry-run no).
    pub actions_rejected: u32,
    /// Net imbalance at the first tick: Σ per channel of
    /// `|usable_out·10⁴ − target_bp·capacity|` — capacity-weighted deviation
    /// in exact bp·Shannon units (no ratio rounding).
    #[serde(with = "u128_string")]
    pub imbalance_start: u128,
    /// Net imbalance at the last tick, same units as `imbalance_start`.
    #[serde(with = "u128_string")]
    pub imbalance_end: u128,
    /// `imbalance_start` normalized by total capacity: mean deviation in bp.
    pub imbalance_start_mean_bp: BasisPoints,
    /// `imbalance_end` normalized by total capacity: mean deviation in bp.
    pub imbalance_end_mean_bp: BasisPoints,
}

/// Managed-vs-baseline comparison for one scenario: the §8.3 with/without
/// picture the demo charts are drawn from.
#[derive(Debug, Clone, Serialize)]
pub struct SimReport {
    /// Scenario machine name.
    pub scenario: String,
    /// Scenario description.
    pub description: String,
    /// Ticks simulated (288 = a full day).
    pub ticks: u32,
    /// Milliseconds of simulated time per tick.
    pub tick_ms: u64,
    /// The policy's target ratio, for chart guides.
    pub target_ratio_bp: BasisPoints,
    /// The policy's depleted threshold, for chart guides.
    pub depleted_below_bp: BasisPoints,
    /// The policy's saturated threshold, for chart guides.
    pub saturated_above_bp: BasisPoints,
    /// The policy's daily fee cap the managed run was bounded by.
    #[serde(with = "u128_string")]
    pub daily_fee_cap: Shannons,
    /// Channel IDs in scenario display order.
    pub channels: Vec<String>,
    /// The run with the real executor.
    pub managed: SimRun,
    /// The baseline run (identical polling, no executor).
    pub baseline: SimRun,
}

/// Exact capacity-weighted net-imbalance metric: Σ over ready channels of
/// `|usable_out·10⁴ − target_bp·capacity|`, in bp·Shannon units. Computed
/// without ratio rounding so run comparisons are exact integer facts.
pub fn imbalance_weighted(snapshots: &[ChannelSnapshot], target_bp: BasisPoints) -> u128 {
    snapshots
        .iter()
        .filter(|s| s.ready)
        .map(|s| {
            s.usable_out()
                .saturating_mul(BP_SCALE)
                .abs_diff((target_bp as u128).saturating_mul(s.capacity()))
        })
        .sum()
}

/// Total ready capacity, the normalizer for the mean-bp display form.
fn total_capacity(snapshots: &[ChannelSnapshot]) -> Shannons {
    snapshots.iter().filter(|s| s.ready).map(|s| s.capacity()).sum()
}

fn mean_bp(weighted: u128, capacity: Shannons) -> BasisPoints {
    if capacity == 0 {
        return 0;
    }
    // weighted / capacity ≤ BP_SCALE by construction, so this fits u16.
    (weighted / capacity).min(BP_SCALE) as BasisPoints
}

/// Run one full simulated day (288 ticks) for one side of the comparison.
/// See [`run_simulated_ticks`] for the mechanics and determinism contract.
pub async fn run_simulated_day(
    policy: &Policy,
    scenario: TrafficScenario,
    managed: bool,
    store: &Store,
    run_id: &str,
) -> Result<SimRun, SimError> {
    run_simulated_ticks(policy, &scenario, managed, store, run_id, TICKS_PER_DAY).await
}

/// The simulation engine: `ticks` iterations of exactly the control loop's
/// tick sequence — poll → snapshot → persist → drift window → classify →
/// (managed only) executor step — against a fresh [`MockNode`] scripted by
/// `scenario`, on the simulated clock.
///
/// In the managed run, every PRICED action is auto-approved through the
/// executor's public [`Approvals`] map — the simulated operator/autopilot.
/// The executor itself is the real Phase 2 state machine, unmodified.
///
/// `run_id` must be unique per invocation (it namespaces the node identity,
/// so DB rows from parallel runs never collide). Nothing derived from it
/// appears in the serialized report, keeping report bytes deterministic.
pub async fn run_simulated_ticks(
    policy: &Policy,
    scenario: &TrafficScenario,
    managed: bool,
    store: &Store,
    run_id: &str,
    ticks: u32,
) -> Result<SimRun, SimError> {
    // The unique run_id is the SUFFIX: the executor derives its intent-id
    // namespace from the node_id's last 8 characters, so the unique part
    // must sit at the end or intent ids would collide across invocations
    // (the simulated clock restarts from the same epoch every run).
    let node_id = format!("03sim{}{}", if managed { "m" } else { "b" }, run_id);
    let node = Arc::new(
        MockNode::new(scenario.channels.clone())
            .with_base_ms(SIM_EPOCH_MS)
            .with_pubkey(node_id.clone()),
    );

    let approvals = Approvals::default();
    let mut executor = if managed {
        let mut ex = Executor::new(
            node.clone(),
            store.clone(),
            policy.clone(),
            node_id.clone(),
            approvals.clone(),
            Arc::new(Metrics::new()?),
        );
        // Mirrors the control loop's startup order (§7): reconcile before
        // the first planning tick. A fresh identity has nothing to reconcile.
        ex.reconcile_on_startup(SIM_EPOCH_MS).await;
        Some(ex)
    } else {
        None
    };

    let channel_ids = scenario.channel_ids();
    let mut trajectories: BTreeMap<String, Vec<Option<BasisPoints>>> = channel_ids
        .iter()
        .map(|id| (id.clone(), Vec::with_capacity(ticks as usize)))
        .collect();
    let mut imbalance_start = 0u128;
    let mut imbalance_end = 0u128;
    let mut capacity_start = 0u128;
    let mut capacity_end = 0u128;

    for tick in 0..ticks {
        let at_ms = SIM_EPOCH_MS + u64::from(tick) * TICK_MS;

        // Poll → snapshot → persist, exactly like control::run.
        let channels = node.list_channels(Default::default()).await?;
        let snapshots: Vec<ChannelSnapshot> =
            channels.iter().map(|c| c.to_snapshot(at_ms)).collect();
        store.insert_snapshots(&node_id, &snapshots).await?;

        // Drift window → per-channel series → classify.
        let window = store
            .window_since(&node_id, at_ms.saturating_sub(DRIFT_WINDOW_MS))
            .await?;
        let mut by_channel: BTreeMap<String, Vec<ChannelSnapshot>> = BTreeMap::new();
        for s in window {
            by_channel.entry(s.channel_id.clone()).or_default().push(s);
        }
        let healths: Vec<ChannelHealth> = by_channel
            .iter()
            .filter(|(id, _)| snapshots.iter().any(|s| &s.channel_id == *id))
            .filter_map(|(_, series)| classify(series, &policy.thresholds))
            .collect();

        // Record the trajectory point for every scenario channel.
        for id in &channel_ids {
            let point = snapshots
                .iter()
                .find(|s| &s.channel_id == id)
                .and_then(|s| s.usable_ratio_bp());
            if let Some(t) = trajectories.get_mut(id) {
                t.push(point);
            }
        }

        if tick == 0 {
            imbalance_start = imbalance_weighted(&snapshots, policy.target_ratio_bp);
            capacity_start = total_capacity(&snapshots);
        }
        if tick + 1 == ticks {
            imbalance_end = imbalance_weighted(&snapshots, policy.target_ratio_bp);
            capacity_end = total_capacity(&snapshots);
        }

        // The money half of the tick (managed run only): one serialized
        // executor step, then auto-approve whatever it priced so the next
        // tick executes it — the simulated operator.
        if let Some(ex) = executor.as_mut() {
            ex.tick(&snapshots, &healths, at_ms).await;
            for action in store.non_terminal_actions(&node_id).await? {
                if action.state == ActionState::Priced {
                    let mut map = approvals.lock().unwrap_or_else(|e| e.into_inner());
                    map.insert(action.intent_id.clone(), true);
                }
            }
        }
    }

    // Day-end accounting from the ledger and the action log.
    let total_fee = store
        .fee_spent_between(&node_id, SIM_EPOCH_MS, SIM_EPOCH_MS + 86_400_000)
        .await?;
    let actions = store.recent_actions(&node_id, 10_000).await?;
    let count = |state: ActionState| actions.iter().filter(|a| a.state == state).count() as u32;

    info!(
        run_id,
        managed,
        scenario = scenario.name,
        total_fee,
        settled = count(ActionState::Settled),
        "simulated day complete"
    );

    Ok(SimRun {
        managed,
        node_id,
        trajectories,
        total_fee,
        actions_settled: count(ActionState::Settled),
        actions_failed: count(ActionState::Failed),
        actions_rejected: count(ActionState::Rejected),
        imbalance_start,
        imbalance_end,
        imbalance_start_mean_bp: mean_bp(imbalance_start, capacity_start),
        imbalance_end_mean_bp: mean_bp(imbalance_end, capacity_end),
    })
}

/// Run the with/without pair for one scenario and package the comparison.
/// Both runs see identical scripted traffic; only the executor differs.
pub async fn compare_scenario(
    policy: &Policy,
    scenario: &TrafficScenario,
    store: &Store,
    run_id: &str,
    ticks: u32,
) -> Result<SimReport, SimError> {
    let baseline = run_simulated_ticks(policy, scenario, false, store, run_id, ticks).await?;
    let managed = run_simulated_ticks(policy, scenario, true, store, run_id, ticks).await?;
    Ok(SimReport {
        scenario: scenario.name.to_string(),
        description: scenario.description.to_string(),
        ticks,
        tick_ms: TICK_MS,
        target_ratio_bp: policy.target_ratio_bp,
        depleted_below_bp: policy.thresholds.depleted_below_bp,
        saturated_above_bp: policy.thresholds.saturated_above_bp,
        daily_fee_cap: policy.max_fee_per_day,
        channels: scenario.channel_ids(),
        managed,
        baseline,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use evenkeel_node::ListChannelsParams;

    /// Local balances of one channel across a whole scripted day.
    async fn day_of_locals(scenario: &TrafficScenario, channel_id: &str) -> Vec<u128> {
        let node = MockNode::new(scenario.channels.clone());
        let mut locals = Vec::new();
        for _ in 0..TICKS_PER_DAY {
            let chans = node.list_channels(ListChannelsParams::default()).await.unwrap();
            let ch = chans.iter().find(|c| c.channel_id == channel_id).unwrap();
            locals.push(ch.local_balance);
        }
        locals
    }

    #[test]
    fn day_geometry_is_exact() {
        assert_eq!(u64::from(TICKS_PER_DAY) * TICK_MS, 86_400_000);
        assert_eq!(SIM_EPOCH_MS % 86_400_000, 0, "epoch must be midnight UTC");
    }

    #[tokio::test]
    async fn steady_drain_flatlines_at_zero() {
        let locals = day_of_locals(&steady_drain(), "0xsim_drain").await;
        assert_eq!(locals[0], 800 * SHANNONS_PER_CKB);
        assert!(locals.windows(2).all(|w| w[1] <= w[0]), "monotone drain");
        assert_eq!(*locals.last().unwrap(), 0, "fully drained by day end");
        // 800 CKB at 5/tick: dry at tick 160.
        assert_eq!(locals[160], 0);
        assert!(locals[159] > 0);
    }

    #[tokio::test]
    async fn burst_is_calm_storm_calm() {
        let locals = day_of_locals(&burst(), "0xsim_burst").await;
        assert!(locals[..96].iter().all(|&l| l == 600 * SHANNONS_PER_CKB), "calm before");
        assert_eq!(locals[96], 600 * SHANNONS_PER_CKB); // storm starts moving at 97
        assert!(locals[100] < locals[96]);
        assert_eq!(locals[136], 0, "burst outruns the channel at tick 136");
        assert!(locals[144..].iter().all(|&l| l == 0), "calm (dry) after");
    }

    #[tokio::test]
    async fn oscillating_swings_inside_its_band_periodically() {
        let locals = day_of_locals(&oscillating(), "0xsim_osc").await;
        let lo = 270 * SHANNONS_PER_CKB;
        let hi = 750 * SHANNONS_PER_CKB;
        assert!(locals.iter().all(|&l| (lo..=hi).contains(&l)));
        assert_eq!(locals[0], hi);
        assert_eq!(locals[48], lo, "trough at half period");
        // Full 96-tick period: three identical cycles across the day.
        for t in 0..96 {
            assert_eq!(locals[t], locals[t + 96]);
            assert_eq!(locals[t], locals[t + 192]);
        }
    }

    #[test]
    fn scenarios_conserve_capacity_and_share_the_control_channels() {
        for scenario in all_scenarios() {
            assert_eq!(scenario.channels.len(), 3);
            let ids = scenario.channel_ids();
            assert!(ids.contains(&"0xsim_source".to_string()));
            assert!(ids.contains(&"0xsim_steady".to_string()));
        }
    }

    #[test]
    fn imbalance_metric_is_exact_and_capacity_weighted() {
        let snap = |id: &str, local: u128, remote: u128| ChannelSnapshot {
            channel_id: id.into(),
            peer: "02aa".into(),
            asset: evenkeel_core::Asset::Ckb,
            local_balance: local,
            remote_balance: remote,
            offered_tlc_balance: 0,
            received_tlc_balance: 0,
            ready: true,
            at_ms: 0,
        };
        // Perfectly balanced at a 5000 bp target: zero.
        assert_eq!(imbalance_weighted(&[snap("a", 500, 500)], 5_000), 0);
        // 900/100 vs target 5000: |9000−5000|·1000 in bp·Shannon = 400·10⁴.
        assert_eq!(imbalance_weighted(&[snap("a", 900, 100)], 5_000), 4_000_000);
        // Additive across channels; not-ready channels are invisible.
        let mut off = snap("b", 0, 1_000);
        assert_eq!(
            imbalance_weighted(&[snap("a", 900, 100), off.clone()], 5_000),
            4_000_000 + 5_000_000
        );
        off.ready = false;
        assert_eq!(imbalance_weighted(&[snap("a", 900, 100), off], 5_000), 4_000_000);
    }
}
