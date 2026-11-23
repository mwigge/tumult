//! The DR history of the lake (schema v25): every DR test of a target,
//! automated and manual together, and the register of when the next one
//! is due.
//!
//! The view `dr_tests` counts the manual exercise types that the default
//! house parameters count as DR. A reader that knows the house passes its
//! own types in a [`DrScope`]; the API reads both through it.

use serde_json::Value;

use crate::schema::{dr_register_sql, dr_tests_sql};
use crate::{ExerciseType, Reader, SqlParam, StoreError};

/// The manual exercise types the default house parameters count as DR.
/// The view `dr_tests` reads these.
pub const DEFAULT_DR_EXERCISE_TYPES: [ExerciseType; 4] = [
    ExerciseType::Backup,
    ExerciseType::Drill,
    ExerciseType::Failover,
    ExerciseType::Restore,
];

/// The manual types of a query, bound as its parameter `$1`.
const BOUND_TYPES: &str = "from_json(CAST($1 AS VARCHAR), '[\"VARCHAR\"]')";

/// Which DR tests a reader asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrScope {
    /// The manual exercise types that count as DR, as the house maps them.
    pub dr_types: Vec<String>,
    /// The environments to read. Empty: every environment.
    pub envs: Vec<String>,
}

impl DrScope {
    fn types(&self) -> SqlParam {
        SqlParam::text_list(&self.dr_types)
    }

    fn envs(&self) -> SqlParam {
        if self.envs.is_empty() {
            return SqlParam::Null;
        }
        SqlParam::text_list(&self.envs)
    }
}

/// The filter on the environments of a query, bound as its parameter `n`.
fn in_envs(column: &str, n: u8) -> String {
    format!(
        "${n} IS NULL OR list_contains(from_json(CAST(${n} AS VARCHAR), '[\"VARCHAR\"]'), {column})"
    )
}

impl Reader {
    /// The rows of `dr_tests` with the manual types of `scope`, in its
    /// environments, oldest first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn dr_tests(&self, scope: &DrScope) -> Result<Vec<Value>, StoreError> {
        let sql = format!(
            "SELECT * FROM ({}) AS t WHERE {} ORDER BY t.executed_at_ns, t.test_id",
            dr_tests_sql(BOUND_TYPES),
            in_envs("t.target_environment", 2)
        );
        self.query_json_rows_bound(&sql, &[scope.types(), scope.envs()])
    }

    /// The due register as of `as_of_ns`: one row per target in the
    /// environments of `scope`, by target and environment. Each row holds
    /// the last DR test (`last_test_at_ns`, `last_test_id`), the interval
    /// in days and where it comes from (`plan`, `function` or `default`),
    /// the next due date, the `state` (`ok`, `due` within 30 days,
    /// `overdue` or `never_tested`) and the planned DR plan that covers
    /// the target, if any (`plan_id`, `plan_due_at_ns`).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn dr_due_register(
        &self,
        scope: &DrScope,
        as_of_ns: i64,
    ) -> Result<Vec<Value>, StoreError> {
        let sql = format!(
            "SELECT * FROM ({}) AS r WHERE {} \
             ORDER BY r.target_system, r.target_environment NULLS FIRST",
            dr_register_sql(BOUND_TYPES, "$2"),
            in_envs("r.target_environment", 3)
        );
        self.query_json_rows_bound(
            &sql,
            &[scope.types(), SqlParam::Int(as_of_ns), scope.envs()],
        )
    }
}

/// Which DR tests of a [`DrScope`] a reader lists: those of one target
/// system, in one period. A field left empty keeps every test.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrTestFilter {
    /// The target system of the tests.
    pub target: Option<String>,
    /// The first time of the period, in unix nanoseconds, included.
    pub from_ns: Option<i64>,
    /// The last time of the period, in unix nanoseconds, included.
    pub to_ns: Option<i64>,
}

impl DrTestFilter {
    /// The parameters of [`in_filter`]: target, start, end.
    fn params(&self) -> [SqlParam; 3] {
        [
            self.target.clone().map_or(SqlParam::Null, SqlParam::Text),
            self.from_ns.map_or(SqlParam::Null, SqlParam::Int),
            self.to_ns.map_or(SqlParam::Null, SqlParam::Int),
        ]
    }
}

/// Where a page of DR tests ended: the time and the id of its last test.
pub type DrTestPosition = (i64, String);

/// The filter of a [`DrTestFilter`] on the tests `t`, bound from parameter
/// `n` on: the target, the start and the end of the period.
fn in_filter(n: u8) -> String {
    let (from, to) = (n + 1, n + 2);
    format!(
        "(CAST(${n} AS VARCHAR) IS NULL OR t.target_system = CAST(${n} AS VARCHAR)) \
         AND (CAST(${from} AS BIGINT) IS NULL OR t.executed_at_ns >= CAST(${from} AS BIGINT)) \
         AND (CAST(${to} AS BIGINT) IS NULL OR t.executed_at_ns <= CAST(${to} AS BIGINT))"
    )
}

/// The DR tests `t` of a scope that a filter keeps, over `$1` to `$5`: the
/// manual types and the environments of the scope, then the filter.
fn kept_tests_sql() -> String {
    format!(
        "SELECT * FROM ({}) AS t WHERE ({}) AND {}",
        dr_tests_sql(BOUND_TYPES),
        in_envs("t.target_environment", 2),
        in_filter(3)
    )
}

/// The position of a page as its two parameters: a null and an empty
/// text for the first page.
fn position_params(after: Option<&DrTestPosition>) -> [SqlParam; 2] {
    let Some((at_ns, test_id)) = after else {
        return [SqlParam::Null, SqlParam::Text(String::new())];
    };
    [SqlParam::Int(*at_ns), SqlParam::Text(test_id.clone())]
}

impl Reader {
    /// One page of the rows of `dr_tests` of `scope` that `filter` keeps,
    /// newest first and then by test id: at most `limit` rows, after the
    /// test at `after` when it is given.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn dr_tests_page(
        &self,
        scope: &DrScope,
        filter: &DrTestFilter,
        after: Option<&DrTestPosition>,
        limit: u32,
    ) -> Result<Vec<Value>, StoreError> {
        let sql = format!(
            "{} AND (CAST($6 AS BIGINT) IS NULL \
             OR t.executed_at_ns < CAST($6 AS BIGINT) \
             OR (t.executed_at_ns = CAST($6 AS BIGINT) AND t.test_id > CAST($7 AS VARCHAR))) \
             ORDER BY t.executed_at_ns DESC, t.test_id LIMIT {limit}",
            kept_tests_sql()
        );
        let [target, from, to] = filter.params();
        let [at, id] = position_params(after);
        self.query_json_rows_bound(
            &sql,
            &[scope.types(), scope.envs(), target, from, to, at, id],
        )
    }

    /// How many DR tests of `scope` that `filter` keeps there are, per
    /// origin and verdict: rows of `origin`, `verdict` and `tests`, by
    /// origin and verdict.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn dr_test_counts(
        &self,
        scope: &DrScope,
        filter: &DrTestFilter,
    ) -> Result<Vec<Value>, StoreError> {
        let sql = format!(
            "SELECT k.origin, k.verdict, count(*) AS tests FROM ({}) AS k \
             GROUP BY k.origin, k.verdict ORDER BY k.origin, k.verdict",
            kept_tests_sql()
        );
        let [target, from, to] = filter.params();
        self.query_json_rows_bound(&sql, &[scope.types(), scope.envs(), target, from, to])
    }

    /// The due register of [`Reader::dr_due_register`], of the target
    /// system `target` only when it is given. Each row also holds what
    /// its last test was (`last_test_origin`, `last_test_verdict`), what
    /// that test measured (`rto_measured_s`, `rpo_measured_s`,
    /// `recovered`), and the RTO and RPO the target is held to
    /// (`rto_target_s`, `rpo_target_s`): those of the last test, and
    /// those of the covering plan where the last test sets none.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn dr_targets(
        &self,
        scope: &DrScope,
        target: Option<&str>,
        as_of_ns: i64,
    ) -> Result<Vec<Value>, StoreError> {
        let sql = format!(
            "SELECT r.target_system, r.target_environment, r.state, \
             r.last_test_id, r.last_test_at_ns, \
             t.origin AS last_test_origin, t.verdict AS last_test_verdict, \
             r.next_due_at_ns, r.interval_days, r.interval_source, \
             r.plan_id, r.plan_due_at_ns, \
             coalesce(t.rto_target_s, p.rto_target_s) AS rto_target_s, \
             coalesce(t.rpo_target_s, p.rpo_target_s) AS rpo_target_s, \
             t.rto_measured_s, t.rpo_measured_s, t.recovered \
             FROM ({register}) AS r \
             LEFT JOIN ({tests}) AS t ON r.last_test_id = t.test_id \
             LEFT JOIN manual_exercise_plans AS p ON r.plan_id = p.id \
             WHERE ({envs}) \
             AND (CAST($4 AS VARCHAR) IS NULL OR r.target_system = CAST($4 AS VARCHAR)) \
             ORDER BY r.target_system, r.target_environment NULLS FIRST",
            register = dr_register_sql(BOUND_TYPES, "$2"),
            tests = dr_tests_sql(BOUND_TYPES),
            envs = in_envs("r.target_environment", 3),
        );
        let target = target.map_or(SqlParam::Null, |t| SqlParam::Text(t.to_string()));
        self.query_json_rows_bound(
            &sql,
            &[scope.types(), SqlParam::Int(as_of_ns), scope.envs(), target],
        )
    }
}
