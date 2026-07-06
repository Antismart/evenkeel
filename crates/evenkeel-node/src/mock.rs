//! `MockNode`: scripted, fault-injectable `FiberRpc` implementation.
//!
//! A first-class artifact (ADR-6), not test scaffolding: it is the dev
//! environment, the CI environment, and the demo fallback. Balances evolve
//! deterministically per `list_channels` call ("tick"), payments settle
//! deterministically per `get_payment` poll, and fees are a fixed proportion
//! of the amount — the same script always produces the same trajectory.
//!
//! Fault hooks cover the §8 scenario suite: transient RPC failures, hard
//! outage, dry-run-says-yes-then-send-fails, payments that stick in flight,
//! and payments that fail outright.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::error::NodeError;
use crate::payments::{
    ListPaymentsParams, ListPaymentsResult, PaymentInfo, PaymentStatus, SendPaymentParams,
    SessionRoute, SessionRouteNode,
};
use crate::rpc_types::{Channel, ChannelStateInfo, ListChannelsParams, NodeInfo, CHANNEL_READY};
use crate::FiberRpc;

/// Deterministic mock fee: 0.1% of the amount (FNN's default
/// `tlc_fee_proportional_millionths = 1000`, matching the Phase 0 spike).
pub const MOCK_FEE_PPM: u128 = 1_000;

/// Balance state of one mock channel at a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MockBalances {
    /// Our side.
    pub local: u128,
    /// Their side.
    pub remote: u128,
    /// Locked in pending outgoing TLCs.
    pub offered_tlc: u128,
    /// Locked in pending incoming TLCs.
    pub received_tlc: u128,
}

impl MockBalances {
    /// Balances with no pending TLCs.
    pub fn simple(local: u128, remote: u128) -> Self {
        Self { local, remote, offered_tlc: 0, received_tlc: 0 }
    }
}

/// How a mock channel's balances evolve over ticks, before payment shifts.
#[derive(Debug, Clone)]
pub enum BalanceScript {
    /// Never changes.
    Constant(MockBalances),
    /// Shifts `per_tick` Shannons from local to remote every tick (routing
    /// traffic steadily draining our side), stopping at zero.
    Drain {
        /// Starting balances.
        start: MockBalances,
        /// Shannons moved local→remote per tick.
        per_tick: u128,
    },
    /// Explicit per-tick balances; holds the last entry once exhausted.
    Steps(Vec<MockBalances>),
}

impl BalanceScript {
    fn at(&self, tick: u64) -> MockBalances {
        match self {
            Self::Constant(b) => *b,
            Self::Drain { start, per_tick } => {
                let moved = per_tick.saturating_mul(tick as u128).min(start.local);
                MockBalances {
                    local: start.local - moved,
                    remote: start.remote.saturating_add(moved),
                    ..*start
                }
            }
            Self::Steps(steps) => steps
                .get((tick as usize).min(steps.len().saturating_sub(1)))
                .copied()
                .unwrap_or(MockBalances::simple(0, 0)),
        }
    }
}

/// Static description of one scripted channel.
#[derive(Debug, Clone)]
pub struct MockChannelSpec {
    /// Channel ID reported to callers.
    pub channel_id: String,
    /// Counterparty pubkey reported to callers.
    pub peer: String,
    /// Balance evolution.
    pub script: BalanceScript,
    /// Whether the channel reports `ChannelReady` + enabled.
    pub ready: bool,
}

/// How the next real (non-dry-run) payment behaves once created.
#[derive(Debug, Clone)]
pub enum PaymentScript {
    /// Settles successfully after this many `get_payment` polls (Inflight
    /// until then). Settlement moves the amount between the two most
    /// imbalanced ready channels and burns the fee.
    SettleAfterPolls(u32),
    /// Terminates `Failed` after this many polls; principal unmoved.
    FailAfterPolls(u32, String),
    /// Stays `Inflight` forever — the STUCK scenario.
    Stick,
}

#[derive(Debug, Clone)]
struct MockPayment {
    hash: String,
    amount: u128,
    fee: u128,
    created_at: u64,
    polls: u32,
    script: PaymentScript,
    status: PaymentStatus,
    failed_error: Option<String>,
    /// Channel the amount leaves / enters on settlement (donor, receiver).
    shift: Option<(String, String)>,
    applied: bool,
}

/// Cumulative settled-payment effect on one channel, applied on top of its
/// balance script at read time.
#[derive(Debug, Clone, Copy, Default)]
struct Shift {
    local: i128,
    remote: i128,
}

#[derive(Debug, Default)]
struct MockState {
    tick: u64,
    fail_next: u32,
    offline: bool,
    fail_next_send: Option<String>,
    next_payment_scripts: VecDeque<PaymentScript>,
    payments: Vec<MockPayment>,
    payment_counter: u64,
    shifts: HashMap<String, Shift>,
}

/// The scripted node. Interior-mutable so it can be shared behind the same
/// `Arc<dyn FiberRpc>` the real node uses.
#[derive(Debug)]
pub struct MockNode {
    pubkey: String,
    channels: Vec<MockChannelSpec>,
    /// Base timestamp for deterministic `created_at` values.
    base_ms: u64,
    state: Mutex<MockState>,
}

impl MockNode {
    /// A node with the given scripted channels.
    pub fn new(channels: Vec<MockChannelSpec>) -> Self {
        Self {
            pubkey: "03deadbeef00000000000000000000000000000000000000000000000000000mock".into(),
            channels,
            base_ms: 0,
            state: Mutex::new(MockState::default()),
        }
    }

    /// Set the base timestamp payments are stamped from (deterministic;
    /// scenario tests align it with their action timestamps).
    pub fn with_base_ms(mut self, base_ms: u64) -> Self {
        self.base_ms = base_ms;
        self
    }

    /// The demo scenario the server ships with when no FNN is configured:
    /// one healthy channel, one steadily draining toward depletion, one
    /// parked saturated. Gives the planner a real (source, sink) pair.
    pub fn demo() -> Self {
        const CKB: u128 = 100_000_000;
        Self::new(vec![
            MockChannelSpec {
                channel_id: "0xmock_healthy".into(),
                peer: "02aaaa".into(),
                script: BalanceScript::Constant(MockBalances::simple(500 * CKB, 500 * CKB)),
                ready: true,
            },
            MockChannelSpec {
                channel_id: "0xmock_draining".into(),
                peer: "02bbbb".into(),
                script: BalanceScript::Drain {
                    start: MockBalances::simple(800 * CKB, 200 * CKB),
                    per_tick: 12 * CKB,
                },
                ready: true,
            },
            MockChannelSpec {
                channel_id: "0xmock_saturated".into(),
                peer: "02cccc".into(),
                script: BalanceScript::Constant(MockBalances::simple(920 * CKB, 80 * CKB)),
                ready: true,
            },
        ])
    }

    /// The node's own pubkey (what `node_info` reports).
    pub fn pubkey(&self) -> &str {
        &self.pubkey
    }

    /// Make the next `n` RPC calls fail with a transient injected fault.
    pub fn fail_next(&self, n: u32) {
        self.lock().fail_next = n;
    }

    /// Toggle a hard outage: every call fails until turned back on.
    pub fn set_offline(&self, offline: bool) {
        self.lock().offline = offline;
    }

    /// Make the next real (non-dry-run) `send_payment` fail with an RPC
    /// error — the dry-run-says-yes-then-send-fails scenario.
    pub fn fail_next_send(&self, reason: impl Into<String>) {
        self.lock().fail_next_send = Some(reason.into());
    }

    /// Queue the behavior of the next created payment (defaults to
    /// `SettleAfterPolls(1)` when the queue is empty).
    pub fn queue_payment_script(&self, script: PaymentScript) {
        self.lock().next_payment_scripts.push_back(script);
    }

    /// Number of real payments created so far.
    pub fn payment_count(&self) -> usize {
        self.lock().payments.len()
    }

    /// Current tick (number of successful `list_channels` calls so far).
    pub fn tick(&self) -> u64 {
        self.lock().tick
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        // A poisoned mutex here means a panic mid-tick in a test; the state
        // is always consistent between calls, so recover it.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn check_faults(&self) -> Result<(), NodeError> {
        let mut st = self.lock();
        if st.offline {
            return Err(NodeError::Injected("node offline"));
        }
        if st.fail_next > 0 {
            st.fail_next -= 1;
            return Err(NodeError::Injected("transient RPC failure"));
        }
        Ok(())
    }

    /// Balances of one channel at the given tick, script plus settled shifts.
    fn balances_at(&self, st: &MockState, spec: &MockChannelSpec, tick: u64) -> MockBalances {
        let base = spec.script.at(tick);
        let shift = st.shifts.get(&spec.channel_id).copied().unwrap_or_default();
        let clamp = |v: u128, d: i128| -> u128 {
            let sum = v as i128 + d; // balances are far below i128::MAX
            sum.max(0) as u128
        };
        MockBalances {
            local: clamp(base.local, shift.local),
            remote: clamp(base.remote, shift.remote),
            ..base
        }
    }

    /// Pick (donor, receiver) for a settling self-payment: the ready channel
    /// with the most usable outbound funds the circle; the lowest-ratio ready
    /// channel receives. Mirrors what node routing would most plausibly do.
    fn pick_shift_pair(&self, st: &MockState) -> Option<(String, String)> {
        let tick = st.tick.saturating_sub(1); // last observed tick
        let ready: Vec<(&MockChannelSpec, MockBalances)> = self
            .channels
            .iter()
            .filter(|c| c.ready)
            .map(|c| (c, self.balances_at(st, c, tick)))
            .collect();
        if ready.len() < 2 {
            return None;
        }
        let donor = ready.iter().max_by_key(|(_, b)| b.local)?;
        let receiver = ready
            .iter()
            .filter(|(c, _)| c.channel_id != donor.0.channel_id)
            .min_by_key(|(_, b)| {
                let cap = b.local.saturating_add(b.remote).max(1);
                b.local.saturating_mul(10_000) / cap
            })?;
        Some((donor.0.channel_id.clone(), receiver.0.channel_id.clone()))
    }

    fn payment_info(&self, p: &MockPayment) -> PaymentInfo {
        PaymentInfo {
            payment_hash: p.hash.clone(),
            status: p.status,
            created_at: p.created_at,
            last_updated_at: p.created_at + p.polls as u64 * 1_000,
            failed_error: p.failed_error.clone(),
            fee: p.fee,
            routers: vec![SessionRoute {
                nodes: vec![
                    SessionRouteNode {
                        pubkey: "02hop".into(),
                        amount: p.amount,
                        channel_outpoint: serde_json::Value::Null,
                    },
                    SessionRouteNode {
                        pubkey: self.pubkey.clone(),
                        amount: p.amount,
                        channel_outpoint: serde_json::Value::Null,
                    },
                ],
            }],
        }
    }
}

#[async_trait::async_trait]
impl FiberRpc for MockNode {
    async fn node_info(&self) -> Result<NodeInfo, NodeError> {
        self.check_faults()?;
        Ok(NodeInfo {
            version: "0.8.1-mock".into(),
            commit_hash: None,
            pubkey: self.pubkey.clone(),
            node_name: Some("evenkeel-mock".into()),
            addresses: vec![],
            channel_count: self.channels.len() as u32,
            peers_count: self.channels.len() as u32,
        })
    }

    async fn list_channels(&self, _params: ListChannelsParams) -> Result<Vec<Channel>, NodeError> {
        self.check_faults()?;
        let mut st = self.lock();
        let tick = st.tick;
        st.tick += 1;
        Ok(self
            .channels
            .iter()
            .map(|spec| {
                let b = self.balances_at(&st, spec, tick);
                Channel {
                    channel_id: spec.channel_id.clone(),
                    pubkey: spec.peer.clone(),
                    channel_outpoint: None,
                    funding_udt_type_script: None,
                    state: ChannelStateInfo {
                        state_name: if spec.ready {
                            CHANNEL_READY.into()
                        } else {
                            "NegotiatingFunding".into()
                        },
                        state_flags: serde_json::Value::Null,
                    },
                    local_balance: b.local,
                    offered_tlc_balance: b.offered_tlc,
                    remote_balance: b.remote,
                    received_tlc_balance: b.received_tlc,
                    enabled: spec.ready,
                    created_at: 0,
                }
            })
            .collect())
    }

    async fn send_payment(&self, params: SendPaymentParams) -> Result<PaymentInfo, NodeError> {
        self.check_faults()?;
        let amount = params.amount.ok_or(NodeError::Rpc {
            code: -32602,
            message: "amount is required for keysend".into(),
        })?;
        let fee = amount.saturating_mul(MOCK_FEE_PPM) / 1_000_000;
        if let Some(max_fee) = params.max_fee_amount {
            if fee > max_fee {
                return Err(NodeError::Rpc {
                    code: -32000,
                    message: format!("fee {fee} exceeds max_fee_amount {max_fee}"),
                });
            }
        }

        let mut st = self.lock();
        if params.dry_run == Some(true) {
            // Pricing only: nothing is created, nothing moves.
            let created_at = self.base_ms + st.payment_counter * 1_000;
            let dry = MockPayment {
                hash: "0xdryrun".into(),
                amount,
                fee,
                created_at,
                polls: 0,
                script: PaymentScript::Stick,
                status: PaymentStatus::Created,
                failed_error: None,
                shift: None,
                applied: false,
            };
            return Ok(self.payment_info(&dry));
        }

        if let Some(reason) = st.fail_next_send.take() {
            return Err(NodeError::Rpc { code: -32000, message: reason });
        }

        st.payment_counter += 1;
        let script = st
            .next_payment_scripts
            .pop_front()
            .unwrap_or(PaymentScript::SettleAfterPolls(1));
        let shift = self.pick_shift_pair(&st);
        let payment = MockPayment {
            hash: format!("0x{:064x}", st.payment_counter),
            amount,
            fee,
            created_at: self.base_ms + st.payment_counter * 1_000,
            polls: 0,
            script,
            status: PaymentStatus::Created,
            failed_error: None,
            shift,
            applied: false,
        };
        let info = self.payment_info(&payment);
        st.payments.push(payment);
        Ok(info)
    }

    async fn get_payment(&self, payment_hash: &str) -> Result<PaymentInfo, NodeError> {
        self.check_faults()?;
        let mut st = self.lock();
        let idx = st
            .payments
            .iter()
            .position(|p| p.hash == payment_hash)
            .ok_or(NodeError::Rpc { code: -32000, message: "payment not found".into() })?;

        // Advance lifecycle on poll (deterministic: N polls, not wall time).
        let (settle_shift, amount, fee) = {
            let p = &mut st.payments[idx];
            if !p.status.is_terminal() {
                p.polls += 1;
                match &p.script {
                    PaymentScript::SettleAfterPolls(n) => {
                        p.status = if p.polls >= *n {
                            PaymentStatus::Success
                        } else {
                            PaymentStatus::Inflight
                        };
                    }
                    PaymentScript::FailAfterPolls(n, reason) => {
                        if p.polls >= *n {
                            p.status = PaymentStatus::Failed;
                            p.failed_error = Some(reason.clone());
                        } else {
                            p.status = PaymentStatus::Inflight;
                        }
                    }
                    PaymentScript::Stick => p.status = PaymentStatus::Inflight,
                }
            }
            if p.status == PaymentStatus::Success && !p.applied {
                p.applied = true;
                (p.shift.clone(), p.amount, p.fee)
            } else {
                (None, 0, 0)
            }
        };

        // Settlement moves the amount donor→receiver and burns the fee from
        // the donor side; per-channel capacity stays conserved.
        if let Some((donor, receiver)) = settle_shift {
            let out = (amount + fee) as i128;
            let d = st.shifts.entry(donor).or_default();
            d.local -= out;
            d.remote += out;
            let r = st.shifts.entry(receiver).or_default();
            r.local += amount as i128;
            r.remote -= amount as i128;
        }

        Ok(self.payment_info(&st.payments[idx]))
    }

    async fn list_payments(
        &self,
        params: ListPaymentsParams,
    ) -> Result<ListPaymentsResult, NodeError> {
        self.check_faults()?;
        let st = self.lock();
        let payments = st
            .payments
            .iter()
            .filter(|p| params.status.is_none_or(|s| p.status == s))
            .map(|p| self.payment_info(p))
            .collect();
        Ok(ListPaymentsResult { payments, last_cursor: None })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use evenkeel_core::{classify, HealthClass, HealthThresholds};

    fn rebalance_params(amount: u128) -> SendPaymentParams {
        SendPaymentParams {
            target_pubkey: Some("03mock".into()),
            amount: Some(amount),
            keysend: Some(true),
            allow_self_payment: Some(true),
            max_fee_amount: Some(amount / 100),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn drain_script_moves_balance_deterministically() {
        let node = MockNode::demo();
        let first = node.list_channels(Default::default()).await.unwrap();
        let second = node.list_channels(Default::default()).await.unwrap();
        let d0 = first.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
        let d1 = second.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
        assert!(d1.local_balance < d0.local_balance);
        assert_eq!(
            d0.local_balance + d0.remote_balance,
            d1.local_balance + d1.remote_balance
        );
    }

    #[tokio::test]
    async fn fault_injection_fails_then_recovers() {
        let node = MockNode::demo();
        node.fail_next(2);
        assert!(node.list_channels(Default::default()).await.is_err());
        assert!(node.node_info().await.is_err());
        assert!(node.list_channels(Default::default()).await.is_ok());

        node.set_offline(true);
        assert!(node.list_channels(Default::default()).await.is_err());
        node.set_offline(false);
        assert!(node.list_channels(Default::default()).await.is_ok());
    }

    #[tokio::test]
    async fn drain_becomes_depleting_via_health_engine() {
        let node = MockNode::demo();
        let mut window = Vec::new();
        for i in 0..6u64 {
            let chans = node.list_channels(Default::default()).await.unwrap();
            let ch = chans.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
            window.push(ch.to_snapshot(i * 300_000));
        }
        let health = classify(&window, &HealthThresholds::default()).unwrap();
        assert_eq!(health.class, HealthClass::Depleting, "{health:?}");
        assert!(health.drift_bp_per_hour.unwrap() < 0);
    }

    #[tokio::test]
    async fn steps_script_holds_last_entry() {
        let node = MockNode::new(vec![MockChannelSpec {
            channel_id: "0xsteps".into(),
            peer: "02dd".into(),
            script: BalanceScript::Steps(vec![
                MockBalances::simple(100, 0),
                MockBalances::simple(60, 40),
            ]),
            ready: true,
        }]);
        for want_local in [100u128, 60, 60, 60] {
            let chans = node.list_channels(Default::default()).await.unwrap();
            assert_eq!(chans[0].local_balance, want_local);
        }
    }

    #[tokio::test]
    async fn dry_run_prices_without_creating_state() {
        let node = MockNode::demo();
        let mut params = rebalance_params(10_000_000_000);
        params.dry_run = Some(true);
        let quote = node.send_payment(params).await.unwrap();
        assert_eq!(quote.fee, 10_000_000); // 0.1% deterministic
        assert_eq!(node.payment_count(), 0);
    }

    #[tokio::test]
    async fn happy_path_settles_and_moves_balance() {
        let node = MockNode::demo();
        node.list_channels(Default::default()).await.unwrap(); // tick so shifts have a baseline

        let amount = 10_000_000_000u128;
        let sent = node.send_payment(rebalance_params(amount)).await.unwrap();
        assert_eq!(sent.status, PaymentStatus::Created);

        let settled = node.get_payment(&sent.payment_hash).await.unwrap();
        assert_eq!(settled.status, PaymentStatus::Success);
        assert_eq!(settled.fee, amount / 1_000);
        assert!(settled.is_self_payment_to(node.pubkey()));

        // Saturated channel (most local) donated amount+fee; the lowest-ratio
        // ready channel at pick time — healthy at 50%, since the drain has
        // barely started — received the amount.
        let chans = node.list_channels(Default::default()).await.unwrap();
        let sat = chans.iter().find(|c| c.channel_id == "0xmock_saturated").unwrap();
        let healthy = chans.iter().find(|c| c.channel_id == "0xmock_healthy").unwrap();
        const CKB: u128 = 100_000_000;
        assert_eq!(sat.local_balance, 920 * CKB - amount - amount / 1_000);
        assert_eq!(healthy.local_balance, 500 * CKB + amount);
    }

    #[tokio::test]
    async fn dry_run_yes_then_send_fails() {
        let node = MockNode::demo();
        let mut dry = rebalance_params(1_000_000_000);
        dry.dry_run = Some(true);
        assert!(node.send_payment(dry).await.is_ok());

        node.fail_next_send("temporary channel failure");
        let err = node.send_payment(rebalance_params(1_000_000_000)).await;
        assert!(matches!(err, Err(NodeError::Rpc { .. })));
        assert_eq!(node.payment_count(), 0);

        // Next attempt goes through — the fault is one-shot.
        assert!(node.send_payment(rebalance_params(1_000_000_000)).await.is_ok());
    }

    #[tokio::test]
    async fn stuck_payment_stays_inflight() {
        let node = MockNode::demo();
        node.queue_payment_script(PaymentScript::Stick);
        let sent = node.send_payment(rebalance_params(1_000_000_000)).await.unwrap();
        for _ in 0..10 {
            let p = node.get_payment(&sent.payment_hash).await.unwrap();
            assert_eq!(p.status, PaymentStatus::Inflight);
        }
    }

    #[tokio::test]
    async fn failed_payment_moves_nothing() {
        let node = MockNode::demo();
        node.list_channels(Default::default()).await.unwrap();
        let before = node.list_channels(Default::default()).await.unwrap();

        node.queue_payment_script(PaymentScript::FailAfterPolls(2, "no route".into()));
        let sent = node.send_payment(rebalance_params(1_000_000_000)).await.unwrap();
        assert_eq!(
            node.get_payment(&sent.payment_hash).await.unwrap().status,
            PaymentStatus::Inflight
        );
        let failed = node.get_payment(&sent.payment_hash).await.unwrap();
        assert_eq!(failed.status, PaymentStatus::Failed);
        assert_eq!(failed.failed_error.as_deref(), Some("no route"));

        // Balance trajectory unchanged apart from the scripted drain.
        let after = node.list_channels(Default::default()).await.unwrap();
        let sat_b = before.iter().find(|c| c.channel_id == "0xmock_saturated").unwrap();
        let sat_a = after.iter().find(|c| c.channel_id == "0xmock_saturated").unwrap();
        assert_eq!(sat_b.local_balance, sat_a.local_balance);
    }

    #[tokio::test]
    async fn list_payments_filters_by_status() {
        let node = MockNode::demo();
        node.queue_payment_script(PaymentScript::SettleAfterPolls(1));
        node.queue_payment_script(PaymentScript::Stick);
        let a = node.send_payment(rebalance_params(1_000)).await.unwrap();
        let b = node.send_payment(rebalance_params(2_000)).await.unwrap();
        node.get_payment(&a.payment_hash).await.unwrap();
        node.get_payment(&b.payment_hash).await.unwrap();

        let all = node.list_payments(Default::default()).await.unwrap();
        assert_eq!(all.payments.len(), 2);
        let success = node
            .list_payments(ListPaymentsParams {
                status: Some(PaymentStatus::Success),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(success.payments.len(), 1);
        assert_eq!(success.payments[0].payment_hash, a.payment_hash);
    }
}
