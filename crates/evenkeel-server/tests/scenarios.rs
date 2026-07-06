//! The MockNode scenario suite (Phase 2, architecture §8.2): drives the real
//! Executor against scripted node behavior and asserts the state machine
//! lands in the correct terminal states with an exact fee ledger.
//!
//! Scenarios: happy path, dry-run-yes-send-fails, stuck payment, transient
//! RPC failure during confirmation, and §7 crash-recovery reconciliation
//! (adopt, orphan, stale). Live-database tests — no-ops without DATABASE_URL.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use evenkeel_core::{classify, Asset, ChannelHealth, ChannelSnapshot, HealthThresholds, Policy};
use evenkeel_node::{
    BalanceScript, FiberRpc, MockBalances, MockChannelSpec, MockNode, PaymentScript,
    SendPaymentParams,
};
use evenkeel_server::executor::{Approvals, Executor};
use evenkeel_server::metrics::Metrics;
use evenkeel_store::{ActionRecord, ActionState, Store, TransitionPatch};

const CKB: u128 = 100_000_000;
const T0: u64 = 1_700_000_000_000;

fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("03test{tag}{nanos}")
}

async fn test_store() -> Option<Store> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(Store::connect(&url).await.unwrap())
}

/// A saturated + depleted constant pair: the planner acts on the first tick.
fn imbalanced_mock(pubkey: &str) -> Arc<MockNode> {
    Arc::new(
        MockNode::new(vec![
            MockChannelSpec {
                channel_id: "0xsat".into(),
                peer: "02aaaa".into(),
                script: BalanceScript::Constant(MockBalances::simple(900 * CKB, 100 * CKB)),
                ready: true,
            },
            MockChannelSpec {
                channel_id: "0xdep".into(),
                peer: "02bbbb".into(),
                script: BalanceScript::Constant(MockBalances::simple(100 * CKB, 900 * CKB)),
                ready: true,
            },
        ])
        .with_base_ms(T0)
        .with_pubkey(pubkey),
    )
}

struct Harness {
    node: Arc<MockNode>,
    store: Store,
    approvals: Approvals,
    executor: Executor,
    node_id: String,
}

impl Harness {
    async fn new(tag: &str) -> Option<Self> {
        let store = test_store().await?;
        let node_id = unique(tag);
        let node = imbalanced_mock(&node_id);
        let approvals = Approvals::default();
        let executor = Executor::new(
            node.clone(),
            store.clone(),
            Policy::default(),
            node_id.clone(),
            approvals.clone(),
            Arc::new(Metrics::new().unwrap()),
        );
        Some(Self { node, store, approvals, executor, node_id })
    }

    async fn observe(&self, at_ms: u64) -> (Vec<ChannelSnapshot>, Vec<ChannelHealth>) {
        let chans = self.node.list_channels(Default::default()).await.unwrap();
        let snaps: Vec<ChannelSnapshot> = chans.iter().map(|c| c.to_snapshot(at_ms)).collect();
        let healths = snaps
            .iter()
            .filter_map(|s| classify(std::slice::from_ref(s), &HealthThresholds::default()))
            .collect();
        (snaps, healths)
    }

    async fn tick(&mut self, at_ms: u64) {
        let (snaps, healths) = self.observe(at_ms).await;
        self.executor.tick(&snaps, &healths, at_ms).await;
    }

    fn approve(&self, intent_id: &str) {
        self.approvals
            .lock()
            .unwrap()
            .insert(intent_id.to_string(), true);
    }

    async fn actions(&self) -> Vec<ActionRecord> {
        self.store.recent_actions(&self.node_id, 50).await.unwrap()
    }

    async fn only_action(&self) -> ActionRecord {
        let all = self.actions().await;
        assert_eq!(all.len(), 1, "expected exactly one action: {all:?}");
        all.into_iter().next().unwrap()
    }

    async fn spent_today(&self, at_ms: u64) -> u128 {
        let day = at_ms - (at_ms % 86_400_000);
        self.store
            .fee_spent_between(&self.node_id, day, day + 86_400_000)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn happy_path_settles_with_exact_ledger() {
    let Some(mut h) = Harness::new("happy").await else { return };

    // Tick 1: plan + price → PRICED, waiting for the operator.
    h.tick(T0).await;
    let priced = h.only_action().await;
    assert_eq!(priced.state, ActionState::Priced);
    assert_eq!(priced.source_channel, "0xsat");
    assert_eq!(priced.sink_channel, "0xdep");
    // 400 CKB moves both channels exactly to target; quote = 0.1%.
    assert_eq!(priced.amount, 400 * CKB);
    assert_eq!(priced.quoted_fee, Some(400 * CKB / 1_000));

    // Nothing executes without approval.
    h.tick(T0 + 1_000).await;
    assert_eq!(h.only_action().await.state, ActionState::Priced);

    // Approve → submit → confirming.
    h.approve(&priced.intent_id);
    h.tick(T0 + 2_000).await;
    let confirming = h.only_action().await;
    assert_eq!(confirming.state, ActionState::Confirming);
    assert!(confirming.payment_hash.is_some());

    // Next tick polls get_payment → settled, actual fee in the ledger.
    h.tick(T0 + 3_000).await;
    let settled = h.only_action().await;
    assert_eq!(settled.state, ActionState::Settled);
    assert_eq!(settled.actual_fee, Some(400 * CKB / 1_000));
    assert_eq!(h.spent_today(T0 + 3_000).await, 400 * CKB / 1_000);

    // Pair cools down: the still-imbalanced fleet plans nothing next tick.
    h.tick(T0 + 4_000).await;
    assert_eq!(h.actions().await.len(), 1);
}

#[tokio::test]
async fn dry_run_yes_then_send_fails_terminates_failed() {
    let Some(mut h) = Harness::new("sendfail").await else { return };

    h.tick(T0).await;
    let priced = h.only_action().await;
    assert_eq!(priced.state, ActionState::Priced);

    // The dry run said yes at plan time; the real send will now fail.
    h.node.fail_next_send("temporary channel failure");
    h.approve(&priced.intent_id);
    h.tick(T0 + 1_000).await;

    let failed = h.only_action().await;
    assert_eq!(failed.state, ActionState::Failed);
    assert!(failed.reason.as_deref().unwrap().contains("send failed"));
    // Failure costs nothing: ledger empty, no payment exists on the node.
    assert_eq!(h.spent_today(T0 + 1_000).await, 0);
    assert_eq!(h.node.payment_count(), 0);
}

#[tokio::test]
async fn stuck_payment_blocks_the_queue() {
    let Some(mut h) = Harness::new("stuck").await else { return };

    h.tick(T0).await;
    let priced = h.only_action().await;
    h.node.queue_payment_script(PaymentScript::Stick);
    h.approve(&priced.intent_id);
    h.tick(T0 + 1_000).await; // submit → confirming

    // Within the deadline it stays confirming.
    h.tick(T0 + 60_000).await;
    assert_eq!(h.only_action().await.state, ActionState::Confirming);

    // Past the 15-minute deadline it goes STUCK…
    h.tick(T0 + 17 * 60_000).await;
    assert_eq!(h.only_action().await.state, ActionState::Stuck);

    // …and the stuck action blocks all new planning despite the fleet
    // still being maximally imbalanced.
    h.tick(T0 + 18 * 60_000).await;
    h.tick(T0 + 19 * 60_000).await;
    assert_eq!(h.actions().await.len(), 1, "stuck action must block the queue");
    assert_eq!(h.spent_today(T0 + 19 * 60_000).await, 0);
}

#[tokio::test]
async fn transient_rpc_failure_during_confirmation_recovers() {
    let Some(mut h) = Harness::new("timeout").await else { return };

    h.tick(T0).await;
    let priced = h.only_action().await;
    h.node.queue_payment_script(PaymentScript::SettleAfterPolls(1));
    h.approve(&priced.intent_id);
    h.tick(T0 + 1_000).await; // confirming

    // get_payment fails this tick; the action must survive unchanged.
    // (Observe first, then arm the fault, so it hits the executor's
    // get_payment and not the harness's own list_channels.)
    let (snaps, healths) = h.observe(T0 + 2_000).await;
    h.node.fail_next(1);
    h.executor.tick(&snaps, &healths, T0 + 2_000).await;
    assert_eq!(h.only_action().await.state, ActionState::Confirming);

    // Recovery on the next tick.
    h.tick(T0 + 3_000).await;
    assert_eq!(h.only_action().await.state, ActionState::Settled);
}

/// §7 crash window: an action stuck in SUBMITTING with no payment_hash, but
/// the payment actually went out — reconciliation adopts it by
/// (self-target, amount, time) and drives it to settlement.
#[tokio::test]
async fn crash_recovery_adopts_matching_payment() {
    let Some(mut h) = Harness::new("adopt").await else { return };
    let amount = 123 * CKB;

    // The pre-crash process: row written, send fired, hash never recorded.
    let intent_id = format!("crash-{}", h.node_id);
    let mut rec = ActionRecord {
        intent_id: intent_id.clone(),
        node_id: h.node_id.clone(),
        asset: Asset::Ckb,
        source_channel: "0xsat".into(),
        sink_channel: "0xdep".into(),
        amount,
        benefit_bp: 100,
        state: ActionState::Priced,
        mode: "advisory".into(),
        quoted_fee: Some(amount / 1_000),
        actual_fee: None,
        payment_hash: None,
        reason: None,
        created_at_ms: T0,
        updated_at_ms: T0,
    };
    h.store.insert_action(&rec).await.unwrap();
    h.store
        .transition_action(&intent_id, &[ActionState::Priced], ActionState::Submitting,
            TransitionPatch::default(), T0 + 500)
        .await
        .unwrap();
    rec.state = ActionState::Submitting;
    // The payment that actually left the node (created_at ≈ T0 within window).
    h.node
        .send_payment(SendPaymentParams {
            target_pubkey: Some(h.node_id.clone()),
            amount: Some(amount),
            keysend: Some(true),
            allow_self_payment: Some(true),
            dry_run: None,
            ..Default::default()
        })
        .await
        .unwrap();

    // "Restart": reconcile adopts the payment and drives it to settlement.
    h.executor.reconcile_on_startup(T0 + 60_000).await;
    let after = h.only_action().await;
    assert_eq!(after.state, ActionState::Settled, "{after:?}");
    assert_eq!(after.actual_fee, Some(amount / 1_000));
    assert!(after.payment_hash.is_some());
}

/// §7 crash window, no match: SUBMITTING with nothing on the node inside the
/// window → ORPHAN-SUSPECT, loudly terminal.
#[tokio::test]
async fn crash_recovery_marks_unmatched_orphan() {
    let Some(mut h) = Harness::new("orphan").await else { return };
    let intent_id = format!("orphan-{}", h.node_id);
    h.store
        .insert_action(&ActionRecord {
            intent_id: intent_id.clone(),
            node_id: h.node_id.clone(),
            asset: Asset::Ckb,
            source_channel: "0xsat".into(),
            sink_channel: "0xdep".into(),
            amount: 55 * CKB,
            benefit_bp: 100,
            state: ActionState::Planned,
            mode: "advisory".into(),
            quoted_fee: None,
            actual_fee: None,
            payment_hash: None,
            reason: None,
            created_at_ms: T0,
            updated_at_ms: T0,
        })
        .await
        .unwrap();
    h.store
        .transition_action(&intent_id, &[ActionState::Planned], ActionState::Submitting,
            TransitionPatch::default(), T0)
        .await
        .unwrap();

    h.executor.reconcile_on_startup(T0 + 60_000).await;
    let after = h.only_action().await;
    assert_eq!(after.state, ActionState::OrphanSuspect);
}

/// Restart with PLANNED/PRICED rows: nothing was ever sent — closed as stale.
#[tokio::test]
async fn crash_recovery_rejects_stale_priced() {
    let Some(mut h) = Harness::new("stale").await else { return };
    let intent_id = format!("stale-{}", h.node_id);
    h.store
        .insert_action(&ActionRecord {
            intent_id: intent_id.clone(),
            node_id: h.node_id.clone(),
            asset: Asset::Ckb,
            source_channel: "0xsat".into(),
            sink_channel: "0xdep".into(),
            amount: 10 * CKB,
            benefit_bp: 100,
            state: ActionState::Priced,
            mode: "advisory".into(),
            quoted_fee: Some(1_000_000),
            actual_fee: None,
            payment_hash: None,
            reason: None,
            created_at_ms: T0,
            updated_at_ms: T0,
        })
        .await
        .unwrap();

    h.executor.reconcile_on_startup(T0 + 60_000).await;
    let after = h.only_action().await;
    assert_eq!(after.state, ActionState::Rejected);
    assert!(after.reason.as_deref().unwrap().contains("stale"));
}
