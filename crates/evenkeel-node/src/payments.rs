//! Payment-path wire types, mirroring FNN v0.8.1 exactly.
//!
//! `send_payment` with `dry_run: true` is how every action is priced before
//! commitment (rule 6); `get_payment` drives settlement tracking; and
//! `list_payments` is the crash-recovery reconciliation source (§7).

use serde::{Deserialize, Serialize};

use crate::hex::{u128_hex, u128_hex_opt, u64_hex, u64_hex_opt};

/// Payment lifecycle: `Created -> Inflight -> Success | Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentStatus {
    /// Session created, no HTLC dispatched yet.
    Created,
    /// First-hop TLC sent, awaiting settlement.
    Inflight,
    /// Settled; `fee` is final.
    Success,
    /// Terminated without settling; principal unmoved.
    Failed,
}

impl PaymentStatus {
    /// Whether the payment can no longer change state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Failed)
    }
}

/// Params for `send_payment`. Only the fields Even Keel uses; all optional on
/// the wire. The circular-rebalance recipe (per the FNN docs): `target_pubkey`
/// = own pubkey, `keysend: true`, `allow_self_payment: true`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SendPaymentParams {
    /// Payment target; own pubkey for rebalances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_pubkey: Option<String>,
    /// Amount in Shannons.
    #[serde(with = "u128_hex_opt", skip_serializing_if = "Option::is_none", default)]
    pub amount: Option<u128>,
    /// Node-side fee ceiling in Shannons — defense-in-depth under our ledger.
    #[serde(with = "u128_hex_opt", skip_serializing_if = "Option::is_none", default)]
    pub max_fee_amount: Option<u128>,
    /// Keysend (no invoice).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keysend: Option<bool>,
    /// UDT type script for UDT-denominated payments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udt_type_script: Option<serde_json::Value>,
    /// Allow a circular route back to ourselves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_self_payment: Option<bool>,
    /// Payment timeout in seconds.
    #[serde(with = "u64_hex_opt", skip_serializing_if = "Option::is_none", default)]
    pub timeout: Option<u64>,
    /// Price + routability check only; nothing moves. Every real send is
    /// preceded by the same params with this set (rule 6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

/// One hop in a recorded payment route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRouteNode {
    /// Node the hop goes through.
    pub pubkey: String,
    /// Amount carried on this hop (Shannons).
    #[serde(with = "u128_hex")]
    pub amount: u128,
    /// Channel used for the hop; shape kept opaque.
    #[serde(default)]
    pub channel_outpoint: serde_json::Value,
}

/// A payment's recorded route (one entry per MPP part).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRoute {
    /// Hops from us to the target.
    pub nodes: Vec<SessionRouteNode>,
}

/// Result of `send_payment` / `get_payment` / each `list_payments` entry
/// (FNN's `GetPaymentCommandResult`).
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentInfo {
    /// Payment hash — the settlement-tracking handle.
    pub payment_hash: String,
    /// Current status.
    pub status: PaymentStatus,
    /// Created at, ms since epoch.
    #[serde(with = "u64_hex")]
    pub created_at: u64,
    /// Last update, ms since epoch.
    #[serde(with = "u64_hex", default)]
    pub last_updated_at: u64,
    /// Failure reason when `Failed`.
    #[serde(default)]
    pub failed_error: Option<String>,
    /// Fee in Shannons: the quote on a dry run, the actual fee once settled.
    #[serde(with = "u128_hex")]
    pub fee: u128,
    /// Recorded route(s); used by reconciliation to recognize self-payments
    /// (last hop pubkey == ours) and their amounts (first hop amount).
    #[serde(default)]
    pub routers: Vec<SessionRoute>,
}

impl PaymentInfo {
    /// The amount the payment carries, from the first hop of the first route.
    /// `None` when routes are absent (defensive: never guess an amount).
    pub fn amount(&self) -> Option<u128> {
        Some(self.routers.first()?.nodes.first()?.amount)
    }

    /// True when the recorded route ends at `pubkey` — a self-payment for us.
    pub fn is_self_payment_to(&self, pubkey: &str) -> bool {
        self.routers
            .first()
            .and_then(|r| r.nodes.last())
            .is_some_and(|n| n.pubkey == pubkey)
    }
}

/// Params for `list_payments`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ListPaymentsParams {
    /// Filter by status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PaymentStatus>,
    /// Page size (node default 15).
    #[serde(with = "u64_hex_opt", skip_serializing_if = "Option::is_none", default)]
    pub limit: Option<u64>,
    /// Exclusive pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Result of `list_payments`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListPaymentsResult {
    /// The payments.
    pub payments: Vec<PaymentInfo>,
    /// Cursor for the next page, when more exist.
    #[serde(default)]
    pub last_cursor: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn send_params_serialize_like_the_spike() {
        // Shape must match the params proven against the live node in Phase 0.
        let p = SendPaymentParams {
            target_pubkey: Some("0285...cef".into()),
            amount: Some(10_000_000_000),
            max_fee_amount: Some(100_000_000),
            keysend: Some(true),
            allow_self_payment: Some(true),
            dry_run: Some(true),
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["amount"], "0x2540be400");
        assert_eq!(v["max_fee_amount"], "0x5f5e100");
        assert_eq!(v["keysend"], true);
        assert!(v.get("udt_type_script").is_none(), "None fields must be absent");
        assert!(v.get("timeout").is_none());
    }

    #[test]
    fn parses_live_payment_result() {
        // Shaped from the Phase 0 settled rebalance (spike-notes).
        let json = r#"{
            "payment_hash": "0x5888472682b1b6dbc954c625ae44dd2ff4d1a57a64985d6b3832352d3684cc54",
            "status": "Success",
            "created_at": "0x197ca2f0000",
            "last_updated_at": "0x197ca2f2000",
            "failed_error": null,
            "fee": "0x989680",
            "routers": [{ "nodes": [
                { "pubkey": "024714ca", "amount": "0x2540be400", "channel_outpoint": "0x00d5f309" },
                { "pubkey": "0285605841", "amount": "0x2540be400", "channel_outpoint": "0xc104a5b4" }
            ]}]
        }"#;
        let p: PaymentInfo = serde_json::from_str(json).unwrap();
        assert_eq!(p.status, PaymentStatus::Success);
        assert!(p.status.is_terminal());
        assert_eq!(p.fee, 10_000_000);
        assert_eq!(p.amount(), Some(10_000_000_000));
        assert!(p.is_self_payment_to("0285605841"));
        assert!(!p.is_self_payment_to("024714ca"));
    }
}
