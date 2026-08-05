//! v10–v13 table-stakes DDL: recurring runs, outbound event notifications
//! and GameDay campaign columns — additive and index-free under the same
//! rule as the v5 run tables.

/// `run_schedules` (v10, interval semantics — not cron; see
/// `tumult_lake::schedules` docs), `webhooks` / `webhook_cursors` (v11),
/// `webhook_dead_letters` (v13), and the v12 GameDay columns (additive
/// ALTERs like v9).
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS run_schedules (
    id              VARCHAR NOT NULL,   -- uuid
    name            VARCHAR NOT NULL,
    registry_id     VARCHAR NOT NULL,   -- references run_registry.id
    interval_s      BIGINT NOT NULL,    -- seconds between fires
    vars_json       VARCHAR,            -- template vars, same shape as runs.params_json
    env             VARCHAR NOT NULL,   -- tier-classified at fire time (default 'dev')
    target          VARCHAR,
    enabled         BOOLEAN NOT NULL,
    next_run_at_ns  BIGINT NOT NULL,
    last_run_at_ns  BIGINT,
    last_run_id     VARCHAR,
    created_by      VARCHAR,
    created_at_ns   BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS webhooks (
    id              VARCHAR NOT NULL,   -- uuid
    name            VARCHAR NOT NULL,
    url             VARCHAR NOT NULL,
    secret          VARCHAR NOT NULL,   -- HMAC key; never serialized by the API
    events          VARCHAR NOT NULL,   -- JSON array of event names; [] = all
    enabled         BOOLEAN NOT NULL,
    created_by      VARCHAR,
    created_at_ns   BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS webhook_cursors (
    webhook_id      VARCHAR NOT NULL,
    last_at_ns      BIGINT NOT NULL     -- run_audit position delivered up to
);

-- v13: webhook dead letters. Same index-free rule. One row per audit event
-- the dispatcher gave up on after bounded retries; `run_audit` remains the
-- source of truth for replay, this table is the record of the loss.
CREATE TABLE IF NOT EXISTS webhook_dead_letters (
    webhook_id  VARCHAR NOT NULL,
    run_id      VARCHAR NOT NULL,
    at_ns       BIGINT NOT NULL,    -- the audit event's original timestamp
    event       VARCHAR NOT NULL,
    detail      VARCHAR,
    actor       VARCHAR,
    error       VARCHAR,            -- last delivery error
    attempts    INTEGER NOT NULL,   -- consecutive failed dispatch ticks
    dead_at_ns  BIGINT NOT NULL     -- when the dispatcher gave up
);

-- v12: GameDay campaigns (additive, like the v9 token-expiry ALTER).
ALTER TABLE run_registry ADD COLUMN IF NOT EXISTS kind VARCHAR;
ALTER TABLE runs ADD COLUMN IF NOT EXISTS gameday_id VARCHAR;
";
