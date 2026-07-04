//! Error type for node RPC operations.

/// Everything that can go wrong talking to a Fiber node.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The HTTP layer failed: connection refused, timeout, TLS, etc.
    /// The node may be down — callers degrade to read-only (§7).
    #[error("transport failure talking to FNN: {0}")]
    Transport(#[from] reqwest::Error),

    /// The node answered with a JSON-RPC error object.
    #[error("FNN RPC error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// Human-readable message from the node.
        message: String,
    },

    /// The node answered 200 but the body didn't match the expected shape —
    /// likely a version mismatch worth surfacing loudly.
    #[error("failed to decode FNN response for {method}: {source}")]
    Decode {
        /// Which RPC method's response failed to decode.
        method: &'static str,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// A fault injected by the MockNode (tests and scenario runs only).
    #[error("injected fault: {0}")]
    Injected(&'static str),
}
