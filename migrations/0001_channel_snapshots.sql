-- Channel snapshot time-series (architecture §5.1).
--
-- Money columns are NUMERIC(39,0): the full u128 Shannon range, no floats.
-- node_id keys every row so the schema is fleet-ready (a v1 non-goal, but
-- anticipated per §1.3); v1 always writes one node's pubkey.
CREATE TABLE channel_snapshots (
    id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    node_id              TEXT        NOT NULL,
    channel_id           TEXT        NOT NULL,
    peer                 TEXT        NOT NULL,
    -- 'ckb' or 'udt:<type-script-json>'
    asset                TEXT        NOT NULL,
    local_balance        NUMERIC(39,0) NOT NULL,
    remote_balance       NUMERIC(39,0) NOT NULL,
    offered_tlc_balance  NUMERIC(39,0) NOT NULL,
    received_tlc_balance NUMERIC(39,0) NOT NULL,
    -- Derived §5.1 fields, persisted so dashboards and ad-hoc SQL agree
    -- with the decision core without re-deriving.
    usable_out           NUMERIC(39,0) NOT NULL,
    usable_in            NUMERIC(39,0) NOT NULL,
    usable_ratio_bp      SMALLINT,   -- NULL for zero-capacity channels
    ready                BOOLEAN     NOT NULL,
    -- Poller clock, milliseconds since the UNIX epoch.
    at_ms                BIGINT      NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Drift windows are read per channel, newest-bounded.
CREATE INDEX idx_snapshots_channel_time
    ON channel_snapshots (node_id, channel_id, at_ms DESC);

-- Staleness checks read the newest snapshot per node.
CREATE INDEX idx_snapshots_node_time
    ON channel_snapshots (node_id, at_ms DESC);
