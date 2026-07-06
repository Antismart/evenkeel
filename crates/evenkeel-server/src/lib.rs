//! Even Keel server internals, exposed as a library so the MockNode scenario
//! suite (integration tests) can drive the executor and control loop
//! directly. The `evenkeel-server` binary is a thin wrapper over this.

pub mod api;
pub mod config;
pub mod control;
pub mod executor;
pub mod metrics;
pub mod state;
