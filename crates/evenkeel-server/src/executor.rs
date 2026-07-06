//! The executor: drives at most ONE rebalance action through the §6 state
//! machine. Serialized by construction (ADR-2) — the control loop calls
//! `tick` once per cycle, and `tick` never plans while any action is
//! non-terminal.
//!
//! ```text
//! PLANNED ──dry_run ok──▶ PRICED ──approved──▶ SUBMITTING ──accepted──▶ CONFIRMING ──▶ SETTLED
//!    │                      │                      │                        ├──▶ FAILED
//!    └──▶ REJECTED ◀────────┘ (declined/stale)     └─(rpc error)▶ FAILED    └──▶ STUCK ─▶ reconcile
//! ```
//!
//! Invariants enforced here:
//! - every send is preceded by a dry run on the same params (rule 6);
//! - the budget is re-checked at execution time, not just plan time;
//! - the channel pair is re-validated ready immediately before submitting;
//! - the action row (with `intent_id`) is written BEFORE the send RPC, so
//!   the §7 crash window is bounded and reconcilable;
//! - `actual_fee` (from `get_payment`) is what enters the ledger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use evenkeel_core::{
    accept_priced, plan, ChannelHealth, ChannelSnapshot, CooldownState, Policy, RebalanceIntent,
};
use evenkeel_node::{FiberRpc, PaymentStatus, SendPaymentParams};
use evenkeel_store::{ActionRecord, ActionState, Store, TransitionPatch};
use tracing::{error, info, warn};

use crate::metrics::Metrics;

/// A priced quote older than this is stale and re-planned rather than sent.
const PRICE_TTL_MS: u64 = 10 * 60 * 1_000;
/// A payment neither settling nor failing within this window goes STUCK.
const CONFIRM_DEADLINE_MS: u64 = 15 * 60 * 1_000;
/// §7 reconcile window: how close a listed payment's `created_at` must be to
/// the action's submit time to be adopted as ours.
const RECONCILE_WINDOW_MS: u64 = 10 * 60 * 1_000;

/// Operator decisions arriving from the API: `intent_id → approve?`.
pub type Approvals = Arc<Mutex<HashMap<String, bool>>>;

/// The serialized executor.
pub struct Executor {
    node: Arc<dyn FiberRpc>,
    store: Store,
    policy: Policy,
    node_id: String,
    cooldowns: CooldownState,
    approvals: Approvals,
    metrics: Arc<Metrics>,
    tick: u64,
    intent_seq: u64,
}

impl Executor {
    /// Build an executor for `node_id` (the managed node's pubkey).
    pub fn new(
        node: Arc<dyn FiberRpc>,
        store: Store,
        policy: Policy,
        node_id: String,
        approvals: Approvals,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            node,
            store,
            policy,
            node_id,
            cooldowns: CooldownState::default(),
            approvals,
            metrics,
            tick: 0,
            intent_seq: 0,
        }
    }

    /// Shannons spent from the daily budget so far today (UTC day window).
    pub async fn spent_today(&self, now_ms: u64) -> u128 {
        let day_start = now_ms - (now_ms % 86_400_000);
        self.store
            .fee_spent_between(&self.node_id, day_start, day_start + 86_400_000)
            .await
            .unwrap_or_else(|e| {
                // Conservative: an unreadable ledger counts as a full budget
                // (§7 — never spend against unknown accounting).
                error!(error = %e, "fee ledger unreadable; treating budget as exhausted");
                self.policy.max_fee_per_day
            })
    }

    /// §7 startup reconciliation. Runs once before the first tick; drives
    /// every non-terminal action as far toward terminal as the node's
    /// payment records allow.
    pub async fn reconcile_on_startup(&mut self, now_ms: u64) {
        let open = match self.store.non_terminal_actions(&self.node_id).await {
            Ok(o) => o,
            Err(e) => {
                error!(error = %e, "cannot read non-terminal actions; refusing to start planning");
                return;
            }
        };
        for action in open {
            let intent_id = action.intent_id.clone();
            info!(intent_id, state = ?action.state, "reconciling non-terminal action");
            match action.state {
                // Never priced or never approved: nothing was sent; close out.
                ActionState::Planned | ActionState::Priced => {
                    self.reject(&action.intent_id, "stale after restart", now_ms).await;
                }
                // Crash window: submitted (maybe), no hash recorded.
                ActionState::Submitting => match action.payment_hash.as_deref() {
                    Some(hash) => {
                        let hash = hash.to_string();
                        self.adopt_confirming(&action, &hash, now_ms).await;
                    }
                    None => self.reconcile_submitting(&action, now_ms).await,
                },
                // Hash known: just keep driving it.
                ActionState::Confirming | ActionState::Stuck => {
                    self.poll_confirming(&action, now_ms).await;
                }
                _ => {}
            }
        }
    }

    /// One serialized executor step. Returns without planning if any action
    /// is non-terminal (§6: the loop never plans over an unresolved action).
    pub async fn tick(
        &mut self,
        snapshots: &[ChannelSnapshot],
        healths: &[ChannelHealth],
        now_ms: u64,
    ) {
        self.tick += 1;
        self.cooldowns.prune(self.tick);

        let open = match self.store.non_terminal_actions(&self.node_id).await {
            Ok(o) => o,
            Err(e) => {
                error!(error = %e, "cannot read open actions; skipping executor tick");
                return;
            }
        };
        self.publish_state_metrics(&open);

        if let Some(action) = open.first() {
            if open.len() > 1 {
                // Should be impossible by construction; make it loud.
                error!(count = open.len(), "multiple non-terminal actions — serialization violated");
            }
            let action = action.clone();
            match action.state {
                ActionState::Priced => self.progress_priced(&action, snapshots, now_ms).await,
                ActionState::Confirming | ActionState::Stuck => {
                    self.poll_confirming(&action, now_ms).await
                }
                ActionState::Submitting => {
                    // Mid-run SUBMITTING means the send call is in flight on
                    // this very process — nothing to do until it returns.
                    warn!(intent_id = %action.intent_id, "action still submitting");
                }
                _ => {}
            }
            return;
        }

        self.plan_and_price(snapshots, healths, now_ms).await;
    }

    // ---- planning ----------------------------------------------------------

    async fn plan_and_price(
        &mut self,
        snapshots: &[ChannelSnapshot],
        healths: &[ChannelHealth],
        now_ms: u64,
    ) {
        let Some(intent) = plan(
            snapshots,
            healths,
            &self.policy,
            &self.cooldowns,
            self.tick,
            None,
        ) else {
            return;
        };

        self.intent_seq += 1;
        let intent_id = format!("ek-{now_ms:x}-{:04x}", self.intent_seq);
        info!(
            intent_id,
            source = %intent.source_channel,
            sink = %intent.sink_channel,
            amount = intent.amount,
            benefit_bp = intent.benefit_bp,
            "planned rebalance intent"
        );

        // Price before anything is committed (rule 6).
        let quote = match self.node.send_payment(self.rebalance_params(&intent, true)).await {
            Ok(q) => q,
            Err(e) => {
                warn!(intent_id, error = %e, "dry run failed; cooling pair");
                self.record_rejected(&intent, &intent_id, format!("dry run failed: {e}"), now_ms)
                    .await;
                return;
            }
        };

        let spent = self.spent_today(now_ms).await;
        if let Err(reason) = accept_priced(intent.benefit_bp, quote.fee, spent, &self.policy) {
            info!(intent_id, ?reason, fee = quote.fee, "priced intent rejected by policy");
            self.record_rejected(&intent, &intent_id, format!("{reason:?}"), now_ms).await;
            return;
        }

        // Surface as PRICED: advisory mode waits for the operator's click.
        let record = ActionRecord {
            intent_id: intent_id.clone(),
            node_id: self.node_id.clone(),
            asset: intent.asset.clone(),
            source_channel: intent.source_channel.clone(),
            sink_channel: intent.sink_channel.clone(),
            amount: intent.amount,
            benefit_bp: intent.benefit_bp,
            state: ActionState::Priced,
            mode: "advisory".into(),
            quoted_fee: Some(quote.fee),
            actual_fee: None,
            payment_hash: None,
            reason: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if let Err(e) = self.store.insert_action(&record).await {
            error!(intent_id, error = %e, "failed to persist priced intent");
            return;
        }
        info!(intent_id, fee = quote.fee, "intent priced; awaiting operator approval");
    }

    // ---- PRICED → SUBMITTING → CONFIRMING -----------------------------------

    async fn progress_priced(
        &mut self,
        action: &ActionRecord,
        snapshots: &[ChannelSnapshot],
        now_ms: u64,
    ) {
        // Stale quote: the network moved on; re-plan rather than send old math.
        if now_ms.saturating_sub(action.created_at_ms) > PRICE_TTL_MS {
            self.reject(&action.intent_id, "price expired unapproved", now_ms).await;
            self.note_pair_cooldown(action);
            return;
        }

        let decision = {
            let mut approvals = self.approvals.lock().unwrap_or_else(|e| e.into_inner());
            approvals.remove(&action.intent_id)
        };
        match decision {
            None => {} // keep waiting for the operator
            Some(false) => {
                info!(intent_id = %action.intent_id, "operator declined");
                self.reject(&action.intent_id, "declined by operator", now_ms).await;
                self.note_pair_cooldown(action);
            }
            Some(true) => self.execute_approved(action, snapshots, now_ms).await,
        }
    }

    async fn execute_approved(
        &mut self,
        action: &ActionRecord,
        snapshots: &[ChannelSnapshot],
        now_ms: u64,
    ) {
        // §6: re-check the budget at execution time — the ledger may have
        // moved since pricing.
        let quoted_fee = action.quoted_fee.unwrap_or(0);
        let spent = self.spent_today(now_ms).await;
        if let Err(reason) = accept_priced(action.benefit_bp, quoted_fee, spent, &self.policy) {
            self.reject(&action.intent_id, &format!("at execution: {reason:?}"), now_ms).await;
            self.note_pair_cooldown(action);
            return;
        }
        // §7: re-validate both channels are still ready.
        let ready = |id: &str| {
            snapshots
                .iter()
                .any(|s| s.channel_id == id && s.ready && s.capacity() > 0)
        };
        if !ready(&action.source_channel) || !ready(&action.sink_channel) {
            self.reject(&action.intent_id, "channel no longer ready", now_ms).await;
            self.note_pair_cooldown(action);
            return;
        }

        // Row moves to SUBMITTING BEFORE the RPC — the §6 crash-window rule.
        if let Err(e) = self
            .store
            .transition_action(
                &action.intent_id,
                &[ActionState::Priced],
                ActionState::Submitting,
                TransitionPatch::default(),
                now_ms,
            )
            .await
        {
            error!(intent_id = %action.intent_id, error = %e, "cannot mark submitting; not sending");
            return;
        }

        let intent = RebalanceIntent {
            asset: action.asset.clone(),
            source_channel: action.source_channel.clone(),
            sink_channel: action.sink_channel.clone(),
            amount: action.amount,
            benefit_bp: action.benefit_bp,
        };
        info!(intent_id = %action.intent_id, "submitting rebalance payment");
        match self.node.send_payment(self.rebalance_params(&intent, false)).await {
            Ok(payment) => {
                let ok = self
                    .store
                    .transition_action(
                        &action.intent_id,
                        &[ActionState::Submitting],
                        ActionState::Confirming,
                        TransitionPatch {
                            payment_hash: Some(payment.payment_hash.clone()),
                            ..Default::default()
                        },
                        now_ms,
                    )
                    .await;
                if let Err(e) = ok {
                    // Payment is in flight but we could not record the hash —
                    // exactly the crash window; reconciliation will adopt it.
                    error!(intent_id = %action.intent_id, error = %e, "failed to record payment hash");
                } else {
                    info!(intent_id = %action.intent_id, hash = %payment.payment_hash, "confirming");
                }
            }
            Err(e) => {
                warn!(intent_id = %action.intent_id, error = %e, "send failed after successful dry run");
                self.fail(&action.intent_id, &format!("send failed: {e}"), now_ms).await;
                self.note_pair_cooldown(action);
            }
        }
    }

    // ---- CONFIRMING / STUCK -------------------------------------------------

    async fn poll_confirming(&mut self, action: &ActionRecord, now_ms: u64) {
        let Some(hash) = action.payment_hash.clone() else {
            // Confirming without a hash cannot progress; reconcile as orphan.
            self.orphan(&action.intent_id, "confirming without payment hash", now_ms).await;
            return;
        };
        let payment = match self.node.get_payment(&hash).await {
            Ok(p) => p,
            Err(e) => {
                warn!(intent_id = %action.intent_id, error = %e, "get_payment failed; will retry");
                return;
            }
        };
        match payment.status {
            PaymentStatus::Success => {
                let ok = self
                    .store
                    .transition_action(
                        &action.intent_id,
                        &[ActionState::Confirming, ActionState::Stuck],
                        ActionState::Settled,
                        TransitionPatch { actual_fee: Some(payment.fee), ..Default::default() },
                        now_ms,
                    )
                    .await;
                match ok {
                    Ok(()) => {
                        info!(intent_id = %action.intent_id, actual_fee = payment.fee, "settled");
                        self.metrics.observe_action("settled", "advisory");
                        self.metrics.add_fee(payment.fee);
                        self.note_pair_cooldown(action);
                    }
                    Err(e) => error!(intent_id = %action.intent_id, error = %e, "settle transition refused"),
                }
            }
            PaymentStatus::Failed => {
                let reason = payment.failed_error.unwrap_or_else(|| "payment failed".into());
                self.fail(&action.intent_id, &reason, now_ms).await;
                self.note_pair_cooldown(action);
            }
            PaymentStatus::Created | PaymentStatus::Inflight => {
                let waited = now_ms.saturating_sub(action.updated_at_ms);
                if action.state == ActionState::Confirming && waited > CONFIRM_DEADLINE_MS {
                    warn!(intent_id = %action.intent_id, waited_ms = waited, "payment stuck — blocking new actions");
                    let _ = self
                        .store
                        .transition_action(
                            &action.intent_id,
                            &[ActionState::Confirming],
                            ActionState::Stuck,
                            TransitionPatch {
                                reason: Some("in flight past confirmation deadline".into()),
                                ..Default::default()
                            },
                            now_ms,
                        )
                        .await;
                    self.metrics.observe_action("stuck", "advisory");
                }
            }
        }
    }

    // ---- §7 crash-window reconciliation ------------------------------------

    async fn reconcile_submitting(&mut self, action: &ActionRecord, now_ms: u64) {
        let listed = match self.node.list_payments(Default::default()).await {
            Ok(l) => l,
            Err(e) => {
                warn!(intent_id = %action.intent_id, error = %e, "list_payments failed; leaving for next reconcile");
                return;
            }
        };
        // Match: a self-payment of exactly our amount created near our submit
        // time (§7). Amount + window keeps false adoption implausible.
        let matched = listed.payments.iter().find(|p| {
            p.is_self_payment_to(&self.node_id)
                && p.amount() == Some(action.amount)
                && p.created_at.abs_diff(action.updated_at_ms) <= RECONCILE_WINDOW_MS
        });
        match matched {
            Some(p) => {
                info!(intent_id = %action.intent_id, hash = %p.payment_hash, "adopted in-flight payment");
                let hash = p.payment_hash.clone();
                self.adopt_confirming(action, &hash, now_ms).await;
            }
            None => {
                warn!(intent_id = %action.intent_id, "no matching payment found; marking orphan-suspect");
                self.orphan(
                    &action.intent_id,
                    "submitted but no matching payment found in reconcile window",
                    now_ms,
                )
                .await;
            }
        }
    }

    async fn adopt_confirming(&mut self, action: &ActionRecord, hash: &str, now_ms: u64) {
        let ok = self
            .store
            .transition_action(
                &action.intent_id,
                &[ActionState::Submitting, ActionState::Confirming, ActionState::Stuck],
                ActionState::Confirming,
                TransitionPatch { payment_hash: Some(hash.to_string()), ..Default::default() },
                now_ms,
            )
            .await;
        if let Err(e) = ok {
            error!(intent_id = %action.intent_id, error = %e, "adopt transition refused");
            return;
        }
        let mut adopted = action.clone();
        adopted.state = ActionState::Confirming;
        adopted.payment_hash = Some(hash.to_string());
        self.poll_confirming(&adopted, now_ms).await;
    }

    // ---- small transition helpers ------------------------------------------

    fn rebalance_params(&self, intent: &RebalanceIntent, dry_run: bool) -> SendPaymentParams {
        SendPaymentParams {
            target_pubkey: Some(self.node_id.clone()),
            amount: Some(intent.amount),
            max_fee_amount: Some(self.policy.max_fee_per_action),
            keysend: Some(true),
            allow_self_payment: Some(true),
            udt_type_script: match &intent.asset {
                evenkeel_core::Asset::Ckb => None,
                evenkeel_core::Asset::Udt(s) => serde_json::from_str(s).ok(),
            },
            timeout: None,
            dry_run: Some(dry_run),
        }
    }

    async fn record_rejected(
        &mut self,
        intent: &RebalanceIntent,
        intent_id: &str,
        reason: String,
        now_ms: u64,
    ) {
        // Cool the pair either way so a permanently unroutable/unprofitable
        // pair doesn't re-propose every tick.
        self.cooldowns.note_action(
            &intent.source_channel,
            &intent.sink_channel,
            self.tick,
            self.policy.cooldown_ticks,
        );
        let record = ActionRecord {
            intent_id: intent_id.to_string(),
            node_id: self.node_id.clone(),
            asset: intent.asset.clone(),
            source_channel: intent.source_channel.clone(),
            sink_channel: intent.sink_channel.clone(),
            amount: intent.amount,
            benefit_bp: intent.benefit_bp,
            state: ActionState::Rejected,
            mode: "advisory".into(),
            quoted_fee: None,
            actual_fee: None,
            payment_hash: None,
            reason: Some(reason),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if let Err(e) = self.store.insert_action(&record).await {
            error!(intent_id, error = %e, "failed to record rejection");
        }
        self.metrics.observe_action("rejected", "advisory");
    }

    fn note_pair_cooldown(&mut self, action: &ActionRecord) {
        self.cooldowns.note_action(
            &action.source_channel,
            &action.sink_channel,
            self.tick,
            self.policy.cooldown_ticks,
        );
    }

    async fn reject(&mut self, intent_id: &str, reason: &str, now_ms: u64) {
        let ok = self
            .store
            .transition_action(
                intent_id,
                &[ActionState::Planned, ActionState::Priced],
                ActionState::Rejected,
                TransitionPatch { reason: Some(reason.to_string()), ..Default::default() },
                now_ms,
            )
            .await;
        if let Err(e) = ok {
            error!(intent_id, error = %e, "reject transition refused");
        } else {
            self.metrics.observe_action("rejected", "advisory");
        }
    }

    async fn fail(&mut self, intent_id: &str, reason: &str, now_ms: u64) {
        let ok = self
            .store
            .transition_action(
                intent_id,
                &[ActionState::Submitting, ActionState::Confirming, ActionState::Stuck],
                ActionState::Failed,
                TransitionPatch { reason: Some(reason.to_string()), ..Default::default() },
                now_ms,
            )
            .await;
        if let Err(e) = ok {
            error!(intent_id, error = %e, "fail transition refused");
        } else {
            self.metrics.observe_action("failed", "advisory");
        }
    }

    async fn orphan(&mut self, intent_id: &str, reason: &str, now_ms: u64) {
        let ok = self
            .store
            .transition_action(
                intent_id,
                &[ActionState::Submitting, ActionState::Confirming, ActionState::Stuck],
                ActionState::OrphanSuspect,
                TransitionPatch { reason: Some(reason.to_string()), ..Default::default() },
                now_ms,
            )
            .await;
        if let Err(e) = ok {
            error!(intent_id, error = %e, "orphan transition refused");
        } else {
            error!(intent_id, reason, "ORPHAN-SUSPECT action — operator attention required");
            self.metrics.observe_action("orphan_suspect", "advisory");
        }
    }

    fn publish_state_metrics(&self, open: &[ActionRecord]) {
        self.metrics.set_action_states(open.iter().map(|a| a.state.as_str()));
    }
}
