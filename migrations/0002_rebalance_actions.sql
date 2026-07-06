-- Rebalance action log + fee ledger source (architecture §6).
--
-- One row per intent, driven through the executor state machine:
--   planned → priced → submitting → confirming → settled | failed
--                    ↘ rejected                 ↘ stuck → (reconciled)
-- orphan_suspect marks §7 crash-window payments we could not match.
--
-- The daily fee ledger is an aggregate over settled rows' actual_fee (the
-- get_payment value, never the dry-run quote).
CREATE TABLE rebalance_actions (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- Client-generated, written BEFORE the send RPC (§6 crash-window rule);
    -- the tracing correlation field across the whole action path.
    intent_id      TEXT        NOT NULL UNIQUE,
    node_id        TEXT        NOT NULL,
    asset          TEXT        NOT NULL,
    source_channel TEXT        NOT NULL,
    sink_channel   TEXT        NOT NULL,
    amount         NUMERIC(39,0) NOT NULL,
    benefit_bp     BIGINT      NOT NULL,
    state          TEXT        NOT NULL,
    mode           TEXT        NOT NULL DEFAULT 'advisory',
    quoted_fee     NUMERIC(39,0),
    actual_fee     NUMERIC(39,0),
    payment_hash   TEXT,
    reason         TEXT,
    created_at_ms  BIGINT      NOT NULL,
    updated_at_ms  BIGINT      NOT NULL
);

-- The executor scans non-terminal actions (startup reconciliation, and the
-- one-in-flight guard) and the dashboard reads recent history.
CREATE INDEX idx_actions_node_state ON rebalance_actions (node_id, state);
CREATE INDEX idx_actions_node_time  ON rebalance_actions (node_id, created_at_ms DESC);
