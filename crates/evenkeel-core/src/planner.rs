//! The rebalance planner: greedy pairwise selection (§5.3, ADR-3).
//!
//! Given the latest snapshots and health verdicts, emit **at most one**
//! intent per tick: move liquidity from the most oversupplied channel to the
//! most starved one, same asset only, sized so neither side overshoots the
//! target. Pricing happens later (dry run in the executor) — the planner
//! never sees a fee; `accept_priced` (policy.rs) is the post-pricing gate.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::health::{ChannelHealth, HealthClass};
use crate::policy::Policy;
use crate::types::{Asset, ChannelSnapshot, Shannons, BP_SCALE};

/// One planned circular rebalance: `amount` leaves local on `source_channel`
/// and arrives local on `sink_channel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalanceIntent {
    /// Asset both channels are denominated in (§5.4: never crosses assets).
    pub asset: Asset,
    /// Oversupplied channel funds flow out of (S).
    pub source_channel: String,
    /// Starved channel funds flow into (D).
    pub sink_channel: String,
    /// Amount in Shannons.
    pub amount: Shannons,
    /// Capacity-weighted imbalance reduction this intent achieves, in bp —
    /// the numerator of the post-pricing benefit/fee test.
    pub benefit_bp: u64,
}

/// Cooldown bookkeeping (§5.3 hysteresis): a rebalanced pair is ineligible
/// for `cooldown_ticks` after an action so the system cannot oscillate a
/// pair back and forth burning fees. Pure data — caller owns persistence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CooldownState {
    /// Pair (ordered lexicographically) → first tick it is eligible again.
    until: HashMap<(String, String), u64>,
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

impl CooldownState {
    /// Whether this pair is still cooling at `now_tick`.
    pub fn is_cooling(&self, a: &str, b: &str, now_tick: u64) -> bool {
        self.until
            .get(&pair_key(a, b))
            .is_some_and(|&until| now_tick < until)
    }

    /// Record an action on this pair at `now_tick`.
    pub fn note_action(&mut self, a: &str, b: &str, now_tick: u64, cooldown_ticks: u64) {
        self.until
            .insert(pair_key(a, b), now_tick.saturating_add(cooldown_ticks));
    }

    /// Drop entries that expired before `now_tick` (bounded memory).
    pub fn prune(&mut self, now_tick: u64) {
        self.until.retain(|_, &mut until| until > now_tick);
    }
}

/// Shannons of usable balance above the target ratio (0 when at/below it).
fn surplus_above_target(s: &ChannelSnapshot, target_bp: u16) -> Shannons {
    let target_amount = s.capacity() / BP_SCALE * target_bp as u128
        + (s.capacity() % BP_SCALE) * target_bp as u128 / BP_SCALE;
    s.usable_out().saturating_sub(target_amount)
}

/// Shannons of usable balance missing below the target ratio.
fn deficit_below_target(d: &ChannelSnapshot, target_bp: u16) -> Shannons {
    let target_amount = d.capacity() / BP_SCALE * target_bp as u128
        + (d.capacity() % BP_SCALE) * target_bp as u128 / BP_SCALE;
    target_amount.saturating_sub(d.usable_out())
}

/// Absolute deviation from target in bp·Shannon units (capacity-weighted).
fn deviation(snap: &ChannelSnapshot, usable: Shannons, target_bp: u16) -> u128 {
    let cap = snap.capacity();
    if cap == 0 {
        return 0;
    }
    let ratio = usable.saturating_mul(BP_SCALE) / cap;
    ratio.abs_diff(target_bp as u128).saturating_mul(cap)
}

/// Capacity-weighted imbalance reduction (bp) from moving `amount` from S's
/// usable-out to D's usable-out, normalized by the pair's total capacity.
pub fn benefit_bp(
    source: &ChannelSnapshot,
    sink: &ChannelSnapshot,
    amount: Shannons,
    target_bp: u16,
) -> u64 {
    let before = deviation(source, source.usable_out(), target_bp)
        .saturating_add(deviation(sink, sink.usable_out(), target_bp));
    let after = deviation(source, source.usable_out().saturating_sub(amount), target_bp)
        .saturating_add(deviation(sink, sink.usable_out().saturating_add(amount), target_bp));
    let reduced = before.saturating_sub(after);
    let total_cap = source.capacity().saturating_add(sink.capacity()).max(1);
    (reduced / total_cap).min(u64::MAX as u128) as u64
}

/// Plan at most one rebalance intent (§5.3).
///
/// `snapshots` are the latest per channel; `healths` the matching verdicts;
/// `route_capacity` an optional upper bound from graph data (absent in v1 —
/// the executor's dry run rejects unroutable amounts, and failure costs
/// nothing). Returns `None` when nothing worth doing exists.
pub fn plan(
    snapshots: &[ChannelSnapshot],
    healths: &[ChannelHealth],
    policy: &Policy,
    cooldowns: &CooldownState,
    now_tick: u64,
    route_capacity: Option<Shannons>,
) -> Option<RebalanceIntent> {
    let by_id: HashMap<&str, &ChannelSnapshot> = snapshots
        .iter()
        .filter(|s| s.ready && s.capacity() > 0)
        .map(|s| (s.channel_id.as_str(), s))
        .collect();

    // Partition candidates by asset (§5.4), keyed for determinism.
    let mut sinks: BTreeMap<&Asset, Vec<(&ChannelSnapshot, u128)>> = BTreeMap::new();
    let mut sources: BTreeMap<&Asset, Vec<(&ChannelSnapshot, u128)>> = BTreeMap::new();
    for h in healths {
        let Some(snap) = by_id.get(h.channel_id.as_str()) else { continue };
        match h.class {
            HealthClass::Depleted | HealthClass::Depleting => {
                let key = deficit_below_target(snap, policy.target_ratio_bp)
                    .saturating_mul(snap.capacity());
                sinks.entry(&snap.asset).or_default().push((snap, key));
            }
            HealthClass::Saturated | HealthClass::Filling => {
                let key = surplus_above_target(snap, policy.target_ratio_bp)
                    .saturating_mul(snap.capacity());
                sources.entry(&snap.asset).or_default().push((snap, key));
            }
            HealthClass::Healthy => {}
        }
    }

    let mut best: Option<RebalanceIntent> = None;
    for (asset, mut ds) in sinks {
        let Some(mut ss) = sources.remove(asset) else { continue };
        // §5.3 steps 1–2: strongest deficit and surplus first (deterministic
        // tie-break on channel id).
        ds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.channel_id.cmp(&b.0.channel_id)));
        ss.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.channel_id.cmp(&b.0.channel_id)));

        // Top pair not in cooldown; walk pairs in rank order.
        let pair = ds.iter().find_map(|(d, _)| {
            ss.iter()
                .find(|(s, _)| {
                    s.channel_id != d.channel_id
                        && !cooldowns.is_cooling(&s.channel_id, &d.channel_id, now_tick)
                })
                .map(|(s, _)| (*d, *s))
        });
        let Some((d, s)) = pair else { continue };

        // §5.3 step 3: don't push S below target, don't overshoot D past it.
        let mut amount = surplus_above_target(s, policy.target_ratio_bp)
            .min(deficit_below_target(d, policy.target_ratio_bp))
            .min(policy.max_amount_per_action);
        if let Some(cap) = route_capacity {
            amount = amount.min(cap);
        }
        if amount == 0 {
            continue;
        }

        let intent = RebalanceIntent {
            asset: asset.clone(),
            source_channel: s.channel_id.clone(),
            sink_channel: d.channel_id.clone(),
            amount,
            benefit_bp: benefit_bp(s, d, amount, policy.target_ratio_bp),
        };
        // One intent per tick: keep the highest-benefit asset's pair.
        if best.as_ref().is_none_or(|b| intent.benefit_bp > b.benefit_bp) {
            best = Some(intent);
        }
    }
    best
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::health::{classify, HealthThresholds};

    fn snap(id: &str, local: u128, remote: u128, asset: Asset) -> ChannelSnapshot {
        ChannelSnapshot {
            channel_id: id.into(),
            peer: "02aa".into(),
            asset,
            local_balance: local,
            remote_balance: remote,
            offered_tlc_balance: 0,
            received_tlc_balance: 0,
            ready: true,
            at_ms: 0,
        }
    }

    fn health_of(s: &ChannelSnapshot) -> ChannelHealth {
        classify(std::slice::from_ref(s), &HealthThresholds::default()).unwrap()
    }

    #[test]
    fn plans_saturated_into_depleted_sized_to_target() {
        let s = snap("0xsat", 9_000, 1_000, Asset::Ckb); // 90%
        let d = snap("0xdep", 1_000, 9_000, Asset::Ckb); // 10%
        let snaps = vec![s.clone(), d.clone()];
        let healths = vec![health_of(&s), health_of(&d)];
        let intent = plan(
            &snaps,
            &healths,
            &Policy::default(),
            &CooldownState::default(),
            0,
            None,
        )
        .unwrap();
        assert_eq!(intent.source_channel, "0xsat");
        assert_eq!(intent.sink_channel, "0xdep");
        // Surplus = 4000, deficit = 4000 → both land exactly on target.
        assert_eq!(intent.amount, 4_000);
        assert!(intent.benefit_bp > 0);
    }

    #[test]
    fn nothing_to_do_when_all_healthy() {
        let a = snap("0xa", 5_000, 5_000, Asset::Ckb);
        let b = snap("0xb", 6_000, 4_000, Asset::Ckb);
        let snaps = vec![a.clone(), b.clone()];
        let healths = vec![health_of(&a), health_of(&b)];
        assert_eq!(
            plan(&snaps, &healths, &Policy::default(), &CooldownState::default(), 0, None),
            None
        );
    }

    #[test]
    fn cooldown_blocks_the_pair_then_releases() {
        let s = snap("0xsat", 9_000, 1_000, Asset::Ckb);
        let d = snap("0xdep", 1_000, 9_000, Asset::Ckb);
        let snaps = vec![s.clone(), d.clone()];
        let healths = vec![health_of(&s), health_of(&d)];
        let policy = Policy::default();

        let mut cooldowns = CooldownState::default();
        cooldowns.note_action("0xsat", "0xdep", 0, policy.cooldown_ticks);
        assert_eq!(plan(&snaps, &healths, &policy, &cooldowns, 5, None), None);
        assert!(plan(&snaps, &healths, &policy, &cooldowns, policy.cooldown_ticks, None).is_some());
    }

    #[test]
    fn never_pairs_across_assets() {
        let s = snap("0xsat_ckb", 9_000, 1_000, Asset::Ckb);
        let d = snap("0xdep_udt", 1_000, 9_000, Asset::Udt("scriptA".into()));
        let snaps = vec![s.clone(), d.clone()];
        let healths = vec![health_of(&s), health_of(&d)];
        assert_eq!(
            plan(&snaps, &healths, &Policy::default(), &CooldownState::default(), 0, None),
            None
        );
    }

    #[test]
    fn route_capacity_bounds_the_amount() {
        let s = snap("0xsat", 9_000, 1_000, Asset::Ckb);
        let d = snap("0xdep", 1_000, 9_000, Asset::Ckb);
        let snaps = vec![s.clone(), d.clone()];
        let healths = vec![health_of(&s), health_of(&d)];
        let intent = plan(
            &snaps,
            &healths,
            &Policy::default(),
            &CooldownState::default(),
            0,
            Some(1_500),
        )
        .unwrap();
        assert_eq!(intent.amount, 1_500);
    }
}
