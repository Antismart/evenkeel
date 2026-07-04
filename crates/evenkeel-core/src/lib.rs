//! Even Keel's pure decision core.
//!
//! Health classification, drift detection, and (Phase 2+) rebalance planning.
//! This crate performs no I/O: no tokio, no reqwest, no sqlx. It takes channel
//! snapshots in and emits classifications (later: intents) out, which is what
//! makes the whole decision path testable without a network (architecture §8).
//!
//! Money is `u128` Shannons everywhere; ratios in decision paths are integer
//! basis points, 0–10_000 (ADR-7). Floats are display-only and do not exist here.

pub mod health;
pub mod types;

pub use health::{classify, drift_bp_per_hour, ChannelHealth, HealthClass, HealthThresholds};
pub use types::{ratio_bp, Asset, BasisPoints, ChannelSnapshot, Shannons, BP_SCALE};
