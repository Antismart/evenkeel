//! The `FiberRpc` trait and its two implementations.
//!
//! - [`RealNode`]: reqwest against a Fiber Network Node's JSON-RPC (v0.8.x).
//! - [`MockNode`]: scripted balance scenarios with deterministic evolution and
//!   fault injection. A first-class artifact (ADR-6): the dev environment, the
//!   CI environment, and the demo fallback — not test scaffolding.
//!
//! The trait carries exactly what the current phase consumes (rule 1: never
//! work ahead). Phase 2 extends it with the payment path (`send_payment`,
//!  `get_payment`, `build_router`, …).

pub mod error;
pub mod hex;
pub mod mock;
pub mod real;
pub mod rpc_types;

pub use error::NodeError;
pub use mock::{BalanceScript, MockBalances, MockChannelSpec, MockNode};
pub use real::RealNode;
pub use rpc_types::{Channel, ChannelStateInfo, ListChannelsParams, NodeInfo, CHANNEL_READY};

/// What Even Keel needs from a Fiber node. Implemented by [`RealNode`] and
/// [`MockNode`]; everything above this trait is network-agnostic.
#[async_trait::async_trait]
pub trait FiberRpc: Send + Sync {
    /// `node_info`: identity and counts for the node we manage.
    async fn node_info(&self) -> Result<NodeInfo, NodeError>;

    /// `list_channels`: all channels with balances and TLC locks — the
    /// poller's whole world in Phase 1.
    async fn list_channels(&self, params: ListChannelsParams) -> Result<Vec<Channel>, NodeError>;
}
