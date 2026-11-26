//! What the lake holds about a target, read for one request. Runs and
//! samples are bounded by the request's lookback: runs are the experiment
//! root spans at or after it, and the target's spans are read only around
//! the runs that recorded no version. Versions are not bounded by it:
//! they come from the `service_version_sightings` rollup and from every
//! run that recorded one, never from a scan of the target's spans.
//!
//! Whatever a request supplied — the target, the series, the principal's
//! scopes, the lookback — reaches the lake as a bound parameter, never as
//! part of a statement's text.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;
use tumult_core::runner::VERSION_WINDOW_SKEW;
use tumult_core::types::bounded_version_text;
use tumult_lake::{Reader, SqlParam, VersionSighting};

use crate::sql_util::{in_scopes, scopes_param};

use super::select::{Env, Execution, RecordedVersion, Run, Sample, STALE_NS};

/// Stored verdicts returned as history.
const HISTORY_LIMIT: usize = 10;
/// Stored verdicts read to find the ones of the unnamed environment.
const UNNAMED_HISTORY_SCAN: usize = 200;
/// The series a latency sample's window length and request timeout are
/// written under, beside it.
const WINDOW_SERIES: &str = "load.window_s";
const TIMEOUT_SERIES: &str = "load.request_timeout_ms";
/// The series a request timeout censors.
const LATENCY_PREFIX: &str = "load.latency_";

/// What a verdict is decided from.
#[derive(Debug, Default)]
pub(crate) struct Evidence {
    /// The target's versions per environment, within the principal's
    /// scopes, whenever seen.
    pub sightings: Vec<VersionSighting>,
    /// The versions runs against the target recorded, whenever they ran.
    pub recorded: Vec<RecordedVersion>,
    /// The runs against the target inside the lookback, in start order.
    pub runs: Vec<Run>,
    /// The requested series of those runs, in a fixed order.
    pub samples: Vec<Sample>,
    /// The rollup is still being read back from an old lake's spans:
    /// versions and deployments may be missing from `sightings`.
    pub backfill_running: bool,
}

/// The experiment root spans against the target `$1` at or after `$2`,
/// inside the scopes `$3`.
fn roots_where() -> String {
    format!(
        "span_name = 'resilience.experiment' AND target_system = $1 \
         AND experiment_id IS NOT NULL AND ts_ns >= $2 AND {}",
        in_scopes(3, "target_environment")
    )
}

/// The CTE every run query starts from: `runs`, one row per run against
/// the target with its window, its environment and the version it
/// recorded — on its journal, or else on its root span.
fn runs_cte() -> String {
    format!(
        "WITH roots AS (\
         SELECT experiment_id AS id, min(ts_ns) AS started, \
         max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, \
         max(span_attrs['resilience.target.version']) AS span_version, \
         max(span_attrs['resilience.target.version_source']) AS span_source \
         FROM spans WHERE {} GROUP BY experiment_id), \
         runs AS (\
         SELECT r.id, r.started, r.ended, r.env, e.status, \
         coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, \
         CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source \
         ELSE r.span_source END AS version_source \
         FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id)",
        roots_where()
    )
}

/// The target's own versioned spans at or after `$4`, joined to the runs
/// that recorded no version, in exactly the run's environment.
const UNRECORDED_JOIN: &str = "FROM runs AS u INNER JOIN spans AS s \
     ON s.target_environment IS NOT DISTINCT FROM u.env \
     WHERE u.version IS NULL AND s.service_name = $1 \
     AND s.service_version IS NOT NULL AND s.service_version <> '' \
     AND s.ts_ns >= $4";

/// A span that is a request the service received or began, as the lake
/// counts them for a run's window.
const IS_REQUEST: &str = "(upper(s.span_kind) IN ('SERVER', 'CONSUMER') \
     OR s.parent_span_id IS NULL OR s.parent_span_id = '')";

/// What a request asks about, as the parameters of its statements.
struct Asked {
    target: String,
    since_ns: i64,
    scopes: SqlParam,
}

impl Asked {
    /// `$1`, `$2`, `$3`: the target, the lookback and the scopes.
    fn runs(&self) -> Vec<SqlParam> {
        vec![
            SqlParam::Text(self.target.clone()),
            SqlParam::Int(self.since_ns),
            self.scopes.clone(),
        ]
    }

    /// The same with `$4`: how far back the target's own spans are read.
    fn spans(&self) -> Vec<SqlParam> {
        let mut params = self.runs();
        params.push(SqlParam::Int(self.since_ns.saturating_sub(STALE_NS)));
        params
    }

    /// The same as [`Asked::runs`] without a lookback.
    fn whenever(&self) -> Vec<SqlParam> {
        vec![
            SqlParam::Text(self.target.clone()),
            SqlParam::Int(i64::MIN),
            self.scopes.clone(),
        ]
    }
}

fn rows(reader: &Reader, sql: &str, params: &[SqlParam]) -> Result<Vec<Value>, String> {
    reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())
}

fn text(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn count(row: &Value, key: &str) -> Option<u64> {
    row.get(key).and_then(Value::as_u64)
}

fn flag(row: &Value, key: &str) -> bool {
    count(row, key).is_some_and(|n| n != 0)
}

/// A version as a span or a journal gave it, held to the bounds.
fn version_of(row: &Value, key: &str) -> Option<String> {
    let raw = row.get(key).and_then(Value::as_str)?;
    Some(bounded_version_text(raw).0.into_owned())
}

fn run_of(row: &Value) -> Option<Run> {
    let source = row.get("version_source").and_then(Value::as_str);
    let recorded = version_of(row, "version").map(|version| (version, source == Some("declared")));
    Some(Run {
        experiment_id: text(row, "id")?,
        started_at_ns: row.get("started")?.as_i64()?,
        ended_at_ns: row.get("ended")?.as_i64()?,
        environment: text(row, "env"),
        recorded,
        mixed: source == Some("mixed"),
        status: text(row, "status"),
        execution: Execution {
            aborted: flag(row, "aborted"),
            truncated: flag(row, "truncated"),
            planned: count(row, "planned"),
            completed: count(row, "completed"),
            unanswered: count(row, "unanswered").unwrap_or(0),
            lost: count(row, "lost").unwrap_or(0),
            requests_sent: count(row, "sent"),
            requests_ok: count(row, "ok"),
        },
        inside: Vec::new(),
        before: Vec::new(),
    })
}

/// The runs over `$1` to `$3`, in start order, with how their load was
/// executed.
fn runs_sql() -> String {
    format!(
        "{} SELECT u.id, u.started, u.ended, u.env, u.status, u.version, \
         u.version_source, \
         CAST(l.aborted AS INTEGER) AS aborted, CAST(l.truncated AS INTEGER) AS truncated, \
         l.planned_windows AS planned, l.completed_windows AS completed, \
         l.unanswered_windows AS unanswered, l.lost_to_unanswered AS lost, \
         l.requests_sent AS sent, l.requests_ok AS ok \
         FROM runs AS u LEFT JOIN load_results AS l ON u.id = l.experiment_id \
         ORDER BY u.started, u.id",
        runs_cte()
    )
}

/// Per run without a recorded version, the requests each version served
/// in the run's window widened by the clock skew, and in its own window,
/// over `$1` to `$4`.
fn inside_sql() -> String {
    let skew_ns = i64::try_from(VERSION_WINDOW_SKEW.as_nanos()).unwrap_or(i64::MAX);
    format!(
        "{} SELECT u.id AS id, s.service_version AS v, count(*) AS n, \
         count(*) FILTER (WHERE s.ts_ns >= u.started AND s.ts_ns <= u.ended) AS own \
         {UNRECORDED_JOIN} \
         AND s.ts_ns >= u.started - {skew_ns} AND s.ts_ns <= u.ended + {skew_ns} \
         AND {IS_REQUEST} \
         GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version",
        runs_cte()
    )
}

/// Per run without a recorded version, the last span of each version
/// before the run's start, over `$1` to `$4`.
fn before_sql() -> String {
    format!(
        "{} SELECT u.id AS id, s.service_version AS v, max(s.ts_ns) AS last {UNRECORDED_JOIN} \
         AND s.ts_ns < u.started AND s.ts_ns >= u.started - {STALE_NS} \
         GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version",
        runs_cte()
    )
}

/// The versions the runs recorded, per environment, over `$1` to `$3`.
fn recorded_sql() -> String {
    format!(
        "{} SELECT u.env AS env, u.version AS version, \
         min(u.started) AS first, max(u.ended) AS last \
         FROM runs AS u WHERE u.version IS NOT NULL \
         AND coalesce(u.version_source, '') <> 'mixed' \
         GROUP BY u.env, u.version ORDER BY u.env, u.version",
        runs_cte()
    )
}

/// The samples of the series `$4` of the runs over `$1` to `$3`, each with
/// its window length and its request timeout.
fn samples_sql() -> String {
    let beside = |series: &str| {
        format!(
            "(SELECT experiment_id, ts_ns, max(value) AS value FROM mine \
             WHERE probe_name = '{series}' GROUP BY experiment_id, ts_ns)"
        )
    };
    format!(
        "WITH mine AS (\
         SELECT experiment_id, ts_ns, probe_name, value FROM probe_samples \
         WHERE phase = 'during' AND experiment_id IN (\
         SELECT experiment_id FROM spans WHERE {roots})) \
         SELECT p.experiment_id AS id, p.value AS value, w.value AS weight, \
         t.value AS timeout \
         FROM mine AS p \
         LEFT JOIN {window} AS w ON w.experiment_id = p.experiment_id AND w.ts_ns = p.ts_ns \
         LEFT JOIN {timeout} AS t ON t.experiment_id = p.experiment_id AND t.ts_ns = p.ts_ns \
         WHERE p.probe_name = $4 \
         ORDER BY p.ts_ns, p.experiment_id, p.value",
        roots = roots_where(),
        window = beside(WINDOW_SERIES),
        timeout = beside(TIMEOUT_SERIES),
    )
}

/// The targets of the runs at or after `$1` in the scopes `$2`.
fn targets_sql() -> String {
    format!(
        "SELECT target_system AS target, target_environment AS env, \
         count(DISTINCT experiment_id) AS runs, max(ts_ns) AS last_run \
         FROM spans WHERE span_name = 'resilience.experiment' \
         AND target_system IS NOT NULL AND experiment_id IS NOT NULL \
         AND ts_ns >= $1 AND {} \
         GROUP BY target_system, target_environment \
         ORDER BY target_system, target_environment NULLS LAST",
        in_scopes(2, "target_environment")
    )
}

/// The runs, in start order, with how their load was executed. A run
/// ingested twice is read once.
fn load_runs(reader: &Reader, asked: &Asked) -> Result<Vec<Run>, String> {
    let sql = runs_sql();
    let mut seen = HashSet::new();
    Ok(rows(reader, &sql, &asked.runs())?
        .iter()
        .filter_map(run_of)
        .filter(|run| seen.insert(run.experiment_id.clone()))
        .collect())
}

/// Per run without a recorded version, how many requests each version of
/// the target served inside the run's own window; for a run whose own
/// window names no version, inside the window widened by the clock skew
/// a runner allows for (the rule the runner follows when it records a
/// version itself). At most runs × versions rows.
fn load_inside(
    reader: &Reader,
    asked: &Asked,
) -> Result<HashMap<String, Vec<(String, u64)>>, String> {
    let sql = inside_sql();
    // Per run and version: requests in the padded window, and in its own.
    let mut inside: HashMap<String, BTreeMap<String, (u64, u64)>> = HashMap::new();
    for row in &rows(reader, &sql, &asked.spans())? {
        let (Some(id), Some(v), Some(n), Some(own)) = (
            text(row, "id"),
            version_of(row, "v"),
            count(row, "n"),
            count(row, "own"),
        ) else {
            continue;
        };
        let served = inside.entry(id).or_default().entry(v).or_default();
        *served = (served.0 + n, served.1 + own);
    }
    Ok(inside
        .into_iter()
        .map(|(id, served)| (id, own_or_padded(&served)))
        .collect())
}

/// The requests each version served in a run's own window; those of the
/// padded window when its own names no version.
fn own_or_padded(served: &BTreeMap<String, (u64, u64)>) -> Vec<(String, u64)> {
    let spoke = served.values().any(|(_, own)| *own > 0);
    served
        .iter()
        .map(|(version, (padded, own))| (version.clone(), if spoke { *own } else { *padded }))
        .filter(|(_, requests)| *requests > 0)
        .collect()
}

/// Per run without a recorded version, each version's last span before
/// the run's start and no more than [`STALE_NS`] before it.
fn load_before(
    reader: &Reader,
    asked: &Asked,
) -> Result<HashMap<String, Vec<(String, i64)>>, String> {
    let sql = before_sql();
    let mut before: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
    for row in &rows(reader, &sql, &asked.spans())? {
        let (Some(id), Some(v), Some(last)) = (
            text(row, "id"),
            version_of(row, "v"),
            row.get("last").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let at = before.entry(id).or_default().entry(v).or_insert(last);
        *at = (*at).max(last);
    }
    Ok(before
        .into_iter()
        .map(|(id, last)| (id, last.into_iter().collect()))
        .collect())
}

/// The versions runs against the target recorded, per environment, with
/// the first and the last of those runs — whenever they ran. A run two
/// versions shared recorded neither.
fn load_recorded(reader: &Reader, asked: &Asked) -> Result<Vec<RecordedVersion>, String> {
    let sql = recorded_sql();
    Ok(rows(reader, &sql, &asked.whenever())?
        .iter()
        .filter_map(|row| {
            Some(RecordedVersion {
                environment: text(row, "env"),
                version: version_of(row, "version")?,
                first_ns: row.get("first")?.as_i64()?,
                last_ns: row.get("last")?.as_i64()?,
            })
        })
        .collect())
}

fn sample_of(row: &Value, latency: bool) -> Option<Sample> {
    let value = row.get("value")?.as_f64()?;
    let timeout = row.get("timeout").and_then(Value::as_f64);
    Some(Sample {
        run: text(row, "id")?,
        value,
        weight: row.get("weight").and_then(Value::as_f64).unwrap_or(1.0),
        censored: latency && timeout.is_some_and(|t| value >= t),
    })
}

/// The series `$4` of the runs, each sample with the seconds of load
/// behind it and whether it is a request given up on at its timeout. The
/// order is total, so the same lake always yields the same sequence.
fn load_samples(reader: &Reader, asked: &Asked, probe: &str) -> Result<Vec<Sample>, String> {
    let sql = samples_sql();
    let mut params = asked.runs();
    params.push(SqlParam::Text(probe.to_string()));
    let latency = probe.starts_with(LATENCY_PREFIX);
    Ok(rows(reader, &sql, &params)?
        .iter()
        .filter_map(|row| sample_of(row, latency))
        .collect())
}

/// The target's sightings inside `scopes` (every environment, the unnamed
/// one included, when there are none).
fn load_sightings(
    reader: &Reader,
    target: &str,
    scopes: &[String],
) -> Result<Vec<VersionSighting>, String> {
    let all = reader
        .version_sightings(target)
        .map_err(|e| e.to_string())?;
    if scopes.is_empty() {
        return Ok(all);
    }
    Ok(all
        .into_iter()
        .filter(|s| {
            s.environment
                .as_ref()
                .is_some_and(|env| scopes.contains(env))
        })
        .collect())
}

/// Read the target's versions, and its runs and samples inside the
/// lookback, all inside `scopes`.
pub(crate) fn load(
    reader: &Reader,
    target: &str,
    probe: &str,
    scopes: &[String],
    since_ns: i64,
) -> Result<Evidence, String> {
    let asked = Asked {
        target: target.to_string(),
        since_ns,
        scopes: scopes_param(scopes),
    };
    let mut inside = load_inside(reader, &asked)?;
    let mut before = load_before(reader, &asked)?;
    let mut runs = load_runs(reader, &asked)?;
    for run in &mut runs {
        run.inside = inside.remove(&run.experiment_id).unwrap_or_default();
        run.before = before.remove(&run.experiment_id).unwrap_or_default();
    }
    Ok(Evidence {
        sightings: load_sightings(reader, target, scopes)?,
        recorded: load_recorded(reader, &asked)?,
        samples: load_samples(reader, &asked, probe)?,
        backfill_running: reader
            .version_backfill_running()
            .map_err(|e| e.to_string())?,
        runs,
    })
}

/// The requests `version` of `target` served in `environment` over
/// `[from_ns, to_ns]`.
pub(crate) fn requests_served(
    reader: &Reader,
    target: &str,
    environment: &Env,
    version: &str,
    (from_ns, to_ns): (i64, i64),
) -> Result<u64, String> {
    let shares = reader
        .observed_versions(target, environment.as_deref(), None, from_ns, to_ns)
        .map_err(|e| e.to_string())?;
    Ok(shares
        .iter()
        .find(|share| share.version == version)
        .map_or(0, |share| share.requests))
}

/// The verdicts stored for `target` in `environment`, latest first.
pub(crate) fn history(
    reader: &Reader,
    target: &str,
    environment: &Env,
) -> Result<Vec<Value>, String> {
    if let Some(env) = environment {
        return reader
            .perf_verdicts(target, std::slice::from_ref(env), HISTORY_LIMIT)
            .map_err(|e| e.to_string());
    }
    let mut rows = reader
        .perf_verdicts(target, &[], UNNAMED_HISTORY_SCAN)
        .map_err(|e| e.to_string())?;
    rows.retain(|row| row.get("envs").is_none_or(Value::is_null));
    rows.truncate(HISTORY_LIMIT);
    Ok(rows)
}

/// One target with runs in the lookback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetSeen {
    pub target: String,
    pub environments: Vec<Env>,
    pub runs: u64,
    pub last_run_ns: i64,
}

/// The targets experiments ran against at or after `since_ns`, by name,
/// each with the environments of its runs.
pub(crate) fn targets(
    reader: &Reader,
    scopes: &[String],
    since_ns: i64,
) -> Result<Vec<TargetSeen>, String> {
    let sql = targets_sql();
    let params = [SqlParam::Int(since_ns), scopes_param(scopes)];
    let mut targets: Vec<TargetSeen> = Vec::new();
    for row in &rows(reader, &sql, &params)? {
        let (Some(target), Some(runs), Some(last)) = (
            text(row, "target"),
            count(row, "runs"),
            row.get("last_run").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let env = text(row, "env");
        match targets.last_mut().filter(|seen| seen.target == target) {
            Some(seen) => {
                seen.environments.push(env);
                seen.runs += runs;
                seen.last_run_ns = seen.last_run_ns.max(last);
            }
            None => targets.push(TargetSeen {
                target,
                environments: vec![env],
                runs,
                last_run_ns: last,
            }),
        }
    }
    Ok(targets)
}

/// The statements of the performance evidence, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        runs_sql(),
        inside_sql(),
        before_sql(),
        recorded_sql(),
        samples_sql(),
        targets_sql(),
    ]
}
