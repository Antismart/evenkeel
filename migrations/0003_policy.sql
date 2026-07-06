-- Operator policy, persisted per node (architecture §5.3 bounds, ADR-4).
--
-- One row per node_id holding every evenkeel_core::Policy field plus the
-- autopilot opt-in flag (default OFF — ADR-4: a tool that spends money by
-- default is wrong for first contact). Money columns are NUMERIC(39,0),
-- crossing the driver as decimal strings; ratios are integer basis points.
CREATE TABLE policy (
    node_id                    TEXT PRIMARY KEY,
    -- Ratio channels are steered toward (bp, 0–10000).
    target_ratio_bp            SMALLINT      NOT NULL,
    max_amount_per_action      NUMERIC(39,0) NOT NULL,
    max_fee_per_action         NUMERIC(39,0) NOT NULL,
    -- The daily budget: the worst case any failure combination can cost (§4).
    max_fee_per_day            NUMERIC(39,0) NOT NULL,
    min_benefit_bp_per_ckb_fee BIGINT        NOT NULL,
    cooldown_ticks             BIGINT        NOT NULL,
    -- Health thresholds (§5.2).
    depleted_below_bp          SMALLINT      NOT NULL,
    saturated_above_bp         SMALLINT      NOT NULL,
    drift_bp_per_hour          BIGINT        NOT NULL,
    min_drift_points           INT           NOT NULL,
    -- Opt-in: when true, PRICED actions that pass policy execute without an
    -- operator click. Default OFF everywhere.
    autopilot                  BOOLEAN       NOT NULL DEFAULT FALSE,
    updated_at_ms              BIGINT        NOT NULL
);

-- §9 audit claim: every automated action records the policy snapshot that
-- authorized it. Serialized Policy JSON; NULL for operator-approved
-- (advisory) and pre-Phase-3 rows.
ALTER TABLE rebalance_actions ADD COLUMN policy_snapshot TEXT;
