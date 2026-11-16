//! v17 risk DDL: `risk_suppressions`, the append-only, hash-chained record
//! of passive-scan findings an operator has accepted for a while. Additive
//! and index-free under the same crash-robustness rule as the v5 run
//! tables.

/// One row per suppression event. `risk_id` is the scanner's stable
/// finding id; the chain (`prev_hash` → `new_hash`) is per risk id, so a
/// finding's acceptance history can be verified independently.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS risk_suppressions (
    id VARCHAR NOT NULL, risk_id VARCHAR NOT NULL,
    reason VARCHAR NOT NULL, actor VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL, expires_at_ns BIGINT,
    prev_hash VARCHAR, new_hash VARCHAR NOT NULL
);
";
