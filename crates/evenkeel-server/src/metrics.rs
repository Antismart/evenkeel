//! Prometheus metrics (§10) — the Phase 1 subset. The tool that watches
//! channels must itself be watchable.
//!
//! Gauge values are `f64` because that's the Prometheus wire type; these are
//! observability projections, never decision inputs (ADR-7 keeps floats
//! display-only, and this is display).

use prometheus::{GaugeVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry, TextEncoder};

/// All §10 gauges/counters plus their registry.
#[derive(Debug)]
pub struct Metrics {
    registry: Registry,
    /// `evenkeel_channel_usable_ratio{channel,asset}` — 0.0–1.0.
    pub usable_ratio: GaugeVec,
    /// `evenkeel_channels_by_state{state}` — channel count per health class.
    pub channels_by_state: GaugeVec,
    /// `evenkeel_drift_slope{channel}` — basis points per hour.
    pub drift_slope: GaugeVec,
    /// `evenkeel_rpc_up` — 1 when the last poll succeeded.
    pub rpc_up: IntGauge,
    /// `evenkeel_snapshot_age_seconds` — age of the newest snapshot.
    pub snapshot_age_seconds: IntGauge,
    /// `evenkeel_rebalance_actions_total{result,mode}` — terminal outcomes.
    pub actions_total: IntCounterVec,
    /// `evenkeel_rebalance_fee_shannons_total` — settled actual fees.
    pub fee_total: IntCounter,
    /// `evenkeel_fee_budget_remaining_shannons` — today's remaining budget.
    pub budget_remaining: IntGauge,
    /// `evenkeel_action_state{state}` — non-terminal action visibility.
    pub action_state: GaugeVec,
}

impl Metrics {
    /// Build and register all gauges.
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let usable_ratio = GaugeVec::new(
            Opts::new("evenkeel_channel_usable_ratio", "Usable outbound share of capacity (0-1)"),
            &["channel", "asset"],
        )?;
        let channels_by_state = GaugeVec::new(
            Opts::new("evenkeel_channels_by_state", "Channels per health class"),
            &["state"],
        )?;
        let drift_slope = GaugeVec::new(
            Opts::new("evenkeel_drift_slope", "Usable-ratio drift in basis points per hour"),
            &["channel"],
        )?;
        let rpc_up = IntGauge::new("evenkeel_rpc_up", "1 when the last FNN poll succeeded")?;
        let snapshot_age_seconds =
            IntGauge::new("evenkeel_snapshot_age_seconds", "Age of the newest snapshot")?;
        let actions_total = IntCounterVec::new(
            Opts::new("evenkeel_rebalance_actions_total", "Rebalance action outcomes"),
            &["result", "mode"],
        )?;
        let fee_total = IntCounter::new(
            "evenkeel_rebalance_fee_shannons_total",
            "Total settled rebalance fees (actual, Shannons)",
        )?;
        let budget_remaining = IntGauge::new(
            "evenkeel_fee_budget_remaining_shannons",
            "Remaining daily fee budget (Shannons)",
        )?;
        let action_state = GaugeVec::new(
            Opts::new("evenkeel_action_state", "Non-terminal actions by state"),
            &["state"],
        )?;

        registry.register(Box::new(usable_ratio.clone()))?;
        registry.register(Box::new(channels_by_state.clone()))?;
        registry.register(Box::new(drift_slope.clone()))?;
        registry.register(Box::new(rpc_up.clone()))?;
        registry.register(Box::new(snapshot_age_seconds.clone()))?;
        registry.register(Box::new(actions_total.clone()))?;
        registry.register(Box::new(fee_total.clone()))?;
        registry.register(Box::new(budget_remaining.clone()))?;
        registry.register(Box::new(action_state.clone()))?;

        Ok(Self {
            registry,
            usable_ratio,
            channels_by_state,
            drift_slope,
            rpc_up,
            snapshot_age_seconds,
            actions_total,
            fee_total,
            budget_remaining,
            action_state,
        })
    }

    /// Count a terminal action outcome.
    pub fn observe_action(&self, result: &str, mode: &str) {
        self.actions_total.with_label_values(&[result, mode]).inc();
    }

    /// Add a settled actual fee to the running total. Saturates at u64 —
    /// far beyond any plausible cumulative fee, and metrics are display-only.
    pub fn add_fee(&self, fee: u128) {
        self.fee_total.inc_by(fee.min(u64::MAX as u128) as u64);
    }

    /// Publish the set of non-terminal action states (one gauge point per
    /// state currently occupied).
    pub fn set_action_states<'a>(&self, states: impl Iterator<Item = &'a str>) {
        self.action_state.reset();
        for s in states {
            self.action_state.with_label_values(&[s]).inc();
        }
    }

    /// Render the registry in Prometheus text exposition format.
    pub fn render(&self) -> String {
        // Encoding into a String cannot fail for well-formed metrics; fall
        // back to empty output rather than panicking the scrape endpoint.
        TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            .unwrap_or_default()
    }
}
