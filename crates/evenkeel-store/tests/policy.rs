//! Live-database round-trip tests for the persisted per-node policy and the
//! §9 `policy_snapshot` audit column. No-ops without `DATABASE_URL`.

#![allow(clippy::unwrap_used)]

use evenkeel_core::{Asset, HealthThresholds, Policy};
use evenkeel_store::{ActionRecord, ActionState, Store, TransitionPatch};

async fn test_store() -> Option<Store> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(Store::connect(&url).await.unwrap())
}

fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{tag}-{nanos}")
}

#[tokio::test]
async fn unknown_node_has_no_policy() {
    let Some(store) = test_store().await else { return };
    assert_eq!(store.load_policy(&unique("nopolicy")).await.unwrap(), None);
}

#[tokio::test]
async fn policy_round_trips_and_upserts() {
    let Some(store) = test_store().await else { return };
    let node = unique("policy");

    // First save: the defaults, autopilot off.
    let defaults = Policy::default();
    store.save_policy(&node, &defaults, false, 1_000).await.unwrap();
    assert_eq!(
        store.load_policy(&node).await.unwrap(),
        Some((defaults, false))
    );

    // Upsert: every field changed, money at the u128 extreme, autopilot on.
    let custom = Policy {
        target_ratio_bp: 6_000,
        max_amount_per_action: u128::MAX,
        max_fee_per_action: u128::MAX - 1,
        max_fee_per_day: 123_456_789_012_345_678_901_234_567_890,
        min_benefit_bp_per_ckb_fee: 250,
        cooldown_ticks: 42,
        thresholds: HealthThresholds {
            depleted_below_bp: 1_500,
            saturated_above_bp: 8_500,
            drift_bp_per_hour: 750,
            min_drift_points: 5,
        },
    };
    store.save_policy(&node, &custom, true, 2_000).await.unwrap();
    assert_eq!(
        store.load_policy(&node).await.unwrap(),
        Some((custom, true)),
        "upsert must replace the single row, u128 money exact"
    );
}

#[tokio::test]
async fn action_policy_snapshot_round_trips() {
    let Some(store) = test_store().await else { return };
    let node = unique("snapaudit");
    let intent = unique("intent");

    store
        .insert_action(&ActionRecord {
            intent_id: intent.clone(),
            node_id: node.clone(),
            asset: Asset::Ckb,
            source_channel: "0xsat".into(),
            sink_channel: "0xdep".into(),
            amount: 10_000_000_000,
            benefit_bp: 1_500,
            state: ActionState::Priced,
            mode: "advisory".into(),
            quoted_fee: Some(10_000_000),
            actual_fee: None,
            payment_hash: None,
            reason: None,
            policy_snapshot: None,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        })
        .await
        .unwrap();

    // The autopilot execution path: PRICED → SUBMITTING sets mode and the §9
    // policy snapshot in the same guarded transition.
    let snapshot_json = serde_json::to_string(&Policy::default()).unwrap();
    store
        .transition_action(
            &intent,
            &[ActionState::Priced],
            ActionState::Submitting,
            TransitionPatch {
                mode: Some("autopilot".into()),
                policy_snapshot: Some(snapshot_json.clone()),
                ..Default::default()
            },
            2_000,
        )
        .await
        .unwrap();

    let a = &store.recent_actions(&node, 1).await.unwrap()[0];
    assert_eq!(a.mode, "autopilot");
    assert_eq!(a.policy_snapshot.as_deref(), Some(snapshot_json.as_str()));
    // The snapshot parses back into the exact authorizing policy.
    let parsed: Policy = serde_json::from_str(a.policy_snapshot.as_deref().unwrap()).unwrap();
    assert_eq!(parsed, Policy::default());
}
