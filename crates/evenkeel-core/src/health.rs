//! Health classification and drift detection (§5.2).
//!
//! Classification is a pure function `(snapshot window, thresholds) →
//! ChannelHealth`. Drift is the slope of `usable_ratio_bp` over the recent
//! window, computed with exact integer linear regression — a channel at 35%
//! and falling fast is a better rebalance target than one parked at 22%.

use serde::{Deserialize, Serialize};

use crate::types::{BasisPoints, ChannelSnapshot};

/// Health classes, ordered from most outbound-starved to most outbound-heavy.
///
/// The derived `Ord` follows that liquidity ordering, which the monotonicity
/// property test relies on: more usable ratio never yields a lower class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthClass {
    /// Usable ratio below the depleted threshold: cannot forward outbound;
    /// needs a refill.
    Depleted,
    /// Trending toward depletion fast enough to act early.
    Depleting,
    /// Inside the healthy band with no significant drift.
    Healthy,
    /// Trending toward saturation.
    Filling,
    /// Usable ratio above the saturated threshold: a rebalance source.
    Saturated,
}

/// Classification thresholds. A subset of operator policy — the full policy
/// engine is Phase 3; these defaults mirror §5.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthThresholds {
    /// Below this usable ratio (bp) a channel is `Depleted`. Default 2_000 (20%).
    pub depleted_below_bp: BasisPoints,
    /// Above this usable ratio (bp) a channel is `Saturated`. Default 8_000 (80%).
    pub saturated_above_bp: BasisPoints,
    /// Absolute drift slope (bp per hour) at or above which a mid-band channel
    /// is `Depleting`/`Filling`. Default 500 (5 percentage points per hour).
    pub drift_bp_per_hour: u32,
    /// Minimum snapshots in the window before drift is trusted. Default 3;
    /// never below 2 (a slope needs two points).
    pub min_drift_points: usize,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            depleted_below_bp: 2_000,
            saturated_above_bp: 8_000,
            drift_bp_per_hour: 500,
            min_drift_points: 3,
        }
    }
}

/// The health engine's verdict on one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelHealth {
    /// Channel this verdict is about.
    pub channel_id: String,
    /// Assigned class.
    pub class: HealthClass,
    /// Latest usable ratio in basis points (`None` for zero-capacity).
    pub usable_ratio_bp: Option<BasisPoints>,
    /// Drift slope in basis points per hour over the window; `None` when the
    /// window is too short to trust.
    pub drift_bp_per_hour: Option<i64>,
}

/// Classify one channel from its time-ordered snapshot window (oldest first).
///
/// The window must contain snapshots of a single channel; the newest snapshot
/// decides the level, the whole window decides the drift. Returns `None` for
/// an empty window or when the newest snapshot is not ready (not
/// CHANNEL_READY / disabled channels are invisible to decisions).
pub fn classify(
    window: &[ChannelSnapshot],
    thresholds: &HealthThresholds,
) -> Option<ChannelHealth> {
    let newest = window.last()?;
    if !newest.ready {
        return None;
    }
    let ratio = newest.usable_ratio_bp();
    let drift = drift_bp_per_hour(window, thresholds.min_drift_points.max(2));

    let class = match ratio {
        None => HealthClass::Depleted, // zero capacity forwards nothing
        Some(r) if r < thresholds.depleted_below_bp => HealthClass::Depleted,
        Some(r) if r > thresholds.saturated_above_bp => HealthClass::Saturated,
        Some(_) => match drift {
            Some(s) if s <= -(thresholds.drift_bp_per_hour as i64) => HealthClass::Depleting,
            Some(s) if s >= thresholds.drift_bp_per_hour as i64 => HealthClass::Filling,
            _ => HealthClass::Healthy,
        },
    };

    Some(ChannelHealth {
        channel_id: newest.channel_id.clone(),
        class,
        usable_ratio_bp: ratio,
        drift_bp_per_hour: drift,
    })
}

/// Slope of `usable_ratio_bp` over the window, in basis points per hour.
///
/// Exact integer least-squares over `(seconds since first snapshot,
/// ratio_bp)` points: `slope = (n·Σty − Σt·Σy) / (n·Σt² − (Σt)²)`, scaled to
/// per-hour before the final division so integer truncation costs at most
/// 1 bp/h. Returns `None` when there are fewer than `min_points` usable
/// points or no time spread. All intermediates are `i128`; with ratios
/// ≤ 10^4 and windows of days, nothing approaches overflow.
pub fn drift_bp_per_hour(window: &[ChannelSnapshot], min_points: usize) -> Option<i64> {
    let first_ms = window.first()?.at_ms;
    let points: Vec<(i128, i128)> = window
        .iter()
        .filter_map(|s| {
            s.usable_ratio_bp()
                .map(|r| ((s.at_ms.saturating_sub(first_ms) / 1_000) as i128, r as i128))
        })
        .collect();
    if points.len() < min_points.max(2) {
        return None;
    }

    let n = points.len() as i128;
    let sum_t: i128 = points.iter().map(|(t, _)| t).sum();
    let sum_y: i128 = points.iter().map(|(_, y)| y).sum();
    let sum_ty: i128 = points.iter().map(|(t, y)| t * y).sum();
    let sum_tt: i128 = points.iter().map(|(t, _)| t * t).sum();

    let denom = n * sum_tt - sum_t * sum_t;
    if denom == 0 {
        return None; // all snapshots at the same second: no slope
    }
    let numer = n * sum_ty - sum_t * sum_y;
    Some(((numer * 3_600) / denom) as i64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::Asset;

    fn snap_at(at_ms: u64, local: u128, remote: u128) -> ChannelSnapshot {
        ChannelSnapshot {
            channel_id: "0xabc".into(),
            peer: "02aa".into(),
            asset: Asset::Ckb,
            local_balance: local,
            remote_balance: remote,
            offered_tlc_balance: 0,
            received_tlc_balance: 0,
            ready: true,
            at_ms,
        }
    }

    fn th() -> HealthThresholds {
        HealthThresholds::default()
    }

    #[test]
    fn classifies_levels_from_newest_snapshot() {
        let depleted = [snap_at(0, 100, 900)];
        let healthy = [snap_at(0, 500, 500)];
        let saturated = [snap_at(0, 900, 100)];
        assert_eq!(classify(&depleted, &th()).unwrap().class, HealthClass::Depleted);
        assert_eq!(classify(&healthy, &th()).unwrap().class, HealthClass::Healthy);
        assert_eq!(classify(&saturated, &th()).unwrap().class, HealthClass::Saturated);
    }

    #[test]
    fn boundary_values_are_not_extreme_classes() {
        // Exactly at the thresholds: 20.00% and 80.00% are mid-band.
        let at_low = [snap_at(0, 2_000, 8_000)];
        let at_high = [snap_at(0, 8_000, 2_000)];
        assert_eq!(classify(&at_low, &th()).unwrap().class, HealthClass::Healthy);
        assert_eq!(classify(&at_high, &th()).unwrap().class, HealthClass::Healthy);
    }

    #[test]
    fn zero_capacity_is_depleted() {
        let w = [snap_at(0, 0, 0)];
        let h = classify(&w, &th()).unwrap();
        assert_eq!(h.class, HealthClass::Depleted);
        assert_eq!(h.usable_ratio_bp, None);
    }

    #[test]
    fn not_ready_channels_are_invisible() {
        let mut s = snap_at(0, 500, 500);
        s.ready = false;
        assert_eq!(classify(&[s], &th()), None);
    }

    #[test]
    fn detects_depleting_drift_before_threshold_hit() {
        // 50% → 35% over 30 minutes = -3000 bp/h: falling fast, still mid-band.
        let w = [
            snap_at(0, 5_000, 5_000),
            snap_at(900_000, 4_250, 5_750),
            snap_at(1_800_000, 3_500, 6_500),
        ];
        let h = classify(&w, &th()).unwrap();
        assert_eq!(h.class, HealthClass::Depleting);
        assert!(h.drift_bp_per_hour.unwrap() <= -2_900);
    }

    #[test]
    fn exact_linear_data_recovers_exact_slope() {
        // +100 bp every hour, exactly.
        let w: Vec<_> = (0..5)
            .map(|i| snap_at(i * 3_600_000, 5_000 + i as u128 * 100, 5_000 - i as u128 * 100))
            .collect();
        assert_eq!(drift_bp_per_hour(&w, 2), Some(100));
    }

    #[test]
    fn constant_series_has_zero_slope() {
        let w: Vec<_> = (0..4).map(|i| snap_at(i * 60_000, 5_000, 5_000)).collect();
        assert_eq!(drift_bp_per_hour(&w, 2), Some(0));
    }

    #[test]
    fn short_or_degenerate_windows_yield_no_drift() {
        assert_eq!(drift_bp_per_hour(&[snap_at(0, 1, 1)], 2), None);
        // Same timestamp twice: no time spread.
        let same = [snap_at(5_000, 100, 100), snap_at(5_000, 200, 0)];
        assert_eq!(drift_bp_per_hour(&same, 2), None);
    }

    #[test]
    fn depleted_beats_drift() {
        // Below threshold AND rising fast: still Depleted (level wins).
        let w = [
            snap_at(0, 500, 9_500),
            snap_at(900_000, 1_000, 9_000),
            snap_at(1_800_000, 1_500, 8_500),
        ];
        assert_eq!(classify(&w, &th()).unwrap().class, HealthClass::Depleted);
    }
}
