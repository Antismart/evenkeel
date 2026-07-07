//! HTTP surface: the dashboard REST API and Prometheus `/metrics`.
//!
//! Reads come from the in-memory dashboard cache the control loop maintains —
//! an FNN or database outage never turns into a dashboard outage (§7). The
//! policy endpoints are the Phase 3 write surface: money crosses this API as
//! decimal strings only (ADR-7), and a policy PUT applies live via the shared
//! [`SharedPolicy`] handle the executor reads each tick.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use evenkeel_core::{HealthThresholds, Policy, Shannons};
use evenkeel_store::Store;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::executor::{Approvals, PolicyState, SharedPolicy};
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
    /// Live policy + autopilot flag, shared with the control loop/executor.
    pub policy: SharedPolicy,
    /// Store handle for persisting policy edits.
    pub store: Store,
}

/// Build the router: `/api/channels`, `/api/status`, `/api/policy`, the
/// advisory approval endpoints, `/metrics`, `/healthz`.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/channels", get(channels))
        .route("/api/status", get(status))
        .route("/api/policy", get(get_policy).put(put_policy))
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

/// Policy as it crosses the API: money is decimal strings (ADR-7), ratios are
/// integer basis points. One shape for GET response and PUT request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBody {
    /// Ratio channels are steered toward (bp, 0–10000).
    pub target_ratio_bp: u16,
    /// Per-action amount cap, Shannons as decimal string.
    pub max_amount_per_action: String,
    /// Per-action fee cap, Shannons as decimal string.
    pub max_fee_per_action: String,
    /// Daily fee budget, Shannons as decimal string.
    pub max_fee_per_day: String,
    /// Benefit floor: bp of imbalance reduction required per CKB of fee.
    pub min_benefit_bp_per_ckb_fee: u64,
    /// Pair cooldown in ticks.
    pub cooldown_ticks: u64,
    /// Depleted threshold (bp).
    pub depleted_below_bp: u16,
    /// Saturated threshold (bp).
    pub saturated_above_bp: u16,
    /// Drift classification threshold (bp/hour).
    pub drift_bp_per_hour: u32,
    /// Minimum window points before drift is trusted.
    pub min_drift_points: usize,
    /// Autopilot opt-in flag. Default OFF; flipping it ON means priced
    /// actions within budget execute without an operator click.
    pub autopilot: bool,
}

impl PolicyBody {
    fn from_state(ps: &PolicyState) -> Self {
        Self {
            target_ratio_bp: ps.policy.target_ratio_bp,
            max_amount_per_action: ps.policy.max_amount_per_action.to_string(),
            max_fee_per_action: ps.policy.max_fee_per_action.to_string(),
            max_fee_per_day: ps.policy.max_fee_per_day.to_string(),
            min_benefit_bp_per_ckb_fee: ps.policy.min_benefit_bp_per_ckb_fee,
            cooldown_ticks: ps.policy.cooldown_ticks,
            depleted_below_bp: ps.policy.thresholds.depleted_below_bp,
            saturated_above_bp: ps.policy.thresholds.saturated_above_bp,
            drift_bp_per_hour: ps.policy.thresholds.drift_bp_per_hour,
            min_drift_points: ps.policy.thresholds.min_drift_points,
            autopilot: ps.autopilot,
        }
    }

    /// Validate and convert into a live policy state. Errors name the field.
    fn into_state(self) -> Result<PolicyState, String> {
        fn money(field: &str, s: &str) -> Result<Shannons, String> {
            s.parse::<u128>().map_err(|e| format!("{field}: {e}"))
        }
        if self.target_ratio_bp > 10_000 {
            return Err("target_ratio_bp must be 0..=10000".into());
        }
        if self.depleted_below_bp > 10_000 || self.saturated_above_bp > 10_000 {
            return Err("thresholds must be 0..=10000 bp".into());
        }
        if self.depleted_below_bp >= self.saturated_above_bp {
            return Err("depleted_below_bp must be below saturated_above_bp".into());
        }
        if !(self.depleted_below_bp..=self.saturated_above_bp).contains(&self.target_ratio_bp) {
            return Err("target_ratio_bp must sit between the thresholds".into());
        }
        Ok(PolicyState {
            policy: Policy {
                target_ratio_bp: self.target_ratio_bp,
                max_amount_per_action: money("max_amount_per_action", &self.max_amount_per_action)?,
                max_fee_per_action: money("max_fee_per_action", &self.max_fee_per_action)?,
                max_fee_per_day: money("max_fee_per_day", &self.max_fee_per_day)?,
                min_benefit_bp_per_ckb_fee: self.min_benefit_bp_per_ckb_fee,
                cooldown_ticks: self.cooldown_ticks,
                thresholds: HealthThresholds {
                    depleted_below_bp: self.depleted_below_bp,
                    saturated_above_bp: self.saturated_above_bp,
                    drift_bp_per_hour: self.drift_bp_per_hour,
                    min_drift_points: self.min_drift_points,
                },
            },
            autopilot: self.autopilot,
        })
    }
}

/// Current live policy + autopilot flag.
async fn get_policy(State(s): State<ApiState>) -> impl IntoResponse {
    let ps = s.policy.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Json(PolicyBody::from_state(&ps))
}

/// Replace the live policy: validate → persist → apply. The executor sees the
/// new bounds on its next tick; nothing about an in-flight action changes.
async fn put_policy(
    State(s): State<ApiState>,
    Json(body): Json<PolicyBody>,
) -> impl IntoResponse {
    let new_state = match body.into_state() {
        Ok(ps) => ps,
        Err(msg) => return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
    };
    // Policy rows are keyed by the managed node's pubkey, learned from the
    // first successful poll — before that there is nothing to key on.
    let node_id = s.dashboard.read().await.status.node_pubkey.clone();
    if node_id.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "node identity not known yet; retry shortly".to_string(),
        )
            .into_response();
    }
    let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    if let Err(e) = s.store.save_policy(&node_id, &new_state.policy, new_state.autopilot, now_ms).await
    {
        error!(error = %e, "failed to persist policy");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to persist policy".to_string())
            .into_response();
    }
    info!(autopilot = new_state.autopilot, "policy updated");
    let body = PolicyBody::from_state(&new_state);
    {
        let mut live = s.policy.lock().unwrap_or_else(|e| e.into_inner());
        *live = new_state;
    }
    Json(body).into_response()
}
