//! HTTP surface: the dashboard REST API and Prometheus `/metrics`.
//!
//! Read-only in Phase 1. Reads come from the in-memory dashboard cache the
//! control loop maintains — an FNN or database outage never turns into a
//! dashboard outage (§7).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};

use crate::metrics::Metrics;
use crate::state::SharedDashboard;

/// Shared handles for request handlers.
#[derive(Clone)]
pub struct ApiState {
    /// Latest health picture, maintained by the control loop.
    pub dashboard: SharedDashboard,
    /// Prometheus registry.
    pub metrics: Arc<Metrics>,
}

/// Build the router: `/api/channels`, `/api/status`, `/metrics`, `/healthz`.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/channels", get(channels))
        .route("/api/status", get(status))
        .route("/metrics", get(metrics))
        .route("/healthz", get(|| async { StatusCode::OK }))
        .with_state(state)
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
