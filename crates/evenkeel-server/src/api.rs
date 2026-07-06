//! HTTP surface: the dashboard REST API and Prometheus `/metrics`.
//!
//! Read-only in Phase 1. Reads come from the in-memory dashboard cache the
//! control loop maintains — an FNN or database outage never turns into a
//! dashboard outage (§7).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tracing::info;

use crate::executor::Approvals;
use crate::metrics::Metrics;
use crate::state::SharedDashboard;

/// Shared handles for request handlers.
#[derive(Clone)]
pub struct ApiState {
    /// Latest health picture, maintained by the control loop.
    pub dashboard: SharedDashboard,
    /// Prometheus registry.
    pub metrics: Arc<Metrics>,
    /// Operator decisions consumed by the executor on its next tick.
    pub approvals: Approvals,
}

/// Build the router: `/api/channels`, `/api/status`, the advisory approval
/// endpoints, `/metrics`, `/healthz`.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/channels", get(channels))
        .route("/api/status", get(status))
        .route("/api/actions/{intent_id}/approve", post(approve))
        .route("/api/actions/{intent_id}/reject", post(reject))
        .route("/metrics", get(metrics))
        .route("/healthz", get(|| async { StatusCode::OK }))
        .with_state(state)
}

/// Operator approves a PRICED action; the executor executes it next tick
/// (after re-checking budget and channel state — approval is necessary, not
/// sufficient).
async fn approve(State(s): State<ApiState>, Path(intent_id): Path<String>) -> impl IntoResponse {
    record_decision(&s, intent_id, true)
}

/// Operator declines a PRICED action; it terminates as `rejected`.
async fn reject(State(s): State<ApiState>, Path(intent_id): Path<String>) -> impl IntoResponse {
    record_decision(&s, intent_id, false)
}

fn record_decision(s: &ApiState, intent_id: String, approve: bool) -> StatusCode {
    info!(intent_id, approve, "operator decision received");
    let mut approvals = s.approvals.lock().unwrap_or_else(|e| e.into_inner());
    approvals.insert(intent_id, approve);
    StatusCode::ACCEPTED
}

/// Full dashboard payload: node status + per-channel views with history.
async fn channels(State(s): State<ApiState>) -> impl IntoResponse {
    let d = s.dashboard.read().await;
    Json(d.clone())
}

/// Node status alone (cheap poll for the staleness banner).
async fn status(State(s): State<ApiState>) -> impl IntoResponse {
    let d = s.dashboard.read().await;
    Json(d.status.clone())
}

/// Prometheus text exposition.
async fn metrics(State(s): State<ApiState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        s.metrics.render(),
    )
}
