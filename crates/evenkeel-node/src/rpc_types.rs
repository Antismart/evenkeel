//! Typed structs mirroring the FNN v0.8.1 JSON-RPC surface Even Keel uses.
//!
//! Field names and encodings match `crates/fiber-lib/src/rpc/README.md` at
//! tag v0.8.1 exactly: integers are `0x`-hex strings on the wire, pubkeys and
//! hashes are hex strings. Unknown fields are ignored so patch releases don't
//! break the poller; fields FNN may omit are `Option` or defaulted.

use evenkeel_core::{Asset, ChannelSnapshot};
use serde::{Deserialize, Serialize};

use crate::hex::{u128_hex, u32_hex, u64_hex};

/// The channel state name FNN reports once a channel can route.
pub const CHANNEL_READY: &str = "ChannelReady";

/// Result of `node_info`.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeInfo {
    /// Node software version, e.g. `"0.8.1"`.
    pub version: String,
    /// Git commit of the node build.
    #[serde(default)]
    pub commit_hash: Option<String>,
    /// This node's identity public key (secp256k1 compressed hex, no 0x).
    pub pubkey: String,
    /// Optional operator-set name.
    #[serde(default)]
    pub node_name: Option<String>,
    /// Announced multiaddresses.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Number of open channels.
    #[serde(with = "u32_hex", default)]
    pub channel_count: u32,
    /// Number of connected peers.
    #[serde(with = "u32_hex", default)]
    pub peers_count: u32,
}

/// Channel state object as reported by `list_channels`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStateInfo {
    /// State machine name, e.g. `"ChannelReady"`, `"NegotiatingFunding"`.
    pub state_name: String,
    /// Sub-state flags; shape varies by state, kept opaque.
    #[serde(default)]
    pub state_flags: serde_json::Value,
}

/// One channel from `list_channels` (v0.8.1 `Channel` type, decision-relevant
/// subset plus identity fields).
#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    /// Channel ID (0x-hex hash).
    pub channel_id: String,
    /// Counterparty node public key.
    pub pubkey: String,
    /// Funding outpoint, present once funded.
    #[serde(default)]
    pub channel_outpoint: Option<String>,
    /// UDT type script if UDT-funded; `None` means native CKB.
    #[serde(default)]
    pub funding_udt_type_script: Option<serde_json::Value>,
    /// Current state.
    pub state: ChannelStateInfo,
    /// Our balance (Shannons, hex on the wire).
    #[serde(with = "u128_hex")]
    pub local_balance: u128,
    /// Balance locked in our pending outgoing TLCs.
    #[serde(with = "u128_hex")]
    pub offered_tlc_balance: u128,
    /// Counterparty balance.
    #[serde(with = "u128_hex")]
    pub remote_balance: u128,
    /// Balance locked in pending incoming TLCs.
    #[serde(with = "u128_hex")]
    pub received_tlc_balance: u128,
    /// Whether the channel is enabled for routing.
    #[serde(default)]
    pub enabled: bool,
    /// Creation time, ms since epoch (hex on the wire).
    #[serde(with = "u64_hex", default)]
    pub created_at: u64,
}

impl Channel {
    /// The asset this channel is denominated in (§5.4: planners partition by
    /// this; rebalances never cross assets).
    pub fn asset(&self) -> Asset {
        match &self.funding_udt_type_script {
            None => Asset::Ckb,
            // The script JSON itself is the identity — equal script, equal asset.
            Some(script) => Asset::Udt(script.to_string()),
        }
    }

    /// True when the channel is ready and enabled — the only channels the
    /// decision core looks at.
    pub fn is_ready(&self) -> bool {
        self.state.state_name == CHANNEL_READY && self.enabled
    }

    /// Project into the core's snapshot type, stamped with the poller's clock.
    pub fn to_snapshot(&self, at_ms: u64) -> ChannelSnapshot {
        ChannelSnapshot {
            channel_id: self.channel_id.clone(),
            peer: self.pubkey.clone(),
            asset: self.asset(),
            local_balance: self.local_balance,
            remote_balance: self.remote_balance,
            offered_tlc_balance: self.offered_tlc_balance,
            received_tlc_balance: self.received_tlc_balance,
            ready: self.is_ready(),
            at_ms,
        }
    }
}

/// Params for `list_channels`. All optional; an empty struct lists everything
/// open.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ListChannelsParams {
    /// Restrict to one counterparty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    /// Include closed channels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_closed: Option<bool>,
}

/// Result wrapper for `list_channels`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListChannelsResult {
    /// The channels.
    pub channels: Vec<Channel>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Verbatim shape from a live v0.8.1 `list_channels` response captured
    /// during the Phase 0 spike (docs/spike-notes.md).
    const LIVE_CHANNEL_JSON: &str = r#"{
        "channel_id": "0xe6a950b634e50ce07ff8a71d9b34e5f0ee1a0c40206b8f6ed46eb4a2be0e074c",
        "is_public": true,
        "channel_outpoint": "0xc104a5b4e353aed7c71fb2cc3e0ee563e1c82f1c10da97692266d5cf2a8bf1c700000000",
        "pubkey": "024714ca19abea4ddc0f3863ffdfb2e2cee76af87c477de4bc67c74a83f8140042",
        "funding_udt_type_script": null,
        "state": { "state_name": "ChannelReady", "state_flags": [] },
        "local_balance": "0x82c1f7f00",
        "offered_tlc_balance": "0x0",
        "remote_balance": "0x12a05f200",
        "received_tlc_balance": "0x0",
        "created_at": "0x197c9c2a41e",
        "enabled": true,
        "tlc_expiry_delta": "0x5265c00",
        "tlc_fee_proportional_millionths": "0x3e8"
    }"#;

    #[test]
    fn parses_live_v081_channel() {
        let ch: Channel = serde_json::from_str(LIVE_CHANNEL_JSON).unwrap();
        assert_eq!(ch.local_balance, 35_100_000_000);
        assert_eq!(ch.remote_balance, 5_000_000_000);
        assert_eq!(ch.asset(), Asset::Ckb);
        assert!(ch.is_ready());

        let snap = ch.to_snapshot(1_000);
        assert_eq!(snap.capacity(), 40_100_000_000);
        // 351 / 401 of capacity usable outbound → 8753 bp.
        assert_eq!(snap.usable_ratio_bp(), Some(8_753));
    }

    #[test]
    fn disabled_or_pending_channels_are_not_ready() {
        let mut v: serde_json::Value = serde_json::from_str(LIVE_CHANNEL_JSON).unwrap();
        v["state"]["state_name"] = "NegotiatingFunding".into();
        let ch: Channel = serde_json::from_value(v.clone()).unwrap();
        assert!(!ch.is_ready());

        v["state"]["state_name"] = "ChannelReady".into();
        v["enabled"] = false.into();
        let ch: Channel = serde_json::from_value(v).unwrap();
        assert!(!ch.is_ready());
    }

    #[test]
    fn udt_channels_get_distinct_asset() {
        let mut v: serde_json::Value = serde_json::from_str(LIVE_CHANNEL_JSON).unwrap();
        v["funding_udt_type_script"] = serde_json::json!({
            "code_hash": "0x1142755a044bf2ee358cba9f2da187ce928c91cd4dc8692ded0337efa677d21a",
            "hash_type": "type",
            "args": "0x878fcc6f1f08d48e87bb1c3b3d5083f23f8a39c5d5c764f253b55b998526439b"
        });
        let ch: Channel = serde_json::from_value(v).unwrap();
        assert_ne!(ch.asset(), Asset::Ckb);
    }

    #[test]
    fn parses_live_node_info() {
        // Shape from the spike's node_info call.
        let json = r#"{
            "version": "0.8.1",
            "commit_hash": "a1b2c3d",
            "pubkey": "0285605841146b278eb2f5ed817ceeb1810924a3fb1ca8f5ac4abe6fef43203cef",
            "node_name": null,
            "addresses": [],
            "chain_hash": "0x10639e0895502b5688a6be8cf69460d76541bfa4821629d86d62ba0aae3f9606",
            "channel_count": "0x3",
            "peers_count": "0x2"
        }"#;
        let info: NodeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "0.8.1");
        assert_eq!(info.channel_count, 3);
        assert_eq!(info.peers_count, 2);
    }
}
