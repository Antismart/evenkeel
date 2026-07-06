//! Persistence for Even Keel: the channel snapshot time-series (Phase 1) and,
//! in later phases, the rebalance action log, policy, and daily fee ledger.
//!
//! PostgreSQL via sqlx with compile-time-checked queries. `u128` Shannons are
//! stored as `NUMERIC(39,0)` and cross the driver boundary as decimal strings
//! (`$n::text::numeric` in, `::text` out) so no float or 64-bit truncation
//! ever touches a balance (ADR-7).

pub mod actions;

pub use actions::{ActionRecord, ActionState, TransitionPatch};

use evenkeel_core::{Asset, ChannelSnapshot, Shannons};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Errors from the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Database/driver failure.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// Migration failure at startup.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A stored value didn't parse back into its domain type — data written
    /// by a different (buggy or newer) version. Surfaced, never guessed at.
    #[error("corrupt row: {0}")]
    Corrupt(String),
    /// A state transition found the action in an unexpected state and
    /// refused to apply — the state machine's integrity guard (§6).
    #[error("transition to {to} refused for {intent_id}: unexpected current state")]
    TransitionRefused {
        /// The action whose transition was refused.
        intent_id: String,
        /// The state the caller tried to move to.
        to: &'static str,
    },
}

/// Handle to the Even Keel database.
#[derive(Debug, Clone)]
pub struct Store {
    pub(crate) pool: PgPool,
}

/// Encode an asset for the `asset` column: `ckb` or `udt:<script-json>`.
fn asset_to_db(asset: &Asset) -> String {
    match asset {
        Asset::Ckb => "ckb".to_string(),
        Asset::Udt(script) => format!("udt:{script}"),
    }
}

/// Decode the `asset` column.
fn asset_from_db(s: &str) -> Asset {
    match s.strip_prefix("udt:") {
        Some(script) => Asset::Udt(script.to_string()),
        None => Asset::Ckb,
    }
}

fn parse_shannons(field: &str, s: &str) -> Result<Shannons, StoreError> {
    s.parse::<u128>()
        .map_err(|e| StoreError::Corrupt(format!("{field}={s:?}: {e}")))
}

impl Store {
    /// Connect and run pending migrations. The pool is small: the writer is
    /// one serialized control loop, readers are a dashboard.
    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        sqlx::migrate!("../../migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Persist one poll tick's snapshots atomically (one transaction — a tick
    /// is all-or-nothing so drift windows never see a half-written tick).
    pub async fn insert_snapshots(
        &self,
        node_id: &str,
        snapshots: &[ChannelSnapshot],
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        for s in snapshots {
            sqlx::query!(
                r#"
                INSERT INTO channel_snapshots
                    (node_id, channel_id, peer, asset,
                     local_balance, remote_balance,
                     offered_tlc_balance, received_tlc_balance,
                     usable_out, usable_in, usable_ratio_bp, ready, at_ms)
                VALUES ($1, $2, $3, $4,
                        $5::text::numeric, $6::text::numeric,
                        $7::text::numeric, $8::text::numeric,
                        $9::text::numeric, $10::text::numeric,
                        $11, $12, $13)
                "#,
                node_id,
                s.channel_id,
                s.peer,
                asset_to_db(&s.asset),
                s.local_balance.to_string(),
                s.remote_balance.to_string(),
                s.offered_tlc_balance.to_string(),
                s.received_tlc_balance.to_string(),
                s.usable_out().to_string(),
                s.usable_in().to_string(),
                s.usable_ratio_bp().map(|bp| bp as i16),
                s.ready,
                s.at_ms as i64,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// All snapshots for a node since `since_ms`, ordered by channel then
    /// time ascending — exactly the window shape `evenkeel_core::classify`
    /// consumes after grouping by channel.
    pub async fn window_since(
        &self,
        node_id: &str,
        since_ms: u64,
    ) -> Result<Vec<ChannelSnapshot>, StoreError> {
        let rows = sqlx::query!(
            r#"
            SELECT channel_id, peer, asset,
                   local_balance::text        AS "local_balance!",
                   remote_balance::text       AS "remote_balance!",
                   offered_tlc_balance::text  AS "offered_tlc_balance!",
                   received_tlc_balance::text AS "received_tlc_balance!",
                   ready, at_ms
            FROM channel_snapshots
            WHERE node_id = $1 AND at_ms >= $2
            ORDER BY channel_id, at_ms ASC
            "#,
            node_id,
            since_ms as i64,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(ChannelSnapshot {
                    channel_id: r.channel_id,
                    peer: r.peer,
                    asset: asset_from_db(&r.asset),
                    local_balance: parse_shannons("local_balance", &r.local_balance)?,
                    remote_balance: parse_shannons("remote_balance", &r.remote_balance)?,
                    offered_tlc_balance: parse_shannons(
                        "offered_tlc_balance",
                        &r.offered_tlc_balance,
                    )?,
                    received_tlc_balance: parse_shannons(
                        "received_tlc_balance",
                        &r.received_tlc_balance,
                    )?,
                    ready: r.ready,
                    at_ms: r.at_ms as u64,
                })
            })
            .collect()
    }

    /// Timestamp of the newest snapshot for a node, for staleness banners and
    /// the `evenkeel_snapshot_age_seconds` gauge. `None` before the first poll.
    pub async fn latest_at_ms(&self, node_id: &str) -> Result<Option<u64>, StoreError> {
        let row = sqlx::query!(
            r#"SELECT MAX(at_ms) AS "max_at_ms" FROM channel_snapshots WHERE node_id = $1"#,
            node_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.max_at_ms.map(|v| v as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_encoding_round_trips() {
        for asset in [Asset::Ckb, Asset::Udt("{\"code_hash\":\"0xaa\"}".into())] {
            assert_eq!(asset_from_db(&asset_to_db(&asset)), asset);
        }
    }
}
