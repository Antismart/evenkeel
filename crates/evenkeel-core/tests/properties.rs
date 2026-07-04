//! Property tests for the §8 core invariants: classification monotonicity,
//! threshold correctness, ratio safety across the full `u128` range, and
//! drift-slope sanity.

#![allow(clippy::unwrap_used)]

use evenkeel_core::{
    classify, drift_bp_per_hour, ratio_bp, Asset, ChannelSnapshot, HealthClass, HealthThresholds,
    BP_SCALE,
};
use proptest::prelude::*;

fn snap(local: u128, remote: u128, offered: u128, received: u128, at_ms: u64) -> ChannelSnapshot {
    ChannelSnapshot {
        channel_id: "0xabc".into(),
        peer: "02aa".into(),
        asset: Asset::Ckb,
        local_balance: local,
        remote_balance: remote,
        offered_tlc_balance: offered,
        received_tlc_balance: received,
        ready: true,
        at_ms,
    }
}

proptest! {
    /// Classification is monotone in usable ratio: for the same capacity and
    /// no drift signal, more local balance never yields a lower class.
    #[test]
    fn classification_monotone_in_ratio(
        capacity in 1u128..1_000_000_000_000,
        a in 0u16..=10_000,
        b in 0u16..=10_000,
    ) {
        let (lo_bp, hi_bp) = if a <= b { (a, b) } else { (b, a) };
        let lo_local = capacity * lo_bp as u128 / BP_SCALE;
        let hi_local = capacity * hi_bp as u128 / BP_SCALE;
        let th = HealthThresholds::default();
        let lo = classify(&[snap(lo_local, capacity - lo_local, 0, 0, 0)], &th).unwrap();
        let hi = classify(&[snap(hi_local, capacity - hi_local, 0, 0, 0)], &th).unwrap();
        prop_assert!(lo.class <= hi.class, "lo={:?} hi={:?}", lo, hi);
    }

    /// Threshold correctness: a single-snapshot window is Depleted iff the
    /// usable ratio is below the depleted threshold (or capacity is zero),
    /// Saturated iff above the saturated threshold, Healthy otherwise.
    #[test]
    fn thresholds_bind_exactly(
        local in 0u128..1_000_000_000_000,
        remote in 0u128..1_000_000_000_000,
        offered in 0u128..1_000_000_000_000,
    ) {
        let th = HealthThresholds::default();
        let s = snap(local, remote, offered, 0, 0);
        let ratio = s.usable_ratio_bp();
        let got = classify(&[s], &th).unwrap().class;
        let want = match ratio {
            None => HealthClass::Depleted,
            Some(r) if r < th.depleted_below_bp => HealthClass::Depleted,
            Some(r) if r > th.saturated_above_bp => HealthClass::Saturated,
            Some(_) => HealthClass::Healthy, // single point → no drift signal
        };
        prop_assert_eq!(got, want);
    }

    /// `ratio_bp` never exceeds 10_000, is `None` exactly when the whole is
    /// zero, and is total (no panic/overflow) across the full u128 range.
    #[test]
    fn ratio_bp_is_total_and_bounded(part in any::<u128>(), whole in any::<u128>()) {
        match ratio_bp(part, whole) {
            None => prop_assert_eq!(whole, 0),
            Some(bp) => {
                prop_assert!(whole > 0);
                prop_assert!(bp <= 10_000);
                if part >= whole {
                    // Saturates at 100% even when part exceeds whole.
                    prop_assert_eq!(bp, 10_000);
                }
            }
        }
    }

    /// Usable-liquidity math is total on arbitrary inputs and never exceeds
    /// the raw balances it derives from.
    #[test]
    fn usable_liquidity_is_bounded(
        local in any::<u128>(),
        remote in any::<u128>(),
        offered in any::<u128>(),
        received in any::<u128>(),
    ) {
        let s = snap(local, remote, offered, received, 0);
        prop_assert!(s.usable_out() <= s.local_balance);
        prop_assert!(s.usable_in() <= s.remote_balance);
    }

    /// Drift is translation-invariant: shifting every ratio by a constant
    /// offset leaves the slope unchanged.
    #[test]
    fn drift_translation_invariant(
        base in 0u128..4_000,
        offset in 0u128..4_000,
        deltas in prop::collection::vec(0u128..500, 3..12),
    ) {
        let capacity = 10_000u128;
        let mk = |shift: u128| -> Vec<ChannelSnapshot> {
            deltas
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let local = (base + shift + d).min(capacity);
                    snap(local, capacity - local, 0, 0, i as u64 * 60_000)
                })
                .collect()
        };
        // Guard: keep both series inside capacity so no .min() clamping skews one.
        prop_assume!(base + offset + 500 < capacity);
        let s1 = drift_bp_per_hour(&mk(0), 2);
        let s2 = drift_bp_per_hour(&mk(offset), 2);
        prop_assert_eq!(s1, s2);
    }

    /// A strictly monotone series produces a slope with the matching sign.
    #[test]
    fn drift_sign_matches_direction(
        step in 1u128..300,
        n in 3usize..10,
        rising in any::<bool>(),
    ) {
        let capacity = 10_000u128;
        let w: Vec<ChannelSnapshot> = (0..n)
            .map(|i| {
                let delta = step * i as u128;
                let local = if rising { 1_000 + delta } else { 9_000 - delta };
                snap(local, capacity - local, 0, 0, i as u64 * 600_000)
            })
            .collect();
        let slope = drift_bp_per_hour(&w, 2).unwrap();
        if rising {
            prop_assert!(slope > 0, "slope={slope}");
        } else {
            prop_assert!(slope < 0, "slope={slope}");
        }
    }
}
