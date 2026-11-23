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
