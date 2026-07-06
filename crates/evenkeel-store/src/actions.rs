//! Rebalance action log and fee ledger (§6).
//!
//! Every transition is guarded by the expected source state(s) in SQL
//! (`WHERE state = ANY(...)`), so an illegal transition updates zero rows and
//! surfaces as `TransitionRefused` instead of silently corrupting the state
//! machine. Terminal fees come from `get_payment` (`actual_fee`), never the
//! dry-run quote — that is what the daily ledger aggregates.

use evenkeel_core::{Asset, Shannons};
use serde::{Deserialize, Serialize};

use crate::{Store, StoreError};

/// Executor states (§6). Stored as lowercase text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionState {
    /// Intent emitted by the planner; not yet priced.
    Planned,
    /// Dry run succeeded; fee quoted; awaiting approval (advisory).
    Priced,
    /// Approved; row (re)written before the send RPC fires.
    Submitting,
    /// Send accepted; polling `get_payment`.
    Confirming,
    /// Payment succeeded; `actual_fee` recorded (terminal).
    Settled,
    /// Payment failed; principal unmoved (terminal).
    Failed,
    /// Not executed: policy said no, price went stale, or operator declined
    /// (terminal).
    Rejected,
    /// In flight past the confirmation horizon: blocks all new actions until
    /// reconciled (non-terminal on purpose).
    Stuck,
    /// §7 crash-window orphan we could not confidently match (terminal,
    /// alert-worthy).
    OrphanSuspect,
}

impl ActionState {
    /// Lowercase database representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Priced => "priced",
            Self::Submitting => "submitting",
            Self::Confirming => "confirming",
            Self::Settled => "settled",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Stuck => "stuck",
            Self::OrphanSuspect => "orphan_suspect",
        }
    }

    /// Parse the database representation.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => Self::Planned,
            "priced" => Self::Priced,
            "submitting" => Self::Submitting,
            "confirming" => Self::Confirming,
            "settled" => Self::Settled,
            "failed" => Self::Failed,
            "rejected" => Self::Rejected,
            "stuck" => Self::Stuck,
            "orphan_suspect" => Self::OrphanSuspect,
            _ => return None,
        })
    }

    /// Whether the action can no longer change.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Settled | Self::Failed | Self::Rejected | Self::OrphanSuspect)
    }
}

/// One action row.
#[derive(Debug, Clone, Serialize)]
pub struct ActionRecord {
    /// Client-generated correlation ID.
    pub intent_id: String,
    /// Node the action belongs to.
    pub node_id: String,
    /// Asset of both channels.
    pub asset: Asset,
    /// Channel funds leave (S).
    pub source_channel: String,
    /// Channel funds arrive (D).
    pub sink_channel: String,
    /// Amount in Shannons.
    pub amount: Shannons,
    /// Planner's capacity-weighted benefit for this intent.
    pub benefit_bp: u64,
    /// Current state.
    pub state: ActionState,
    /// `advisory` (Phase 2) or `autopilot` (Phase 3).
    pub mode: String,
    /// Dry-run fee quote (Shannons).
    pub quoted_fee: Option<Shannons>,
    /// Actual settled fee from `get_payment` (Shannons).
    pub actual_fee: Option<Shannons>,
    /// Payment hash, once known.
    pub payment_hash: Option<String>,
    /// Rejection/failure/orphan detail.
    pub reason: Option<String>,
    /// Created, ms since epoch.
    pub created_at_ms: u64,
    /// Last transition, ms since epoch.
    pub updated_at_ms: u64,
}

fn asset_to_db(asset: &Asset) -> String {
    match asset {
        Asset::Ckb => "ckb".to_string(),
        Asset::Udt(script) => format!("udt:{script}"),
    }
}

fn asset_from_db(s: &str) -> Asset {
    match s.strip_prefix("udt:") {
        Some(script) => Asset::Udt(script.to_string()),
        None => Asset::Ckb,
    }
}

fn parse_opt_shannons(field: &str, v: Option<String>) -> Result<Option<Shannons>, StoreError> {
    v.map(|s| {
        s.parse::<u128>()
            .map_err(|e| StoreError::Corrupt(format!("{field}={s:?}: {e}")))
    })
    .transpose()
}

/// Fields a transition may set alongside the new state.
#[derive(Debug, Default, Clone)]
pub struct TransitionPatch {
    /// Set the dry-run quote.
    pub quoted_fee: Option<Shannons>,
    /// Set the settled fee.
    pub actual_fee: Option<Shannons>,
    /// Set the payment hash.
    pub payment_hash: Option<String>,
    /// Set the reason.
    pub reason: Option<String>,
}

impl Store {
    /// Insert a fresh action row (state `Planned` or `Priced`).
    pub async fn insert_action(&self, a: &ActionRecord) -> Result<(), StoreError> {
        sqlx::query!(
            r#"
            INSERT INTO rebalance_actions
                (intent_id, node_id, asset, source_channel, sink_channel,
                 amount, benefit_bp, state, mode, quoted_fee,
                 created_at_ms, updated_at_ms)
            VALUES ($1, $2, $3, $4, $5,
                    $6::text::numeric, $7, $8, $9, $10::text::numeric,
                    $11, $12)
            "#,
            a.intent_id,
            a.node_id,
            asset_to_db(&a.asset),
            a.source_channel,
            a.sink_channel,
            a.amount.to_string(),
            a.benefit_bp as i64,
            a.state.as_str(),
            a.mode,
            a.quoted_fee.map(|f| f.to_string()),
            a.created_at_ms as i64,
            a.updated_at_ms as i64,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Move an action `from` one of the expected states `to` a new one,
    /// applying `patch`. Refuses (without touching the row) when the action
    /// is not in an expected state — the state machine's integrity guard.
    pub async fn transition_action(
        &self,
        intent_id: &str,
        from: &[ActionState],
        to: ActionState,
        patch: TransitionPatch,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let from_states: Vec<String> = from.iter().map(|s| s.as_str().to_string()).collect();
        let result = sqlx::query!(
            r#"
            UPDATE rebalance_actions
            SET state = $2,
                quoted_fee   = COALESCE($3::text::numeric, quoted_fee),
                actual_fee   = COALESCE($4::text::numeric, actual_fee),
                payment_hash = COALESCE($5, payment_hash),
                reason       = COALESCE($6, reason),
                updated_at_ms = $7
            WHERE intent_id = $1 AND state = ANY($8)
            "#,
            intent_id,
            to.as_str(),
            patch.quoted_fee.map(|f| f.to_string()),
            patch.actual_fee.map(|f| f.to_string()),
            patch.payment_hash,
            patch.reason,
            now_ms as i64,
            &from_states,
        )
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::TransitionRefused {
                intent_id: intent_id.to_string(),
                to: to.as_str(),
            });
        }
        Ok(())
    }

    /// All actions not in a terminal state — the startup reconciliation set
    /// and the executor's one-in-flight guard.
    pub async fn non_terminal_actions(
        &self,
        node_id: &str,
    ) -> Result<Vec<ActionRecord>, StoreError> {
        self.fetch_actions(
            node_id,
            "state IN ('planned','priced','submitting','confirming','stuck')",
            1_000,
        )
        .await
    }

    /// Most recent actions for the dashboard log, newest first.
    pub async fn recent_actions(
        &self,
        node_id: &str,
        limit: i64,
    ) -> Result<Vec<ActionRecord>, StoreError> {
        self.fetch_actions(node_id, "TRUE", limit).await
    }

    /// Sum of settled `actual_fee` in `[from_ms, to_ms)` — the fee ledger.
    pub async fn fee_spent_between(
        &self,
        node_id: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Shannons, StoreError> {
        let row = sqlx::query!(
            r#"
            SELECT COALESCE(SUM(actual_fee), 0)::text AS "total!"
            FROM rebalance_actions
            WHERE node_id = $1 AND state = 'settled'
              AND updated_at_ms >= $2 AND updated_at_ms < $3
            "#,
            node_id,
            from_ms as i64,
            to_ms as i64,
        )
        .fetch_one(&self.pool)
        .await?;
        row.total
            .parse::<u128>()
            .map_err(|e| StoreError::Corrupt(format!("fee sum {:?}: {e}", row.total)))
    }

    // Shared row-mapping for the two fetch shapes. The state filter is a
    // trusted constant from this module, never caller input.
    async fn fetch_actions(
        &self,
        node_id: &str,
        state_filter: &str,
        limit: i64,
    ) -> Result<Vec<ActionRecord>, StoreError> {
        let sql = format!(
            r#"
            SELECT intent_id, node_id, asset, source_channel, sink_channel,
                   amount::text AS amount, benefit_bp, state, mode,
                   quoted_fee::text AS quoted_fee, actual_fee::text AS actual_fee,
                   payment_hash, reason, created_at_ms, updated_at_ms
            FROM rebalance_actions
            WHERE node_id = $1 AND {state_filter}
            ORDER BY created_at_ms DESC
            LIMIT $2
            "#
        );
        let rows = sqlx::query(&sql)
            .bind(node_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        use sqlx::Row;
        rows.into_iter()
            .map(|r| {
                let state_str: String = r.get("state");
                let state = ActionState::parse(&state_str).ok_or_else(|| {
                    StoreError::Corrupt(format!("unknown action state {state_str:?}"))
                })?;
                let amount_str: String = r.get("amount");
                Ok(ActionRecord {
                    intent_id: r.get("intent_id"),
                    node_id: r.get("node_id"),
                    asset: asset_from_db(&r.get::<String, _>("asset")),
                    source_channel: r.get("source_channel"),
                    sink_channel: r.get("sink_channel"),
                    amount: amount_str.parse::<u128>().map_err(|e| {
                        StoreError::Corrupt(format!("amount={amount_str:?}: {e}"))
                    })?,
                    benefit_bp: r.get::<i64, _>("benefit_bp") as u64,
                    state,
                    mode: r.get("mode"),
                    quoted_fee: parse_opt_shannons("quoted_fee", r.get("quoted_fee"))?,
                    actual_fee: parse_opt_shannons("actual_fee", r.get("actual_fee"))?,
                    payment_hash: r.get("payment_hash"),
                    reason: r.get("reason"),
                    created_at_ms: r.get::<i64, _>("created_at_ms") as u64,
                    updated_at_ms: r.get::<i64, _>("updated_at_ms") as u64,
                })
            })
            .collect()
    }
}
