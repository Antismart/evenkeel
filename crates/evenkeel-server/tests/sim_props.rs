//! The §8 simulated-day property: **for any policy, total fees spent ≤ the
//! daily cap AND net imbalance does not increase over a simulated day** —
//! i.e. no policy setting can make Even Keel spend fees while leaving the
//! fleet worse-balanced than doing nothing.
//!
//! Runs the real harness (real executor + store + MockNode) over the
//! steady_drain scenario for arbitrary-but-sane policies. Half a simulated
//! day (144 ticks) keeps the 16-case suite well under the runtime budget
//! while still covering plan → price → approve → settle cycles and cooldowns.
//! Live-database test — no-ops without DATABASE_URL (like scenarios.rs).

#![allow(clippy::unwrap_used)]

use evenkeel_core::{HealthThresholds, Policy};
use evenkeel_server::sim;
use evenkeel_store::Store;
use proptest::prelude::*;

/// Ticks per case: half a simulated day (12h) — enough for several full
/// executor cycles under any cooldown in the generated range.
const TICKS: u32 = 144;

fn unique_run_id(case: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("prop{case}{nanos:x}")
}

/// Arbitrary-but-sane policies: target ratio across the whole actionable
/// band, money caps spanning several orders of magnitude (mantissa × 10^exp,
/// from sub-Shannon-fee scale to thousands of CKB), benefit floors from
/// "free-for-all" to "nothing is ever worth it", cooldowns 0..=50.
fn arb_policy() -> impl Strategy<Value = Policy> {
    (
        1_000u16..=9_000,          // target_ratio_bp
        (1u128..=9, 6u32..=13),    // max_amount_per_action = m × 10^e (0.01–90k CKB)
        (1u128..=9, 3u32..=9),     // max_fee_per_action = m × 10^e
        (1u128..=9, 3u32..=10),    // max_fee_per_day = m × 10^e
        0u64..=10_000,             // min_benefit_bp_per_ckb_fee
        0u64..=50,                 // cooldown_ticks
    )
        .prop_map(|(target, (am, ae), (fm, fe), (dm, de), floor, cooldown)| Policy {
            target_ratio_bp: target,
            max_amount_per_action: am * 10u128.pow(ae),
            max_fee_per_action: fm * 10u128.pow(fe),
            max_fee_per_day: dm * 10u128.pow(de),
            min_benefit_bp_per_ckb_fee: floor,
            cooldown_ticks: cooldown,
            thresholds: HealthThresholds::default(),
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        .. ProptestConfig::default()
    })]

    #[test]
    fn any_policy_respects_the_cap_and_never_worsens_imbalance(policy in arb_policy()) {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let report = rt.block_on(async {
                let store = Store::connect(&url).await.unwrap();
                let scenario = sim::steady_drain();
                sim::compare_scenario(&policy, &scenario, &store, &unique_run_id("sd"), TICKS)
                    .await
                    .unwrap()
            });

            // Fees ≤ daily cap: the ledger bound holds for every policy.
            prop_assert!(
                report.managed.total_fee <= policy.max_fee_per_day,
                "fees {} exceed daily cap {}",
                report.managed.total_fee,
                policy.max_fee_per_day,
            );
            // Net imbalance does not increase: managed never ends the day
            // worse-balanced than the untouched baseline (exact integer
            // bp·Shannon metric, same scripted traffic).
            prop_assert!(
                report.managed.imbalance_end <= report.baseline.imbalance_end,
                "managed end imbalance {} worse than baseline {}",
                report.managed.imbalance_end,
                report.baseline.imbalance_end,
            );
            // And the converse guard from the Phase 3 brief: fee spend
            // without net imbalance reduction is impossible.
            if report.managed.total_fee > 0 {
                prop_assert!(
                    report.managed.imbalance_end < report.baseline.imbalance_end,
                    "spent {} in fees without reducing imbalance",
                    report.managed.total_fee,
                );
            }
        }
    }
}
