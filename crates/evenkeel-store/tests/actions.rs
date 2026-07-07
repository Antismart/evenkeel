//! Live-database tests for the action log: guarded transitions, the
//! non-terminal scan, and the fee ledger. No-ops without `DATABASE_URL`.

#![allow(clippy::unwrap_used)]

use evenkeel_core::Asset;
use evenkeel_store::{ActionRecord, ActionState, Store, StoreError, TransitionPatch};

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

fn action(node: &str, intent: &str, at_ms: u64) -> ActionRecord {
    ActionRecord {
        intent_id: intent.into(),
        node_id: node.into(),
        asset: Asset::Ckb,
        source_channel: "0xsat".into(),
        sink_channel: "0xdep".into(),
        amount: 10_000_000_000,
        benefit_bp: 1_500,
        state: ActionState::Planned,
        mode: "advisory".into(),
        quoted_fee: None,
        actual_fee: None,
        payment_hash: None,
        reason: None,
        policy_snapshot: None,
        created_at_ms: at_ms,
        updated_at_ms: at_ms,
    }
}

#[tokio::test]
async fn happy_path_transitions_and_ledger() {
    let Some(store) = test_store().await else { return };
    let node = unique("happy");
    let intent = unique("intent");

    store.insert_action(&action(&node, &intent, 1_000)).await.unwrap();

    // planned → priced (with quote) → submitting → confirming (hash) → settled (actual fee)
    store
        .transition_action(
            &intent,
            &[ActionState::Planned],
            ActionState::Priced,
            TransitionPatch { quoted_fee: Some(10_000_000), ..Default::default() },
            2_000,
        )
        .await
        .unwrap();
    store
        .transition_action(&intent, &[ActionState::Priced], ActionState::Submitting,
            TransitionPatch::default(), 3_000)
        .await
        .unwrap();
    store
        .transition_action(
            &intent,
            &[ActionState::Submitting],
            ActionState::Confirming,
            TransitionPatch { payment_hash: Some("0xabc".into()), ..Default::default() },
            4_000,
        )
        .await
        .unwrap();
    store
        .transition_action(
            &intent,
            &[ActionState::Confirming],
            ActionState::Settled,
            TransitionPatch { actual_fee: Some(9_999_999), ..Default::default() },
            5_000,
        )
        .await
        .unwrap();

    let recent = store.recent_actions(&node, 10).await.unwrap();
    assert_eq!(recent.len(), 1);
    let a = &recent[0];
    assert_eq!(a.state, ActionState::Settled);
    assert_eq!(a.quoted_fee, Some(10_000_000));
    assert_eq!(a.actual_fee, Some(9_999_999));
    assert_eq!(a.payment_hash.as_deref(), Some("0xabc"));

    // Ledger counts the ACTUAL fee, inside the window only.
    assert_eq!(store.fee_spent_between(&node, 0, 10_000).await.unwrap(), 9_999_999);
    assert_eq!(store.fee_spent_between(&node, 6_000, 10_000).await.unwrap(), 0);
    // Terminal → nothing non-terminal left.
    assert!(store.non_terminal_actions(&node).await.unwrap().is_empty());
}

#[tokio::test]
async fn illegal_transition_is_refused() {
    let Some(store) = test_store().await else { return };
    let node = unique("illegal");
    let intent = unique("intent");
    store.insert_action(&action(&node, &intent, 1_000)).await.unwrap();

    // planned → settled is not a legal edge (must pass through the machine).
    let err = store
        .transition_action(&intent, &[ActionState::Confirming], ActionState::Settled,
            TransitionPatch::default(), 2_000)
        .await;
    assert!(matches!(err, Err(StoreError::TransitionRefused { .. })));

    // The row is untouched.
    let a = &store.recent_actions(&node, 1).await.unwrap()[0];
    assert_eq!(a.state, ActionState::Planned);
    assert_eq!(a.updated_at_ms, 1_000);
}

#[tokio::test]
async fn non_terminal_scan_finds_stuck_and_confirming() {
    let Some(store) = test_store().await else { return };
    let node = unique("scan");

    for (i, to) in [ActionState::Confirming, ActionState::Stuck, ActionState::Failed]
        .iter()
        .enumerate()
    {
        let intent = unique(&format!("i{i}"));
        store.insert_action(&action(&node, &intent, 1_000 + i as u64)).await.unwrap();
        store
            .transition_action(&intent, &[ActionState::Planned], *to,
                TransitionPatch::default(), 2_000)
            .await
            .unwrap();
    }

    let open = store.non_terminal_actions(&node).await.unwrap();
    assert_eq!(open.len(), 2, "confirming + stuck, not failed: {open:?}");
    assert!(open.iter().all(|a| !a.state.is_terminal()));
}
