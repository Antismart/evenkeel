//! Property tests for the §8 planner invariants: the planner never proposes
//! an amount exceeding surplus/deficit, never pairs cross-asset, never emits
//! a zero or over-cap amount, always strictly reduces imbalance, and the
//! post-pricing gate never lets a fee bust a budget.

#![allow(clippy::unwrap_used)]

use evenkeel_core::{
    accept_priced, classify, plan, Asset, ChannelSnapshot, CooldownState, HealthThresholds,
    Policy, RejectReason, BP_SCALE,
};
use proptest::prelude::*;

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

/// Arbitrary fleet: up to 8 channels with arbitrary splits across 2 assets.
fn fleet() -> impl Strategy<Value = Vec<ChannelSnapshot>> {
    prop::collection::vec(
        (1u128..1_000_000_000, 0u16..=10_000, prop::bool::ANY),
        2..8,
    )
    .prop_map(|specs| {
        specs
            .into_iter()
            .enumerate()
            .map(|(i, (capacity, ratio_bp, is_ckb))| {
                let local = capacity * ratio_bp as u128 / BP_SCALE;
                let asset = if is_ckb { Asset::Ckb } else { Asset::Udt("u1".into()) };
                snap(&format!("0xch{i}"), local, capacity - local, asset)
            })
            .collect()
    })
}

fn plan_fleet(snaps: &[ChannelSnapshot], policy: &Policy) -> Option<evenkeel_core::RebalanceIntent> {
    let healths: Vec<_> = snaps
        .iter()
        .filter_map(|s| classify(std::slice::from_ref(s), &HealthThresholds::default()))
        .collect();
    plan(snaps, &healths, policy, &CooldownState::default(), 0, None)
}

proptest! {
    /// Amount respects every §5.3 bound and the intent strictly helps.
    #[test]
    fn planned_amount_respects_all_bounds(snaps in fleet()) {
        let policy = Policy::default();
        if let Some(intent) = plan_fleet(&snaps, &policy) {
            let s = snaps.iter().find(|c| c.channel_id == intent.source_channel).unwrap();
            let d = snaps.iter().find(|c| c.channel_id == intent.sink_channel).unwrap();

            // Same asset only (§5.4).
            prop_assert_eq!(&s.asset, &d.asset);
            prop_assert_ne!(&intent.source_channel, &intent.sink_channel);

            // Never a zero or over-cap amount.
            prop_assert!(intent.amount > 0);
            prop_assert!(intent.amount <= policy.max_amount_per_action);

            // Source never pushed below target; sink never past it.
            let target = policy.target_ratio_bp as u128;
            let s_after = (s.usable_out() - intent.amount) * BP_SCALE / s.capacity();
            let d_after = (d.usable_out() + intent.amount) * BP_SCALE / d.capacity();
            prop_assert!(s_after >= target - 1, "source overshot: {s_after}");
            prop_assert!(d_after <= target + 1, "sink overshot: {d_after}");

            // The move strictly reduces imbalance.
            prop_assert!(intent.benefit_bp > 0 || intent.amount * BP_SCALE / s.capacity().max(d.capacity()) == 0);
        }
    }

    /// The planner only ever proposes source=Saturated/Filling and
    /// sink=Depleted/Depleting channels — never a healthy one.
    #[test]
    fn planner_only_touches_unhealthy_channels(snaps in fleet()) {
        let policy = Policy::default();
        if let Some(intent) = plan_fleet(&snaps, &policy) {
            let th = HealthThresholds::default();
            let s = snaps.iter().find(|c| c.channel_id == intent.source_channel).unwrap();
            let d = snaps.iter().find(|c| c.channel_id == intent.sink_channel).unwrap();
            let s_bp = s.usable_ratio_bp().unwrap();
            let d_bp = d.usable_ratio_bp().unwrap();
            prop_assert!(s_bp > th.saturated_above_bp, "source not saturated: {s_bp}");
            prop_assert!(d_bp < th.depleted_below_bp, "sink not depleted: {d_bp}");
        }
    }

    /// A cooled-down pair is never re-proposed within the cooldown window.
    #[test]
    fn cooldown_prevents_retrigger(snaps in fleet(), elapsed in 0u64..20) {
        let policy = Policy::default();
        if let Some(first) = plan_fleet(&snaps, &policy) {
            let mut cooldowns = CooldownState::default();
            cooldowns.note_action(&first.source_channel, &first.sink_channel, 0, policy.cooldown_ticks);
            let healths: Vec<_> = snaps
                .iter()
                .filter_map(|s| classify(std::slice::from_ref(s), &HealthThresholds::default()))
                .collect();
            let again = plan(&snaps, &healths, &policy, &cooldowns, elapsed, None);
            if elapsed < policy.cooldown_ticks {
                if let Some(again) = again {
                    let same_pair = again.source_channel == first.source_channel
                        && again.sink_channel == first.sink_channel;
                    prop_assert!(!same_pair, "pair re-triggered during cooldown");
                }
            }
        }
    }

    /// The post-pricing gate can never accept a fee that busts either budget,
    /// for any inputs whatsoever.
    #[test]
    fn budgets_are_inviolable(
        benefit in any::<u64>(),
        fee in any::<u128>(),
        spent in any::<u128>(),
    ) {
        let policy = Policy::default();
        if accept_priced(benefit, fee, spent, &policy).is_ok() {
            prop_assert!(fee <= policy.max_fee_per_action);
            prop_assert!(spent.saturating_add(fee) <= policy.max_fee_per_day);
        }
        // And symmetric: an over-budget fee is always rejected.
        if fee > policy.max_fee_per_action {
            prop_assert_eq!(
                accept_priced(benefit, fee, spent, &policy),
                Err(RejectReason::FeeExceedsPerActionCap)
            );
        }
    }
}
