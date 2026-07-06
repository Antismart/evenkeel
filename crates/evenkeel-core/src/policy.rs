//! Operator policy: the safety bounds every action must clear.
//!
//! Phase 2 carries the planner-relevant subset (budgets, benefit floor,
//! cooldown); the full policy engine with autopilot arrives in Phase 3.
//! All money is `u128` Shannons; the benefit/fee test is an integer
//! cross-multiplication — no division, no floats (ADR-7).

use serde::{Deserialize, Serialize};

use crate::health::HealthThresholds;
use crate::types::{BasisPoints, Shannons};

/// Shannons per CKB, the unit the benefit/fee floor is quoted against.
pub const SHANNONS_PER_CKB: u128 = 100_000_000;

/// Bounds and knobs for rebalancing decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Ratio channels are steered toward (§5.3 "aim for 0.50").
    pub target_ratio_bp: BasisPoints,
    /// Hard cap on a single rebalance amount.
    pub max_amount_per_action: Shannons,
    /// Hard cap on a single action's fee — also sent node-side as
    /// `max_fee_amount` (defense in depth).
    pub max_fee_per_action: Shannons,
    /// Daily fee budget; the worst case any failure combination can cost (§4).
    pub max_fee_per_day: Shannons,
    /// Minimum capacity-weighted imbalance reduction (bp) required per CKB
    /// of fee. An action below this floor is not worth its price.
    pub min_benefit_bp_per_ckb_fee: u64,
    /// Ticks a rebalanced pair is ineligible after an action (hysteresis).
    pub cooldown_ticks: u64,
    /// Classification thresholds.
    pub thresholds: HealthThresholds,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            target_ratio_bp: 5_000,
            max_amount_per_action: 1_000 * SHANNONS_PER_CKB,
            max_fee_per_action: SHANNONS_PER_CKB,        // 1 CKB
            max_fee_per_day: 10 * SHANNONS_PER_CKB,      // 10 CKB
            min_benefit_bp_per_ckb_fee: 100,
            cooldown_ticks: 10,
            thresholds: HealthThresholds::default(),
        }
    }
}

/// Why a priced intent was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Quoted fee exceeds `max_fee_per_action`.
    FeeExceedsPerActionCap,
    /// Quoted fee would bust the daily budget.
    DailyBudgetExhausted,
    /// Imbalance reduction per fee below the policy floor.
    BenefitTooLowForFee,
}

/// Post-pricing verdict (§5.3 step 5). Called with the dry-run fee at plan
/// time and re-checked with the same function at execution time against the
/// then-current ledger (§6: PRICED→SUBMITTING re-checks the budget).
pub fn accept_priced(
    benefit_bp: u64,
    fee: Shannons,
    spent_today: Shannons,
    policy: &Policy,
) -> Result<(), RejectReason> {
    if fee > policy.max_fee_per_action {
        return Err(RejectReason::FeeExceedsPerActionCap);
    }
    if spent_today.saturating_add(fee) > policy.max_fee_per_day {
        return Err(RejectReason::DailyBudgetExhausted);
    }
    // benefit_bp / (fee / 1 CKB) >= floor, as integer cross-multiplication:
    // benefit_bp * 1 CKB >= floor * fee. A zero fee is always worth it.
    if (benefit_bp as u128).saturating_mul(SHANNONS_PER_CKB)
        < (policy.min_benefit_bp_per_ckb_fee as u128).saturating_mul(fee)
    {
        return Err(RejectReason::BenefitTooLowForFee);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_enforces_all_three_bounds() {
        let p = Policy::default();
        // Fee over per-action cap.
        assert_eq!(
            accept_priced(10_000, p.max_fee_per_action + 1, 0, &p),
            Err(RejectReason::FeeExceedsPerActionCap)
        );
        // Fee busting the daily budget.
        assert_eq!(
            accept_priced(10_000, SHANNONS_PER_CKB, p.max_fee_per_day, &p),
            Err(RejectReason::DailyBudgetExhausted)
        );
        // Benefit floor: 100 bp/CKB required; 50 bp for 1 CKB fee → reject.
        assert_eq!(
            accept_priced(50, SHANNONS_PER_CKB, 0, &p),
            Err(RejectReason::BenefitTooLowForFee)
        );
        // 200 bp for 1 CKB → accept; zero fee always accepts.
        assert_eq!(accept_priced(200, SHANNONS_PER_CKB, 0, &p), Ok(()));
        assert_eq!(accept_priced(0, 0, 0, &p), Ok(()));
    }
}
