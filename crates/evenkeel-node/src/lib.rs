//! The `FiberRpc` trait and its two implementations.
//!
//! - `RealNode`: reqwest against a Fiber Network Node's JSON-RPC (v0.8.x).
//! - `MockNode`: scripted balance scenarios with deterministic fees and fault
//!   injection. A first-class artifact (ADR-6): the dev environment, the CI
//!   environment, and the demo fallback — not test scaffolding.
