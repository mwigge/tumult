//! `GET /api/dr?target=&environment=&from=&to=&limit=&cursor=`: the DR
//! view of the environments of the caller.
//!
//! * `targets`: one entry per target of the DR test register, read as of
//!   now: its state, the last DR test, when the next one is due and where
//!   the interval comes from, the planned DR exercise that covers it, the
//!   RTO and RPO it is held to and what the last test measured.
//! * `tests`: the rows of the lake view `dr_tests` in the period, newest
//!   first, one page at a time as the entity list pages (`limit`, default
//!   100 and at most 1000, `total`, `truncated` and `next_cursor`).
//! * `summary`: the targets per state, and the tests of the period per
//!   origin and verdict.
//!
//! A run that did not recover measured no RTO. Its test, and a target
//! whose last test it is, say the least its RTO was in `rto_at_least_s`:
//! the time from the outage seen to the end of the watch, as its verdict
//! on the RTO holds it.
//!
//! The period is `from` to `to` in unix nanoseconds, both ends included:
//! by default the 365 days up to now, or up to `to`. It selects the tests;
//! the targets stay as they are now. `target` keeps one target system.
//! A scoped principal sees its own environments only, and `environment`
//! narrows the view to one of them. A query field the route does not know
//! is refused, and every value of a request reaches the store as a bound
//! parameter.

use std::collections::HashMap;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tumult_lake::{DrScope, DrTestFilter, DrTestPosition, Reader, SqlParam};

use crate::auth::Principal;
use crate::error::{bad_request, ApiError};
use crate::handlers::entities::page::{decode_cursor, encode_cursor, row_limit};
use crate::handlers::risks::envs_for;
use crate::sql_util::{now_ns, with_reader};
use crate::ApiState;

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// The days of the period when the request names no start.
const PERIOD_DAYS: i64 = 365;

/// The query of `GET /api/dr`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DrQuery {
    /// The target system.
    target: Option<String>,
    /// One environment of the caller.
    environment: Option<String>,
    /// The start of the period, in unix nanoseconds.
    from: Option<i64>,
    /// The end of the period, in unix nanoseconds.
    to: Option<i64>,
    /// The most tests on one page.
    limit: Option<u32>,
    /// Where the page starts: the `next_cursor` of the page before.
    cursor: Option<String>,
}

/// The answer of `GET /api/dr`.
#[derive(Debug, Serialize)]
pub(crate) struct DrView {
    /// The time the targets are read at.
    as_of_ns: i64,
    /// The start of the period of the tests.
    from_ns: i64,
    /// The end of the period of the tests.
    to_ns: i64,
    targets: Vec<DrTarget>,
    /// The tests of the period on this page.
    tests: Vec<DrTest>,
    /// The tests on this page.
    count: usize,
    /// The tests of the period.
    total: u64,
    /// Whether tests of the period come after this page.
    truncated: bool,
    /// The cursor of the next page, or none on the last page.
    next_cursor: Option<String>,
    summary: DrSummary,
}

/// One target of the DR test register.
#[derive(Debug, Serialize)]
pub(crate) struct DrTarget {
    system: String,
    environment: Option<String>,
    /// `ok`, `due` (within 30 days), `overdue` or `never_tested`.
    state: String,
    /// The last DR test that proves something, if any.
    last_test: Option<DrLastTest>,
    /// When the next DR test is due; none for a target never tested.
    next_due_at_ns: Option<i64>,
    /// The days between two DR tests.
    interval_days: i64,
    /// Where the interval comes from: `plan`, `function` or `default`.
    interval_source: String,
    /// The planned DR exercise that covers the target.
    plan: Option<DrPlan>,
    /// The RTO the target is held to: that of the last test, else that
    /// of the plan.
    rto_target_s: Option<f64>,
    /// The RPO the target is held to, read as the RTO is.
    rpo_target_s: Option<f64>,
    /// The RTO the last test measured.
    rto_measured_s: Option<f64>,
    /// The RPO the last test measured.
    rpo_measured_s: Option<f64>,
    /// Whether the target recovered in the last test.
    recovered: Option<bool>,
    /// The least the RTO was, when the last test is a run that did not
    /// recover.
    rto_at_least_s: Option<f64>,
}

/// The last DR test of a target.
#[derive(Debug, Serialize)]
pub(crate) struct DrLastTest {
    /// The `test_id` of the test in `dr_tests`.
    id: String,
    /// `automated` or `manual`.
    origin: String,
    executed_at_ns: i64,
    /// `met`, `breached`, `not_recovered` or `not_measured`.
    verdict: String,
}

/// The planned DR exercise that covers a target.
#[derive(Debug, Serialize)]
pub(crate) struct DrPlan {
    id: String,
    due_at_ns: i64,
}

/// One row of the lake view `dr_tests`, with every column of the view.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DrTest {
    test_id: String,
    origin: String,
    experiment_name: Option<String>,
    exercise_type: Option<String>,
    target_system: Option<String>,
    target_environment: Option<String>,
    executed_at_ns: i64,
    outcome: Option<String>,
    rto_target_s: Option<f64>,
    rto_measured_s: Option<f64>,
    /// The least the RTO was, for a run that did not recover. Not a
    /// column of the view.
    #[serde(default)]
    rto_at_least_s: Option<f64>,
    recovered: Option<bool>,
    no_outage: Option<bool>,
    rpo_target_s: Option<f64>,
    rpo_measured_s: Option<f64>,
    data_verified: Option<bool>,
    verdict: String,
    evidence_id: String,
    reviewer: Option<String>,
    plan_id: Option<String>,
}

/// The counts of a DR view.
#[derive(Debug, Default, Serialize)]
pub(crate) struct DrSummary {
    /// The targets per state.
    states: StateCounts,
    /// The tests of the period.
    tests: TestCounts,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct StateCounts {
    ok: u64,
    due: u64,
    overdue: u64,
    never_tested: u64,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct TestCounts {
    total: u64,
    automated: VerdictCounts,
    manual: VerdictCounts,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct VerdictCounts {
    met: u64,
    breached: u64,
    not_recovered: u64,
    not_measured: u64,
}

/// A row of `Reader::dr_targets`.
#[derive(Debug, Deserialize)]
struct TargetRow {
    target_system: String,
    target_environment: Option<String>,
    state: String,
    last_test_id: Option<String>,
    last_test_at_ns: Option<i64>,
    last_test_origin: Option<String>,
    last_test_verdict: Option<String>,
    next_due_at_ns: Option<i64>,
    interval_days: i64,
    interval_source: String,
    plan_id: Option<String>,
    plan_due_at_ns: Option<i64>,
    rto_target_s: Option<f64>,
    rpo_target_s: Option<f64>,
    rto_measured_s: Option<f64>,
    rpo_measured_s: Option<f64>,
    recovered: Option<bool>,
}

impl From<TargetRow> for DrTarget {
    fn from(row: TargetRow) -> Self {
        let last_test = match (
            row.last_test_id,
            row.last_test_origin,
            row.last_test_at_ns,
            row.last_test_verdict,
        ) {
            (Some(id), Some(origin), Some(executed_at_ns), Some(verdict)) => Some(DrLastTest {
                id,
                origin,
                executed_at_ns,
                verdict,
            }),
            _ => None,
        };
        let plan = row
            .plan_id
            .zip(row.plan_due_at_ns)
            .map(|(id, due_at_ns)| DrPlan { id, due_at_ns });
        Self {
            system: row.target_system,
            environment: row.target_environment,
            state: row.state,
            last_test,
            next_due_at_ns: row.next_due_at_ns,
            interval_days: row.interval_days,
            interval_source: row.interval_source,
            plan,
            rto_target_s: row.rto_target_s,
            rpo_target_s: row.rpo_target_s,
            rto_measured_s: row.rto_measured_s,
            rpo_measured_s: row.rpo_measured_s,
            recovered: row.recovered,
            rto_at_least_s: None,
        }
    }
}

/// What one request reads.
struct DrRequest {
    scope: DrScope,
    target: Option<String>,
    from_ns: i64,
    to_ns: i64,
    after: Option<DrTestPosition>,
    limit: u32,
    as_of_ns: i64,
}

impl DrRequest {
    /// The tests of the request: of its target, in its period.
    fn filter(&self) -> DrTestFilter {
        DrTestFilter {
            target: self.target.clone(),
            from_ns: Some(self.from_ns),
            to_ns: Some(self.to_ns),
        }
    }
}

/// The period of a request: `from` to `to`, by default the 365 days up
/// to `to`, and `to` by default `now_ns`. The DR test report reads its
/// period by the same rule.
pub(crate) fn period(
    from: Option<i64>,
    to: Option<i64>,
    now_ns: i64,
) -> Result<(i64, i64), String> {
    let to_ns = to.unwrap_or(now_ns);
    let from_ns = from.unwrap_or_else(|| to_ns.saturating_sub(PERIOD_DAYS * DAY_NS));
    if from_ns > to_ns {
        return Err("from must not be later than to".into());
    }
    Ok((from_ns, to_ns))
}

/// `rows` read as `T`.
fn rows_as<T: DeserializeOwned>(rows: Vec<Value>) -> Result<Vec<T>, String> {
    rows.into_iter()
        .map(|row| serde_json::from_value(row).map_err(|e| format!("a DR row: {e}")))
        .collect()
}

/// The targets of `targets` per state.
fn state_counts(targets: &[DrTarget]) -> Result<StateCounts, String> {
    let mut counts = StateCounts::default();
    for target in targets {
        let count = match target.state.as_str() {
            "ok" => &mut counts.ok,
            "due" => &mut counts.due,
            "overdue" => &mut counts.overdue,
            "never_tested" => &mut counts.never_tested,
            other => return Err(format!("unknown DR state {other:?}")),
        };
        *count += 1;
    }
    Ok(counts)
}

/// The rows of `Reader::dr_test_counts` as counts per origin and verdict.
fn test_counts(rows: &[Value]) -> Result<TestCounts, String> {
    let mut counts = TestCounts::default();
    for row in rows {
        let origin = row["origin"].as_str().unwrap_or_default();
        let verdict = row["verdict"].as_str().unwrap_or_default();
        let tests = row["tests"].as_u64().unwrap_or_default();
        let verdicts = match origin {
            "automated" => &mut counts.automated,
            "manual" => &mut counts.manual,
            other => return Err(format!("unknown DR test origin {other:?}")),
        };
        let count = match verdict {
            "met" => &mut verdicts.met,
            "breached" => &mut verdicts.breached,
            "not_recovered" => &mut verdicts.not_recovered,
            "not_measured" => &mut verdicts.not_measured,
            other => return Err(format!("unknown DR verdict {other:?}")),
        };
        *count += tests;
        counts.total += tests;
    }
    Ok(counts)
}

/// The RTO verdict of each run of `$1`, a JSON list of experiment ids,
/// that did not recover: its observed value is the time from the outage
/// seen to the end of the watch, the least its RTO was.
const RTO_AT_LEAST_SQL: &str = "SELECT experiment_id, max(observed) AS rto_at_least_s \
     FROM nfr_verdicts \
     WHERE kind = 'rto' AND nfr_id = 'rto' AND NOT recovered AND observed IS NOT NULL \
     AND list_contains(from_json(CAST($1 AS VARCHAR), '[\"VARCHAR\"]'), experiment_id) \
     GROUP BY experiment_id";

/// A row of [`RTO_AT_LEAST_SQL`].
#[derive(Debug, Deserialize)]
struct AtLeastRow {
    experiment_id: String,
    rto_at_least_s: f64,
}

/// The experiment id of a run in the `test_id` of `dr_tests`, which is
/// the origin and the evidence id: none for a manual record.
fn run_id(test_id: &str) -> Option<&str> {
    test_id.strip_prefix("automated:")
}

/// The least RTO of each run among `ids` that did not recover, by
/// experiment id.
fn rto_at_least(reader: &Reader, ids: &[String]) -> Result<HashMap<String, f64>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = reader
        .query_json_rows_bound(RTO_AT_LEAST_SQL, &[SqlParam::text_list(ids)])
        .map_err(|e| e.to_string())?;
    Ok(rows_as::<AtLeastRow>(rows)?
        .into_iter()
        .map(|row| (row.experiment_id, row.rto_at_least_s))
        .collect())
}

/// `targets` and `tests` with the least RTO of each run among them that
/// did not recover. The runs are asked for, and the store keeps those
/// that did not recover.
fn with_rto_at_least(
    reader: &Reader,
    targets: &mut [DrTarget],
    tests: &mut [DrTest],
) -> Result<(), String> {
    let mut ids: Vec<String> = tests
        .iter()
        .filter(|test| test.origin == "automated")
        .map(|test| test.evidence_id.clone())
        .collect();
    ids.extend(
        targets
            .iter()
            .filter_map(|target| run_id(&target.last_test.as_ref()?.id))
            .map(str::to_string),
    );
    let found = rto_at_least(reader, &ids)?;
    for test in tests.iter_mut().filter(|test| test.origin == "automated") {
        test.rto_at_least_s = found.get(&test.evidence_id).copied();
    }
    for target in targets.iter_mut() {
        let id = target.last_test.as_ref().and_then(|last| run_id(&last.id));
        target.rto_at_least_s = id.and_then(|id| found.get(id)).copied();
    }
    Ok(())
}

/// One page of DR tests.
struct TestPage {
    tests: Vec<DrTest>,
    /// Whether tests of the period come after the page.
    truncated: bool,
    /// Where the next page starts, when one does.
    next_cursor: Option<String>,
}

/// The targets of the register in the scope of `request`.
fn read_targets(reader: &Reader, request: &DrRequest) -> Result<Vec<DrTarget>, String> {
    let rows = reader
        .dr_targets(&request.scope, request.target.as_deref(), request.as_of_ns)
        .map_err(|e| e.to_string())?;
    Ok(rows_as::<TargetRow>(rows)?
        .into_iter()
        .map(DrTarget::from)
        .collect())
}

/// The page of tests `request` asks for.
fn read_page(reader: &Reader, request: &DrRequest) -> Result<TestPage, String> {
    // One test more than the page holds says whether the period goes on.
    let rows = reader
        .dr_tests_page(
            &request.scope,
            &request.filter(),
            request.after.as_ref(),
            request.limit + 1,
        )
        .map_err(|e| e.to_string())?;
    let mut tests: Vec<DrTest> = rows_as(rows)?;
    let page_len = usize::try_from(request.limit).map_err(|e| e.to_string())?;
    let truncated = tests.len() > page_len;
    tests.truncate(page_len);
    let next_cursor = match tests.last() {
        Some(last) if truncated => Some(encode_cursor(&(last.executed_at_ns, &last.test_id))?),
        _ => None,
    };
    Ok(TestPage {
        tests,
        truncated,
        next_cursor,
    })
}

/// The DR view of `request`.
fn read_view(reader: &Reader, request: &DrRequest) -> Result<DrView, String> {
    let mut targets = read_targets(reader, request)?;
    let mut page = read_page(reader, request)?;
    with_rto_at_least(reader, &mut targets, &mut page.tests)?;
    let counts = reader
        .dr_test_counts(&request.scope, &request.filter())
        .map_err(|e| e.to_string())?;
    let summary = DrSummary {
        states: state_counts(&targets)?,
        tests: test_counts(&counts)?,
    };
    Ok(DrView {
        as_of_ns: request.as_of_ns,
        from_ns: request.from_ns,
        to_ns: request.to_ns,
        targets,
        count: page.tests.len(),
        total: summary.tests.total,
        tests: page.tests,
        truncated: page.truncated,
        next_cursor: page.next_cursor,
        summary,
    })
}

/// `GET /api/dr`: see the module docs.
pub(crate) async fn view(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    query: Result<Query<DrQuery>, QueryRejection>,
) -> Result<Json<DrView>, ApiError> {
    let Query(q) = query.map_err(|e| bad_request(format!("invalid query: {e}")))?;
    let envs = envs_for(&principal, q.environment.filter(|e| !e.is_empty()))?;
    let as_of_ns = now_ns();
    let (from_ns, to_ns) = period(q.from, q.to, as_of_ns).map_err(bad_request)?;
    let after: Option<DrTestPosition> = decode_cursor(q.cursor.as_deref()).map_err(bad_request)?;
    let request = DrRequest {
        scope: DrScope {
            dr_types: state.score_runtime().params().dr_exercise_types(),
            envs,
        },
        target: q.target.filter(|t| !t.is_empty()),
        from_ns,
        to_ns,
        after,
        limit: row_limit(q.limit),
        as_of_ns,
    };
    let view = with_reader(&state.db_path, move |reader| read_view(reader, &request)).await?;
    Ok(Json(view))
}

/// The statements of the DR view, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![RTO_AT_LEAST_SQL]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_period_is_the_365_days_up_to_its_end_and_its_end_is_now_by_default() {
        let now = 1_000 * DAY_NS;
        assert_eq!(period(None, None, now), Ok((635 * DAY_NS, now)));
        assert_eq!(
            period(None, Some(500 * DAY_NS), now),
            Ok((135 * DAY_NS, 500 * DAY_NS))
        );
        assert_eq!(period(Some(7), None, now), Ok((7, now)));
        assert_eq!(period(Some(7), Some(7), now), Ok((7, 7)));
        assert_eq!(
            period(Some(8), Some(7), now),
            Err("from must not be later than to".to_string())
        );
        assert_eq!(
            period(None, Some(i64::MIN + 1), now),
            Ok((i64::MIN, i64::MIN + 1)),
            "a start before the first time there is is that time"
        );
    }
}
