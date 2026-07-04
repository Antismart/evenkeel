//! `MockNode`: scripted, fault-injectable `FiberRpc` implementation.
//!
//! A first-class artifact (ADR-6), not test scaffolding: it is the dev
//! environment, the CI environment, and the demo fallback. Balances evolve
//! deterministically per `list_channels` call ("tick"), so the same script
//! always produces the same trajectory. Deterministic payment fees join in
//! Phase 2 with the executor.

use std::sync::Mutex;

use crate::error::NodeError;
use crate::rpc_types::{Channel, ChannelStateInfo, ListChannelsParams, NodeInfo, CHANNEL_READY};
use crate::FiberRpc;

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

/// How a mock channel's balances evolve over ticks. All scripts are pure
/// functions of the tick number — replaying a scenario is exact.
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

#[derive(Debug)]
struct MockState {
    tick: u64,
    fail_next: u32,
    offline: bool,
}

/// The scripted node. Interior-mutable so it can be shared behind the same
/// `Arc<dyn FiberRpc>` the real node uses.
#[derive(Debug)]
pub struct MockNode {
    pubkey: String,
    channels: Vec<MockChannelSpec>,
    state: Mutex<MockState>,
}

impl MockNode {
    /// A node with the given scripted channels.
    pub fn new(channels: Vec<MockChannelSpec>) -> Self {
        Self {
            pubkey: "03deadbeef00000000000000000000000000000000000000000000000000000mock".into(),
            channels,
            state: Mutex::new(MockState { tick: 0, fail_next: 0, offline: false }),
        }
    }

    /// The demo scenario the server ships with when no FNN is configured:
    /// one healthy channel, one steadily draining toward depletion, one
    /// parked saturated. Shows every dashboard state without a network.
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

    /// Make the next `n` RPC calls fail with a transient injected fault.
    pub fn fail_next(&self, n: u32) {
        self.lock().fail_next = n;
    }

    /// Toggle a hard outage: every call fails until turned back on.
    pub fn set_offline(&self, offline: bool) {
        self.lock().offline = offline;
    }

    /// Current tick (number of successful `list_channels` calls so far).
    pub fn tick(&self) -> u64 {
        self.lock().tick
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        // A poisoned mutex here means a panic mid-tick in a test; the state
        // is a few integers and always consistent, so recover it.
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
        let tick = {
            let mut st = self.lock();
            let t = st.tick;
            st.tick += 1;
            t
        };
        Ok(self
            .channels
            .iter()
            .map(|spec| {
                let b = spec.script.at(tick);
                Channel {
                    channel_id: spec.channel_id.clone(),
                    pubkey: spec.peer.clone(),
                    channel_outpoint: None,
                    funding_udt_type_script: None,
                    state: ChannelStateInfo {
                        state_name: if spec.ready { CHANNEL_READY.into() } else { "NegotiatingFunding".into() },
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use evenkeel_core::{classify, HealthClass, HealthThresholds};

    #[tokio::test]
    async fn drain_script_moves_balance_deterministically() {
        let node = MockNode::demo();
        let first = node.list_channels(Default::default()).await.unwrap();
        let second = node.list_channels(Default::default()).await.unwrap();
        let d0 = first.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
        let d1 = second.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
        assert!(d1.local_balance < d0.local_balance);
        // Total capacity is conserved by a drain.
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

    /// End-to-end through the core: polling the draining mock long enough
    /// produces a Depleting classification before it ever hits Depleted.
    #[tokio::test]
    async fn drain_becomes_depleting_via_health_engine() {
        let node = MockNode::demo();
        let mut window = Vec::new();
        for i in 0..6u64 {
            let chans = node.list_channels(Default::default()).await.unwrap();
            let ch = chans.iter().find(|c| c.channel_id == "0xmock_draining").unwrap();
            // Poller stamps 5-minute intervals.
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
}
