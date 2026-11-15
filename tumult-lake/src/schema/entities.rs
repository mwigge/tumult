//! v21 entity DDL: `entity_versions`, one row per distinct state of one
//! entity, and the `entities_current` view over the latest row of each.

/// Index-free and without a primary key, like every table that receives
/// UPDATEs since v5: `last_seen_ns` and `last_received_ns` move on each
/// heartbeat and a delete sets the tombstone. `crate::entities` keeps
/// `(entity_key, environment, valid_from_ns)` unique.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS entity_versions (
    entity_key VARCHAR NOT NULL,
    environment VARCHAR NOT NULL DEFAULT '',
    entity_type VARCHAR NOT NULL,
    entity_id MAP (VARCHAR, VARCHAR) NOT NULL,
    state_hash VARCHAR NOT NULL,
    description JSON,
    relationships JSON,
    valid_from_ns BIGINT NOT NULL,
    last_seen_ns BIGINT NOT NULL,
    last_received_ns BIGINT NOT NULL,
    interval_s BIGINT NOT NULL DEFAULT 0,
    deleted_at_ns BIGINT,
    delete_reason VARCHAR,
    wire_form VARCHAR NOT NULL,
    observer VARCHAR NOT NULL,
    schema_url VARCHAR
);
";

/// For a table that a development build of v21 created before the column
/// existed: `CREATE TABLE IF NOT EXISTS` leaves such a table as it is. The
/// column is added and filled from `last_seen_ns`, the best record such a
/// lake has of when the daemon last heard of an entity. An added column
/// cannot be declared `NOT NULL`; every statement of `crate::entities`
/// writes it. Run before [`VIEW`], which reads the column.
pub(super) const ADD_RECEIPT_COLUMN: &str = "
ALTER TABLE entity_versions ADD COLUMN IF NOT EXISTS last_received_ns BIGINT;
UPDATE entity_versions SET last_received_ns = last_seen_ns
WHERE last_received_ns IS NULL;
";

/// The latest row of each entity per environment, with its liveness:
/// `gone` once deleted, `expired` after three missed report intervals,
/// `live` otherwise. An entity with `interval_s = 0` sends no periodic
/// reports and never expires.
///
/// The silence is measured from `last_received_ns`, which the daemon's
/// clock set, against the daemon's `now()`. `last_seen_ns` is the sender's
/// clock: a sender that runs behind, or delivers from a retry queue, would
/// otherwise look silent while it reports.
pub(super) const VIEW: &str = "
CREATE OR REPLACE VIEW entities_current AS
SELECT
    entity_key,
    environment,
    entity_type,
    entity_id,
    state_hash,
    description,
    relationships,
    valid_from_ns,
    last_seen_ns,
    last_received_ns,
    interval_s,
    deleted_at_ns,
    delete_reason,
    wire_form,
    observer,
    schema_url,
    CASE
        WHEN deleted_at_ns IS NOT NULL THEN 'gone'
        WHEN
            interval_s > 0
            AND epoch_ns(now()) - last_received_ns > 3 * interval_s * 1000000000
            THEN 'expired'
        ELSE 'live'
    END AS liveness
FROM entity_versions
QUALIFY
    row_number() OVER (
        PARTITION BY entity_key, environment
        ORDER BY valid_from_ns DESC
    ) = 1;
";
