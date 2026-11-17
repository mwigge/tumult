//! The evidence of one run, as its journal left it in the lake: the steady
//! state, the steps, the objectives, the load and the samples of the
//! probes. It is the `evidence` of `GET /api/experiments/{id}`.

use serde_json::{json, Map, Value};
use tumult_lake::{Reader, SqlParam};

/// The most probe samples a page carries. `probe_samples_total` says how
/// many the run has.
const MAX_PROBE_SAMPLES: usize = 5000;

/// The most steps and objectives a page carries.
const MAX_ROWS: usize = 2000;

/// The most values of load windows a page reads: ten series a window.
const MAX_WINDOW_VALUES: usize = 20_000;

/// What every series of a load window is named after in `probe_samples`.
const LOAD_SERIES_PREFIX: &str = "load.";

/// The run itself. One row, or none for a run that left no journal.
const SUMMARY: &str = "\
SELECT
    test_kind AS kind,
    started_at_ns,
    ended_at_ns,
    {
        'before_met': hypothesis_before_met,
        'after_met': hypothesis_after_met
    } AS hypothesis,
    {
        'rto_target_s': rto_target_s,
        'rto_measured_s': rto_measured_s,
        'rpo_target_s': rpo_target_s,
        'rpo_measured_s': rpo_measured_s
    } AS recovery
FROM experiments
WHERE experiment_id = $1";

/// The steps in the order the run took them: the steady state, the method,
/// the steady state again, the rollbacks.
const STEPS: &str = "\
SELECT
    phase,
    name,
    activity_type,
    status,
    started_at_ns,
    duration_ms,
    output,
    error
FROM activity_results
WHERE experiment_id = $1
ORDER BY
    CASE phase
        WHEN 'hypothesis_before' THEN 0
        WHEN 'method' THEN 1
        WHEN 'hypothesis_after' THEN 2
        ELSE 3
    END,
    started_at_ns,
    rowid";

/// The objectives in the order the definition declared them.
const OBJECTIVES: &str = "\
SELECT
    nfr_id,
    kind,
    metric,
    operator,
    threshold,
    observed,
    state,
    margin,
    criticality
FROM nfr_verdicts
WHERE experiment_id = $1
ORDER BY rowid";

const LOAD: &str = "\
SELECT
    tool,
    started_at_ns,
    ended_at_ns,
    duration_s,
    vus,
    throughput_rps,
    latency_p50_ms,
    latency_p95_ms,
    latency_p99_ms,
    error_rate,
    total_requests,
    thresholds_met
FROM load_results
WHERE experiment_id = $1";

/// Whether a row of `probe_samples` is a value of a load window: `$2` is
/// the list of the series a window is stored as.
const IS_LOAD_SERIES: &str =
    "list_contains(from_json(CAST($2 AS VARCHAR), '[\"VARCHAR\"]'), probe_name)";

/// The run of `experiment_id` and the series of a load window, as the two
/// parameters of a statement on `probe_samples`.
fn sample_params(experiment_id: &str) -> [SqlParam; 2] {
    let series: Vec<String> = tumult_lake::arrow_convert::load_series_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    [
        SqlParam::Text(experiment_id.to_string()),
        SqlParam::text_list(&series),
    ]
}

fn rows(reader: &Reader, sql: &str, params: &[SqlParam]) -> Result<Vec<Value>, String> {
    reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())
}

/// The rows of `select` for one run, `limit` at most.
fn rows_of_run(
    reader: &Reader,
    select: &str,
    experiment_id: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let params = [SqlParam::Text(experiment_id.to_string())];
    rows(reader, &format!("{select} LIMIT {limit}"), &params)
}

/// The windows of a load, from the values of their series: the values of
/// one window share its start. A series is named in a window without its
/// prefix.
fn windows(values: Vec<Value>) -> Vec<Value> {
    let mut windows: Vec<Map<String, Value>> = Vec::new();
    for value in values {
        let Some(series) = value["probe_name"].as_str() else {
            continue;
        };
        let series = series.strip_prefix(LOAD_SERIES_PREFIX).unwrap_or(series);
        let start = &value["ts_ns"];
        if windows.last().is_none_or(|last| &last["ts_ns"] != start) {
            windows.push(Map::from_iter([("ts_ns".to_string(), start.clone())]));
        }
        if let Some(window) = windows.last_mut() {
            window.insert(series.to_string(), value["value"].clone());
        }
    }
    windows.into_iter().map(Value::Object).collect()
}

/// The load of the run with its windows, or `null` for a run without one.
fn load(reader: &Reader, experiment_id: &str) -> Result<Value, String> {
    let Some(mut load) = rows_of_run(reader, LOAD, experiment_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(Value::Null);
    };
    let values = rows(
        reader,
        &format!(
            "SELECT ts_ns, probe_name, value FROM probe_samples \
             WHERE experiment_id = $1 AND {IS_LOAD_SERIES} \
             ORDER BY ts_ns, rowid LIMIT {MAX_WINDOW_VALUES}"
        ),
        &sample_params(experiment_id),
    )?;
    load["windows"] = Value::Array(windows(values));
    Ok(load)
}

/// The samples of the probes of the run, the earliest first, and how many
/// the run has.
fn probe_samples(reader: &Reader, experiment_id: &str) -> Result<(Vec<Value>, Value), String> {
    let of_probes = format!("FROM probe_samples WHERE experiment_id = $1 AND NOT {IS_LOAD_SERIES}");
    let params = sample_params(experiment_id);
    let samples = rows(
        reader,
        &format!(
            "SELECT probe_name AS probe, phase, ts_ns, value AS duration_ms, ok {of_probes} \
             ORDER BY ts_ns, probe_name, rowid LIMIT {MAX_PROBE_SAMPLES}"
        ),
        &params,
    )?;
    let total = rows(
        reader,
        &format!("SELECT count(*) AS n {of_probes}"),
        &params,
    )?;
    let total = total.first().map_or(json!(0), |row| row["n"].clone());
    Ok((samples, total))
}

/// The evidence of the run `experiment_id`: `null` for a run the lake has
/// no journal of.
pub(crate) fn of_run(reader: &Reader, experiment_id: &str) -> Result<Value, String> {
    let Some(mut evidence) = rows_of_run(reader, SUMMARY, experiment_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(Value::Null);
    };
    let (samples, total) = probe_samples(reader, experiment_id)?;
    evidence["steps"] = rows_of_run(reader, STEPS, experiment_id, MAX_ROWS)?.into();
    evidence["objectives"] = rows_of_run(reader, OBJECTIVES, experiment_id, MAX_ROWS)?.into();
    evidence["load"] = load(reader, experiment_id)?;
    evidence["probe_samples"] = samples.into();
    evidence["probe_samples_total"] = total;
    Ok(evidence)
}
