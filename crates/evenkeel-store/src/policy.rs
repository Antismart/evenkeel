//! Persisted per-node policy (Phase 3, ADR-4).
//!
//! One row per `node_id` carrying every [`Policy`] field plus the autopilot
//! opt-in flag. Money columns are `NUMERIC(39,0)` crossing the driver as
//! decimal strings (ADR-7); ratios are integer basis points. When no row
//! exists the server runs `Policy::default()` with autopilot OFF and writes
//! nothing until the operator's first explicit save.

use evenkeel_core::{HealthThresholds, Policy};

use crate::{parse_shannons, Store, StoreError};

fn corrupt_int(field: &str, v: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("{field}={v} out of domain range"))
}

impl Store {
    /// Load the persisted policy and autopilot flag for `node_id`, or `None`
    /// when the operator has never saved one (the caller then runs defaults).
    pub async fn load_policy(&self, node_id: &str) -> Result<Option<(Policy, bool)>, StoreError> {
        let row = sqlx::query!(
            r#"
            SELECT target_ratio_bp,
                   max_amount_per_action::text AS "max_amount_per_action!",
                   max_fee_per_action::text    AS "max_fee_per_action!",
                   max_fee_per_day::text       AS "max_fee_per_day!",
                   min_benefit_bp_per_ckb_fee, cooldown_ticks,
                   depleted_below_bp, saturated_above_bp,
                   drift_bp_per_hour, min_drift_points, autopilot
            FROM policy
            WHERE node_id = $1
            "#,
            node_id,
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else { return Ok(None) };
        let policy = Policy {
            target_ratio_bp: u16::try_from(r.target_ratio_bp)
                .map_err(|_| corrupt_int("target_ratio_bp", r.target_ratio_bp))?,
            max_amount_per_action: parse_shannons(
                "max_amount_per_action",
                &r.max_amount_per_action,
            )?,
            max_fee_per_action: parse_shannons("max_fee_per_action", &r.max_fee_per_action)?,
            max_fee_per_day: parse_shannons("max_fee_per_day", &r.max_fee_per_day)?,
            min_benefit_bp_per_ckb_fee: u64::try_from(r.min_benefit_bp_per_ckb_fee)
                .map_err(|_| corrupt_int("min_benefit_bp_per_ckb_fee", r.min_benefit_bp_per_ckb_fee))?,
            cooldown_ticks: u64::try_from(r.cooldown_ticks)
                .map_err(|_| corrupt_int("cooldown_ticks", r.cooldown_ticks))?,
            thresholds: HealthThresholds {
                depleted_below_bp: u16::try_from(r.depleted_below_bp)
                    .map_err(|_| corrupt_int("depleted_below_bp", r.depleted_below_bp))?,
                saturated_above_bp: u16::try_from(r.saturated_above_bp)
                    .map_err(|_| corrupt_int("saturated_above_bp", r.saturated_above_bp))?,
                drift_bp_per_hour: u32::try_from(r.drift_bp_per_hour)
                    .map_err(|_| corrupt_int("drift_bp_per_hour", r.drift_bp_per_hour))?,
                min_drift_points: usize::try_from(r.min_drift_points)
                    .map_err(|_| corrupt_int("min_drift_points", r.min_drift_points))?,
            },
        };
        Ok(Some((policy, r.autopilot)))
    }

    /// Upsert the policy row for `node_id`. Called only on explicit operator
    /// saves — running on defaults never writes a row implicitly.
    ///
    /// Integer fields saturate at their column's range (bp fields are
    /// validated to 0–10000 upstream, far inside `SMALLINT`).
    pub async fn save_policy(
        &self,
        node_id: &str,
        policy: &Policy,
        autopilot: bool,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        sqlx::query!(
            r#"
            INSERT INTO policy
                (node_id, target_ratio_bp,
                 max_amount_per_action, max_fee_per_action, max_fee_per_day,
                 min_benefit_bp_per_ckb_fee, cooldown_ticks,
                 depleted_below_bp, saturated_above_bp,
                 drift_bp_per_hour, min_drift_points,
                 autopilot, updated_at_ms)
            VALUES ($1, $2,
                    $3::text::numeric, $4::text::numeric, $5::text::numeric,
                    $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (node_id) DO UPDATE SET
                target_ratio_bp            = EXCLUDED.target_ratio_bp,
                max_amount_per_action      = EXCLUDED.max_amount_per_action,
                max_fee_per_action         = EXCLUDED.max_fee_per_action,
                max_fee_per_day            = EXCLUDED.max_fee_per_day,
                min_benefit_bp_per_ckb_fee = EXCLUDED.min_benefit_bp_per_ckb_fee,
                cooldown_ticks             = EXCLUDED.cooldown_ticks,
                depleted_below_bp          = EXCLUDED.depleted_below_bp,
                saturated_above_bp         = EXCLUDED.saturated_above_bp,
                drift_bp_per_hour          = EXCLUDED.drift_bp_per_hour,
                min_drift_points           = EXCLUDED.min_drift_points,
                autopilot                  = EXCLUDED.autopilot,
                updated_at_ms              = EXCLUDED.updated_at_ms
            "#,
            node_id,
            policy.target_ratio_bp.min(i16::MAX as u16) as i16,
            policy.max_amount_per_action.to_string(),
            policy.max_fee_per_action.to_string(),
            policy.max_fee_per_day.to_string(),
            policy.min_benefit_bp_per_ckb_fee.min(i64::MAX as u64) as i64,
            policy.cooldown_ticks.min(i64::MAX as u64) as i64,
            policy.thresholds.depleted_below_bp.min(i16::MAX as u16) as i16,
            policy.thresholds.saturated_above_bp.min(i16::MAX as u16) as i16,
            policy.thresholds.drift_bp_per_hour as i64,
            policy.thresholds.min_drift_points.min(i32::MAX as usize) as i32,
            autopilot,
            now_ms as i64,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
