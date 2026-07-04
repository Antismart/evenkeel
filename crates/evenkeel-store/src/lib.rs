//! Persistence for Even Keel: the channel snapshot time-series (Phase 1) and,
//! in later phases, the rebalance action log, policy, and daily fee ledger.
//!
//! PostgreSQL via sqlx with compile-time-checked queries; a SQLite feature
//! flag for single-binary operator installs arrives with the packaging work.
