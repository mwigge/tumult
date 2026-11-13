//! v20 function-register DDL: the legal entities a deployment serves, the
//! functions each one registers, the mapping from telemetry identity to
//! function, and the daily per-requirement evidence status.
//!
//! All four tables are additive and index-free under the same
//! crash-robustness rule as the v5 run tables: the register is rewritten
//! on every sync and the status table is upserted daily, so a daemon
//! killed mid-write must not come back with desynced ART indexes. The
//! tables are tiny; scans are free and uniqueness is enforced in code.
//!
//! List-valued columns (`flags`, `feeds`, `declared`, `test_plan`) hold a
//! JSON array as text, like `webhooks.events`.

/// `ict_legal_entities`: one row per legal entity (keyed by LEI) with its
/// scoring profile, flags and connected feeds. `ict_functions`: one row
/// per (function, legal entity, licensed activity); `register_code` is the
/// `F<n>` value assigned once and never renumbered, and `active = false`
/// marks a function removed from the declaration so its code is never
/// reused. `ict_function_services`: the `ict.entity.id` values and service
/// globs that map telemetry to a function. `requirement_status_daily`: the
/// evidence status of one requirement for one function on one day.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS ict_legal_entities (
    lei VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    country VARCHAR,
    entity_type VARCHAR,
    competent_authority VARCHAR,
    profile VARCHAR NOT NULL,
    flags VARCHAR NOT NULL,
    feeds VARCHAR NOT NULL,
    registry_signed_by VARCHAR,
    registry_signed_at VARCHAR,
    updated_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS ict_functions (
    function_id VARCHAR NOT NULL,
    register_code VARCHAR NOT NULL,
    lei VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    licensed_activity VARCHAR NOT NULL,
    tier VARCHAR NOT NULL,
    is_critical_or_important BOOLEAN NOT NULL,
    reasons VARCHAR,
    last_assessed VARCHAR,
    rto_s DOUBLE,
    rpo_s DOUBLE,
    impact VARCHAR,
    org_unit VARCHAR,
    declared VARCHAR NOT NULL,
    test_plan VARCHAR NOT NULL,
    active BOOLEAN NOT NULL,
    position BIGINT NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    updated_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS ict_function_services (
    function_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL,
    pattern VARCHAR NOT NULL,
    position BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS requirement_status_daily (
    as_of DATE NOT NULL,
    function_id VARCHAR NOT NULL,
    requirement_id VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    value DOUBLE,
    source VARCHAR NOT NULL,
    computed_at_ns BIGINT NOT NULL
);
";
