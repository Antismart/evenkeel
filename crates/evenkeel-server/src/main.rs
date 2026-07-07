//! Even Keel server: Axum REST API for the dashboard, Prometheus `/metrics`,
//! and the single serialized control loop (poll → classify → store → plan →
//! execute-at-most-one → log).

use std::sync::Arc;

use evenkeel_node::{FiberRpc, MockNode, RealNode};
use evenkeel_server::api::{router, ApiState};
use evenkeel_server::config::{Config, NodeMode};
use evenkeel_server::executor::{Approvals, SharedPolicy};
use evenkeel_server::metrics::Metrics;
use evenkeel_server::state::SharedDashboard;
use evenkeel_server::control;
use evenkeel_store::Store;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env();
    info!(?config.node_mode, bind = %config.bind, "starting evenkeel-server");

    let node: Arc<dyn FiberRpc> = match config.node_mode {
        NodeMode::Mock => {
            info!("running against the MockNode demo scenario (ADR-6)");
            Arc::new(MockNode::demo())
        }
        NodeMode::Real => {
            info!(url = %config.fnn_url, "running against a real FNN");
            Arc::new(RealNode::new(config.fnn_url.clone())?)
        }
    };

    let store = Store::connect(&config.database_url).await?;
    let dashboard: SharedDashboard = SharedDashboard::default();
    let metrics = Arc::new(Metrics::new()?);
    let approvals: Approvals = Approvals::default();
    // Boot on defaults (advisory, autopilot OFF); the control loop replaces
    // this with the node's persisted policy once identity is known, and
    // PUT /api/policy edits it live.
    let policy: SharedPolicy = SharedPolicy::default();

    tokio::spawn(control::run(
        config.clone(),
        node,
        store.clone(),
        dashboard.clone(),
        metrics.clone(),
        approvals.clone(),
        policy.clone(),
    ));

    let app = router(ApiState { dashboard, metrics, approvals, policy, store });
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    info!(addr = %config.bind, "API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutting down");
        })
        .await?;
    Ok(())
}
