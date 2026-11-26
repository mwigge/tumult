//! The statements of the function register and its daily snapshots
//! (`ict.rs`).

pub(crate) const LEGAL_ENTITIES: &str =
    "SELECT lei, name, country, entity_type, competent_authority, profile, flags, feeds, \
    registry_signed_by, registry_signed_at FROM ict_legal_entities ORDER BY lei";

pub(crate) const FUNCTION_SERVICES: &str =
    "SELECT function_id, kind, pattern FROM ict_function_services ORDER BY position";

pub(crate) const DELETE_LEGAL_ENTITIES: &str = "DELETE FROM ict_legal_entities";

pub(crate) const INSERT_LEGAL_ENTITY: &str =
    "INSERT INTO ict_legal_entities (lei, name, country, entity_type, \
    competent_authority, profile, flags, feeds, registry_signed_by, \
    registry_signed_at, updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const DEACTIVATE_FUNCTIONS: &str =
    "UPDATE ict_functions SET active = false, updated_at_ns = ? WHERE active";

pub(crate) const UPDATE_FUNCTION: &str =
    "UPDATE ict_functions SET name = ?, tier = ?, is_critical_or_important = ?, \
    reasons = ?, last_assessed = ?, rto_s = ?, rpo_s = ?, impact = ?, org_unit = ?, \
    declared = ?, test_plan = ?, active = true, position = ?, updated_at_ns = ? \
    WHERE function_id = ? AND lei = ? AND licensed_activity = ?";

pub(crate) const INSERT_FUNCTION: &str =
    "INSERT INTO ict_functions (function_id, register_code, lei, name, \
    licensed_activity, tier, is_critical_or_important, reasons, last_assessed, rto_s, \
    rpo_s, impact, org_unit, declared, test_plan, active, position, registered_at_ns, \
    updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, true, ?, ?, ?)";

pub(crate) const DELETE_FUNCTION_SERVICES: &str = "DELETE FROM ict_function_services";

pub(crate) const INSERT_FUNCTION_SERVICE: &str =
    "INSERT INTO ict_function_services (function_id, kind, pattern, position) \
    VALUES (?, ?, ?, ?)";

pub(crate) const DELETE_REQUIREMENT_STATUS: &str = "DELETE FROM requirement_status_daily \
    WHERE as_of = DATE '1970-01-01' + CAST(? AS INTEGER) \
    AND function_id = ? AND requirement_id = ?";

pub(crate) const INSERT_REQUIREMENT_STATUS: &str =
    "INSERT INTO requirement_status_daily (as_of, function_id, \
    requirement_id, status, value, source, computed_at_ns) \
    VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, ?, ?, ?, ?, ?)";

pub(crate) const DELETE_MATURITY: &str = "DELETE FROM maturity_daily \
    WHERE as_of = DATE '1970-01-01' + CAST(? AS INTEGER) AND lei = ?";

pub(crate) const INSERT_MATURITY: &str =
    "INSERT INTO maturity_daily (as_of, lei, level, index_value, \
    unknown_share, source, computed_at_ns) \
    VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, ?, ?, ?, ?, ?)";

/// Latest day number the day arithmetic accepts (the year 2243): keeps
/// `DATE + INTEGER` inside the range of both types.
const MAX_DAY: i64 = 100_000;

fn day_sql(day: i64) -> String {
    format!(
        "DATE '1970-01-01' + CAST({} AS INTEGER)",
        day.clamp(0, MAX_DAY)
    )
}

const DAY_NUMBER: &str = "CAST(datediff('day', DATE '1970-01-01', as_of) AS BIGINT) AS as_of_day";

/// The newest `limit` status rows of the days `from_day..=to_day`, oldest
/// first.
pub(crate) fn status_between(from_day: i64, to_day: i64, limit: usize) -> String {
    format!(
        "SELECT * FROM (SELECT {DAY_NUMBER}, function_id, requirement_id, status, value, \
         source FROM requirement_status_daily WHERE as_of BETWEEN {} AND {} \
         ORDER BY as_of DESC, function_id DESC, requirement_id DESC LIMIT {limit}) AS newest \
         ORDER BY as_of_day, function_id, requirement_id",
        day_sql(from_day),
        day_sql(to_day)
    )
}

/// The maturity rows of one legal entity for the days `from_day..=to_day`.
pub(crate) fn maturity_between(from_day: i64, to_day: i64) -> String {
    format!(
        "SELECT {DAY_NUMBER}, lei, level, index_value, unknown_share, source \
         FROM maturity_daily WHERE lei = ? AND as_of BETWEEN {} AND {} ORDER BY as_of",
        day_sql(from_day),
        day_sql(to_day)
    )
}

/// Delete the status rows of the days before `before_day`.
pub(crate) fn prune_status(before_day: i64) -> String {
    format!(
        "DELETE FROM requirement_status_daily WHERE as_of < {}",
        day_sql(before_day)
    )
}

/// Delete the maturity rows of the days before `before_day`.
pub(crate) fn prune_maturity(before_day: i64) -> String {
    format!(
        "DELETE FROM maturity_daily WHERE as_of < {}",
        day_sql(before_day)
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        LEGAL_ENTITIES,
        FUNCTION_SERVICES,
        DELETE_LEGAL_ENTITIES,
        INSERT_LEGAL_ENTITY,
        DEACTIVATE_FUNCTIONS,
        UPDATE_FUNCTION,
        INSERT_FUNCTION,
        DELETE_FUNCTION_SERVICES,
        INSERT_FUNCTION_SERVICE,
        DELETE_REQUIREMENT_STATUS,
        INSERT_REQUIREMENT_STATUS,
        DELETE_MATURITY,
        INSERT_MATURITY,
        status_between(20_500, 20_530, 1_000),
        maturity_between(20_500, 20_530),
        prune_status(20_400),
        prune_maturity(20_400)
    ]
}
