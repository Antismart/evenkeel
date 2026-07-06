//! Domain types shared across the decision core.
//!
//! Money is `u128` Shannons (1 CKB = 100_000_000 Shannons). Ratios in any
//! decision path are integer basis points 0–10_000 (ADR-7); the float never
//! appears here.

use serde::{Deserialize, Serialize};

/// Amount in Shannons, the smallest CKB unit (1 CKB = 10^8 Shannons).
///
/// A plain alias rather than a newtype: FNN's RPC, the store, and this crate
/// all speak the same unit, and `u128` end-to-end is the CLAUDE.md rule.
pub type Shannons = u128;

/// One basis point = 1/100th of a percent. Ratios are 0–10_000.
pub type BasisPoints = u16;

/// Number of basis points in the whole (100%).
pub const BP_SCALE: u128 = 10_000;

/// The asset a channel is denominated in. Rebalances never cross assets
/// (§5.4), so planners partition by this key before pairing channels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Asset {
    /// Native CKB.
    Ckb,
    /// A user-defined token, identified by the hash of its type script as a
    /// hex string (we never interpret the script here, only compare it).
    Udt(String),
}

/// A point-in-time observation of one channel, as captured by the poller.
///
/// Field semantics mirror FNN v0.8.x `list_channels` exactly; the usable-
/// liquidity derivations live on this type so every consumer applies the
/// same §5.1 formulas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    /// FNN channel ID (0x-prefixed hash).
    pub channel_id: String,
    /// Counterparty node public key.
    pub peer: String,
    /// Asset the channel is denominated in.
    pub asset: Asset,
    /// Our side's balance.
    pub local_balance: Shannons,
    /// Counterparty's balance.
    pub remote_balance: Shannons,
    /// Locked in pending outgoing TLCs (not spendable by us).
    pub offered_tlc_balance: Shannons,
    /// Locked in pending incoming TLCs (not receivable capacity).
    pub received_tlc_balance: Shannons,
    /// True when the channel is CHANNEL_READY and enabled — only such
    /// channels participate in decisions.
    pub ready: bool,
    /// Capture time, milliseconds since the UNIX epoch (from the poller's
    /// clock; the core never reads a clock itself).
    pub at_ms: u64,
}

impl ChannelSnapshot {
    /// What we can actually send: local balance net of pending outgoing TLCs
    /// (§5.1). Saturating: a snapshot mid-settlement can transiently report
    /// offered > local, which means "nothing usable", not a panic.
    pub fn usable_out(&self) -> Shannons {
        self.local_balance.saturating_sub(self.offered_tlc_balance)
    }

    /// What we can actually receive: remote balance net of pending incoming
    /// TLCs (§5.1).
    pub fn usable_in(&self) -> Shannons {
        self.remote_balance.saturating_sub(self.received_tlc_balance)
    }

    /// Total channel capacity for this asset.
    pub fn capacity(&self) -> Shannons {
        self.local_balance.saturating_add(self.remote_balance)
    }

    /// Usable outbound share of capacity in basis points — the number every
    /// decision runs on (§5.1). `None` for a zero-capacity channel.
    pub fn usable_ratio_bp(&self) -> Option<BasisPoints> {
        ratio_bp(self.usable_out(), self.capacity())
    }

    /// Raw local share of capacity in basis points. Display-oriented; kept
    /// integer so even display values have one source of truth.
    pub fn local_ratio_bp(&self) -> Option<BasisPoints> {
        ratio_bp(self.local_balance, self.capacity())
    }
}

/// `part / whole` in basis points, rounded down, `None` when `whole == 0`.
///
/// Overflow-safe for the full `u128` range: when `part` is too large to
/// multiply by 10_000, both operands are scaled down together first (the
/// ratio is preserved to well beyond basis-point precision).
pub fn ratio_bp(part: Shannons, whole: Shannons) -> Option<BasisPoints> {
    if whole == 0 {
        return None;
    }
    let (p, w) = if part > u128::MAX / BP_SCALE {
        // Scale both down; 2^16 keeps far more precision than bp resolution.
        (part >> 16, (whole >> 16).max(1))
    } else {
        (part, whole)
    };
    let bp = (p.saturating_mul(BP_SCALE) / w).min(BP_SCALE);
    // min(10_000) fits u16 by construction.
    Some(bp as BasisPoints)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn snap(local: u128, remote: u128, offered: u128, received: u128) -> ChannelSnapshot {
        ChannelSnapshot {
            channel_id: "0xabc".into(),
            peer: "02aa".into(),
            asset: Asset::Ckb,
            local_balance: local,
            remote_balance: remote,
            offered_tlc_balance: offered,
            received_tlc_balance: received,
            ready: true,
            at_ms: 0,
        }
    }

    #[test]
    fn usable_nets_out_tlcs() {
        let s = snap(1_000, 500, 200, 100);
        assert_eq!(s.usable_out(), 800);
        assert_eq!(s.usable_in(), 400);
        assert_eq!(s.capacity(), 1_500);
        // 800 / 1500 = 53.33% → 5333 bp
        assert_eq!(s.usable_ratio_bp(), Some(5333));
    }

    #[test]
    fn zero_capacity_has_no_ratio() {
        assert_eq!(snap(0, 0, 0, 0).usable_ratio_bp(), None);
    }

    #[test]
    fn tlc_exceeding_balance_saturates_to_zero() {
        let s = snap(100, 100, 150, 0);
        assert_eq!(s.usable_out(), 0);
        assert_eq!(s.usable_ratio_bp(), Some(0));
    }

    #[test]
    fn ratio_survives_extreme_values() {
        assert_eq!(ratio_bp(u128::MAX, u128::MAX), Some(10_000));
        assert_eq!(ratio_bp(u128::MAX / 2, u128::MAX), Some(4_999));
        assert_eq!(ratio_bp(1, u128::MAX), Some(0));
    }
}
