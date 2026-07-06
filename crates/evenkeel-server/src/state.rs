//! Shared server state: the latest health picture, refreshed by the control
//! loop, read by the API. The dashboard never blocks on the node or the
//! database — during an outage it serves the last known picture with a
//! staleness flag (§7 degrade-to-read-only).

use std::sync::Arc;

use evenkeel_core::{BasisPoints, ChannelHealth};
use serde::Serialize;
use tokio::sync::RwLock;

/// One point of a sparkline: when, and what the usable ratio was.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryPoint {
    /// Snapshot time, ms since epoch.
    pub at_ms: u64,
    /// Usable ratio in basis points (`null` for zero capacity).
    pub usable_ratio_bp: Option<BasisPoints>,
}

/// Everything the dashboard shows for one channel.
///
/// Balances are decimal strings: they are `u128` Shannons and JSON numbers
/// (f64) cannot carry them. Formatting into CKB happens client-side,
/// display-only (ADR-7).
#[derive(Debug, Clone, Serialize)]
pub struct ChannelView {
    /// Channel ID.
    pub channel_id: String,
    /// Counterparty pubkey.
    pub peer: String,
    /// `"ckb"` or `"udt:…"`.
    pub asset: String,
    /// Health verdict from the core engine.
    pub health: ChannelHealth,
    /// Local balance, Shannons as decimal string.
    pub local_balance: String,
    /// Remote balance, Shannons as decimal string.
    pub remote_balance: String,
    /// Usable outbound, Shannons as decimal string.
    pub usable_out: String,
    /// Usable inbound, Shannons as decimal string.
    pub usable_in: String,
    /// Recent usable-ratio history for the drift sparkline, time-ascending.
    pub history: Vec<HistoryPoint>,
}

/// Node-level status for the header + staleness banner.
#[derive(Debug, Clone, Serialize, Default)]
pub struct NodeStatus {
    /// Managed node's pubkey (empty until first successful poll).
    pub node_pubkey: String,
    /// Node software version.
    pub node_version: String,
    /// Whether the last poll succeeded.
    pub rpc_up: bool,
    /// Newest snapshot time, ms since epoch (`null` before first poll).
    pub last_snapshot_ms: Option<u64>,
    /// True when the newest snapshot is older than the configured limit —
    /// the dashboard shows the staleness banner and the data is untrusted.
    pub stale: bool,
}

/// One rebalance action for the dashboard: the proposal card (state
/// `priced`) and the action log. Money fields are decimal strings (ADR-7).
#[derive(Debug, Clone, Serialize)]
pub struct ActionView {
    /// Correlation ID; the approve/reject API key.
    pub intent_id: String,
    /// `"ckb"` or `"udt:…"`.
    pub asset: String,
    /// Channel funds leave.
    pub source_channel: String,
    /// Channel funds arrive.
    pub sink_channel: String,
    /// Amount, Shannons as decimal string.
    pub amount: String,
    /// Planner benefit in bp.
    pub benefit_bp: u64,
    /// Executor state (lowercase, §6 names).
    pub state: String,
    /// `advisory` or `autopilot`.
    pub mode: String,
    /// Dry-run quote, Shannons as decimal string.
    pub quoted_fee: Option<String>,
    /// Settled actual fee, Shannons as decimal string.
    pub actual_fee: Option<String>,
    /// Payment hash once known.
    pub payment_hash: Option<String>,
    /// Rejection/failure detail.
    pub reason: Option<String>,
    /// Created, ms since epoch.
    pub created_at_ms: u64,
    /// Last transition, ms since epoch.
    pub updated_at_ms: u64,
}

/// Daily fee ledger summary for the header.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LedgerView {
    /// Spent today (settled actual fees), Shannons as decimal string.
    pub spent_today: String,
    /// Daily budget, Shannons as decimal string.
    pub daily_budget: String,
}

/// The whole dashboard payload, swapped atomically each tick.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Dashboard {
    /// Node status.
    pub status: NodeStatus,
    /// Per-channel views, stable order (by channel id).
    pub channels: Vec<ChannelView>,
    /// Recent rebalance actions, newest first (proposals + log).
    pub actions: Vec<ActionView>,
    /// Fee budget picture.
    pub ledger: LedgerView,
}

/// Shared handle the loop writes and the API reads.
pub type SharedDashboard = Arc<RwLock<Dashboard>>;
