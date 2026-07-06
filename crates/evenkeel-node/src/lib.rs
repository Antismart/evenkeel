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
pub mod payments;
pub mod real;
pub mod rpc_types;

pub use error::NodeError;
pub use mock::{BalanceScript, MockBalances, MockChannelSpec, MockNode, PaymentScript};
pub use payments::{
    ListPaymentsParams, ListPaymentsResult, PaymentInfo, PaymentStatus, SendPaymentParams,
    SessionRoute, SessionRouteNode,
};
pub use real::RealNode;
pub use rpc_types::{Channel, ChannelStateInfo, ListChannelsParams, NodeInfo, CHANNEL_READY};

/// What Even Keel needs from a Fiber node. Implemented by [`RealNode`] and
/// [`MockNode`]; everything above this trait is network-agnostic.
#[async_trait::async_trait]
pub trait FiberRpc: Send + Sync {
    /// `node_info`: identity and counts for the node we manage.
    async fn node_info(&self) -> Result<NodeInfo, NodeError>;

    /// `list_channels`: all channels with balances and TLC locks.
    async fn list_channels(&self, params: ListChannelsParams) -> Result<Vec<Channel>, NodeError>;

    /// `send_payment`: price (with `dry_run: true`) or execute a payment.
    /// The executor never calls this for real without a preceding dry run on
    /// the same params (rule 6).
    async fn send_payment(&self, params: SendPaymentParams) -> Result<PaymentInfo, NodeError>;

    /// `get_payment`: settlement tracking for one payment.
    async fn get_payment(&self, payment_hash: &str) -> Result<PaymentInfo, NodeError>;

    /// `list_payments`: recent payments — the §7 crash-recovery
    /// reconciliation source.
    async fn list_payments(&self, params: ListPaymentsParams)
        -> Result<ListPaymentsResult, NodeError>;
}
