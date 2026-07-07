//! Environment-driven configuration. Everything has a safe default except the
//! database. `EVENKEEL_NODE_MODE=real` is the default — Even Keel expects to
//! manage a live FNN; a missing node degrades to the §7 read-only picture,
//! never a crash. The scripted MockNode demo (ADR-6) stays one env var away
//! (`EVENKEEL_NODE_MODE=mock`) for token-free runs, CI, and the simulation.

use std::time::Duration;

/// Which `FiberRpc` implementation to run against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeMode {
    /// Scripted MockNode demo scenario.
    Mock,
    /// Real FNN at `fnn_url`.
    Real,
}

/// Server configuration, read once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Mock or real node.
    pub node_mode: NodeMode,
    /// FNN JSON-RPC endpoint (real mode).
    pub fnn_url: String,
    /// Postgres connection string.
    pub database_url: String,
    /// Address the API + metrics bind to.
    pub bind: String,
    /// Poll cadence.
    pub poll_interval: Duration,
    /// How far back the drift window reaches.
    pub drift_window: Duration,
    /// Snapshots older than this trip the staleness banner and stop being
    /// trusted (§7 degrade-to-read-only).
    pub stale_after: Duration,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_secs(key: &str, default: u64) -> Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    Duration::from_secs(secs)
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Self {
        let node_mode = match env_or("EVENKEEL_NODE_MODE", "real").to_lowercase().as_str() {
            "mock" => NodeMode::Mock,
            _ => NodeMode::Real,
        };
        Self {
            node_mode,
            fnn_url: env_or("EVENKEEL_FNN_URL", "http://127.0.0.1:8227"),
            database_url: env_or(
                "DATABASE_URL",
                "postgres://evenkeel:evenkeel@127.0.0.1:5433/evenkeel",
            ),
            bind: env_or("EVENKEEL_BIND", "127.0.0.1:3030"),
            poll_interval: env_secs("EVENKEEL_POLL_INTERVAL_SECS", 30),
            drift_window: env_secs("EVENKEEL_DRIFT_WINDOW_SECS", 3_600),
            stale_after: env_secs("EVENKEEL_STALE_AFTER_SECS", 180),
        }
    }
}
