//! Prometheus metrics (§10) — the Phase 1 subset. The tool that watches
//! channels must itself be watchable.
//!
//! Gauge values are `f64` because that's the Prometheus wire type; these are
//! observability projections, never decision inputs (ADR-7 keeps floats
//! display-only, and this is display).

use prometheus::{GaugeVec, IntGauge, Opts, Registry, TextEncoder};

/// All Phase 1 gauges plus their registry.
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

        registry.register(Box::new(usable_ratio.clone()))?;
        registry.register(Box::new(channels_by_state.clone()))?;
        registry.register(Box::new(drift_slope.clone()))?;
        registry.register(Box::new(rpc_up.clone()))?;
        registry.register(Box::new(snapshot_age_seconds.clone()))?;

        Ok(Self { registry, usable_ratio, channels_by_state, drift_slope, rpc_up, snapshot_age_seconds })
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
