//! What the lake holds about a target, read for one request. Every query
//! is bounded by the request's lookback: runs are the experiment root
//! spans at or after it, and the target's spans are read only around the
//! runs that recorded no version. Versions come from the
//! `service_version_sightings` rollup, never from a scan of the spans.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tumult_lake::{Reader, VersionSighting};

use super::select::{Env, Execution, Run, STALE_NS};
use crate::sql_util::{and_pred, env_scope_where, sql_string};

/// Stored verdicts returned as history.
const HISTORY_LIMIT: usize = 10;
/// Stored verdicts read to find the ones of the unnamed environment.
const UNNAMED_HISTORY_SCAN: usize = 200;

/// What a verdict is decided from.
#[derive(Debug, Default)]
pub(crate) struct Evidence {
    /// The target's versions per environment, within the principal's
    /// scopes, whenever seen.
    pub sightings: Vec<VersionSighting>,
    /// The runs against the target inside the lookback, in start order.
    pub runs: Vec<Run>,
    /// `(experiment_id, value)` of the requested series for those runs, in
    /// a fixed order.
    pub samples: Vec<(String, f64)>,
}

/// The experiment root spans against `t` at or after `since_ns`.
fn roots_where(t: &str, scope: &str, since_ns: i64) -> String {
    format!(
        "span_name = 'resilience.experiment' AND target_system = {t} \
         AND experiment_id IS NOT NULL AND ts_ns >= {since_ns}{scope}"
    )
}

/// The CTE every run query starts from: `runs`, one row per run against
/// the target with its window, its environment and the version it
/// recorded — on its journal, or else on its root span.
fn runs_cte(roots: &str) -> String {
    format!(
        "WITH roots AS (\
         SELECT experiment_id AS id, min(ts_ns) AS started, \
         max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, \
         max(span_attrs['resilience.target.version']) AS span_version, \
         max(span_attrs['resilience.target.version_source']) AS span_source \
         FROM spans WHERE {roots} GROUP BY experiment_id), \
         runs AS (\
         SELECT r.id, r.started, r.ended, r.env, e.status, \
         coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, \
         CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source \
         ELSE r.span_source END AS version_source \
         FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id)"
    )
}

/// The target's own versioned spans, joined to the runs that recorded no
/// version, in exactly the run's environment.
fn unrecorded_join(t: &str, since_ns: i64) -> String {
    format!(
        "FROM runs AS u INNER JOIN spans AS s \
         ON s.target_environment IS NOT DISTINCT FROM u.env \
         WHERE u.version IS NULL AND s.service_name = {t} \
         AND s.service_version IS NOT NULL AND s.service_version <> '' \
         AND s.ts_ns >= {}",
        since_ns.saturating_sub(STALE_NS)
    )
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

fn run_of(row: &Value) -> Option<Run> {
    let recorded = text(row, "version").map(|version| {
        let declared = row.get("version_source").and_then(Value::as_str) == Some("declared");
        (version, declared)
    });
    Some(Run {
        experiment_id: text(row, "id")?,
        started_at_ns: row.get("started")?.as_i64()?,
        ended_at_ns: row.get("ended")?.as_i64()?,
        environment: text(row, "env"),
        recorded,
        status: text(row, "status"),
        execution: Execution {
            aborted: flag(row, "aborted"),
            truncated: flag(row, "truncated"),
            planned: count(row, "planned"),
            completed: count(row, "completed"),
            unanswered: count(row, "unanswered").unwrap_or(0),
            lost: count(row, "lost").unwrap_or(0),
        },
        inside: Vec::new(),
        before: Vec::new(),
    })
}

/// The runs, in start order, with how their load was executed. A run
/// ingested twice is read once.
fn load_runs(reader: &Reader, cte: &str) -> Result<Vec<Run>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{cte} SELECT u.id, u.started, u.ended, u.env, u.status, u.version, \
             u.version_source, \
             CAST(l.aborted AS INTEGER) AS aborted, CAST(l.truncated AS INTEGER) AS truncated, \
             l.planned_windows AS planned, l.completed_windows AS completed, \
             l.unanswered_windows AS unanswered, l.lost_to_unanswered AS lost \
             FROM runs AS u LEFT JOIN load_results AS l ON u.id = l.experiment_id \
             ORDER BY u.started, u.id"
        ))
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    Ok(rows
        .iter()
        .filter_map(run_of)
        .filter(|run| seen.insert(run.experiment_id.clone()))
        .collect())
}

/// Per run without a recorded version, how many of the target's spans
/// each version has inside the run's window: at most runs × versions
/// rows.
fn load_inside(
    reader: &Reader,
    cte: &str,
    join: &str,
) -> Result<HashMap<String, Vec<(String, u64)>>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{cte} SELECT u.id AS id, s.service_version AS v, count(*) AS n {join} \
             AND s.ts_ns >= u.started AND s.ts_ns <= u.ended \
             GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version"
        ))
        .map_err(|e| e.to_string())?;
    let mut inside: HashMap<String, Vec<(String, u64)>> = HashMap::new();
    for row in &rows {
        let (Some(id), Some(v), Some(n)) = (text(row, "id"), text(row, "v"), count(row, "n"))
        else {
            continue;
        };
        inside.entry(id).or_default().push((v, n));
    }
    Ok(inside)
}

/// Per run without a recorded version, each version's last span before
/// the run's start and no more than [`STALE_NS`] before it.
fn load_before(
    reader: &Reader,
    cte: &str,
    join: &str,
) -> Result<HashMap<String, Vec<(String, i64)>>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{cte} SELECT u.id AS id, s.service_version AS v, max(s.ts_ns) AS last {join} \
             AND s.ts_ns < u.started AND s.ts_ns >= u.started - {STALE_NS} \
             GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version"
        ))
        .map_err(|e| e.to_string())?;
    let mut before: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for row in &rows {
        let (Some(id), Some(v), Some(last)) = (
            text(row, "id"),
            text(row, "v"),
            row.get("last").and_then(Value::as_i64),
        ) else {
            continue;
        };
        before.entry(id).or_default().push((v, last));
    }
    Ok(before)
}

/// The requested probe series of the runs, as `(experiment_id, value)`.
/// The order is total, so the same lake always yields the same sequence.
fn load_samples(reader: &Reader, probe: &str, roots: &str) -> Result<Vec<(String, f64)>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT experiment_id AS id, value FROM probe_samples \
             WHERE probe_name = {} AND phase = 'during' AND experiment_id IN (\
             SELECT experiment_id FROM spans WHERE {roots}) \
             ORDER BY ts_ns, experiment_id, value",
            sql_string(probe)
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| Some((text(row, "id")?, row.get("value")?.as_f64()?)))
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

/// Read the target's versions, runs and samples inside `scopes` and the
/// lookback.
pub(crate) fn load(
    reader: &Reader,
    target: &str,
    probe: &str,
    scopes: &[String],
    since_ns: i64,
) -> Result<Evidence, String> {
    let t = sql_string(target);
    let scope = and_pred(env_scope_where("target_environment", scopes));
    let roots = roots_where(&t, &scope, since_ns);
    let cte = runs_cte(&roots);
    let join = unrecorded_join(&t, since_ns);
    let mut inside = load_inside(reader, &cte, &join)?;
    let mut before = load_before(reader, &cte, &join)?;
    let mut runs = load_runs(reader, &cte)?;
    for run in &mut runs {
        run.inside = inside.remove(&run.experiment_id).unwrap_or_default();
        run.before = before.remove(&run.experiment_id).unwrap_or_default();
    }
    Ok(Evidence {
        sightings: load_sightings(reader, target, scopes)?,
        samples: load_samples(reader, probe, &roots)?,
        runs,
    })
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
    let scope = and_pred(env_scope_where("target_environment", scopes));
    let rows = reader
        .query_json_rows(&format!(
            "SELECT target_system AS target, target_environment AS env, \
             count(DISTINCT experiment_id) AS runs, max(ts_ns) AS last_run \
             FROM spans WHERE span_name = 'resilience.experiment' \
             AND target_system IS NOT NULL AND experiment_id IS NOT NULL \
             AND ts_ns >= {since_ns}{scope} \
             GROUP BY target_system, target_environment \
             ORDER BY target_system, target_environment NULLS LAST"
        ))
        .map_err(|e| e.to_string())?;
    let mut targets: Vec<TargetSeen> = Vec::new();
    for row in &rows {
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
