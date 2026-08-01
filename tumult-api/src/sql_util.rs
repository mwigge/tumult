//! Shared helpers: time windows, SQL quoting/predicates, per-user
//! environment scoping, the read-only-reader wrapper and small row→JSON
//! reducers used by the query handlers.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tumult_lake::{Reader, Store};

// ---------------------------------------------------------------------------
// helpers

/// Current time as epoch nanoseconds.
pub(crate) fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// `24h` / `7d` / `14d` → seconds.
pub(crate) fn parse_range(range: &str) -> Option<i64> {
    match range {
        "24h" => Some(86_400),
        "7d" => Some(7 * 86_400),
        "14d" => Some(14 * 86_400),
        _ => None,
    }
}

/// `5m` / `1h` / `1d` → seconds.
pub(crate) fn parse_interval(interval: &str) -> Option<i64> {
    match interval {
        "5m" => Some(300),
        "1h" => Some(3_600),
        "1d" => Some(86_400),
        _ => None,
    }
}

/// Window `[from, to)` for `range` ending at now, and the previous equal
/// window before it.
pub(crate) fn windows(range: &str) -> Option<((i64, i64), (i64, i64))> {
    let secs = parse_range(range)?;
    let to = now_ns();
    let from = to - secs * 1_000_000_000;
    Some(((from, to), (from - secs * 1_000_000_000, from)))
}

/// Quote a user-supplied string as a SQL string literal.
pub(crate) fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Quote a user-supplied substring for `ILIKE … ESCAPE '\'` (contains-match).
pub(crate) fn sql_contains(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('\'', "''");
    format!("'%{esc}%'")
}

/// Quote a user-supplied string as an `ILIKE` pattern with no wildcards —
/// a case-insensitive exact match (`%`/`_` escaped, use with `ESCAPE '\'`).
pub(crate) fn sql_ieq(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('\'', "''");
    format!("'{esc}'")
}

/// Time-window predicate on `ts_ns`.
pub(crate) fn w(from: i64, to: i64) -> String {
    format!("ts_ns >= {from} AND ts_ns < {to}")
}

/// `col IN ('a', 'b')` predicate restricting reads to a principal's
/// environment scopes, or `None` when the set is empty (all environments —
/// also the synthetic open-auth principal's case). Scoping is read-side
/// filtering only; mutations are role-gated by the auth middleware.
pub(crate) fn env_scope_where(col: &str, scopes: &[String]) -> Option<String> {
    if scopes.is_empty() {
        return None;
    }
    let list = scopes
        .iter()
        .map(|s| sql_string(s))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{col} IN ({list})"))
}

/// `" AND <predicate>"` suffix for a WHERE clause, or empty when the
/// principal is unscoped (the predicate is `None`).
pub(crate) fn and_pred(pred: Option<String>) -> String {
    pred.map_or_else(String::new, |p| format!(" AND {p}"))
}

/// EXISTS predicate scoping spans/logs rows to an in-scope experiment
/// trace: `target_environment` lives only on the root span, so child spans
/// and log rows reach it through `trace_id` correlation. Rows with no
/// in-scope linkage are hidden from scoped principals (fail closed).
pub(crate) fn env_trace_exists(alias: &str, scopes: &[String]) -> Option<String> {
    env_scope_where("se.target_environment", scopes).map(|env| {
        format!("EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = {alias}.trace_id AND {env})")
    })
}

/// Environment predicate for one of the `metric_*` tables: metric points
/// carry no environment column, so they reach the root span's
/// `target_environment` through `experiment_name` correlation.
pub(crate) fn env_metric_exists(table: &str, scopes: &[String]) -> Option<String> {
    env_scope_where("se.target_environment", scopes).map(|env| {
        format!(
            "EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = {table}.experiment_name AND {env})"
        )
    })
}

/// Environment predicate for a metric definition's source table: `spans`
/// binds its own column, the metric tables bind through `experiment_name`.
pub(crate) fn env_table_predicate(table: &str, scopes: &[String]) -> Option<String> {
    if table == "spans" {
        env_scope_where("target_environment", scopes)
    } else {
        env_metric_exists(table, scopes)
    }
}

/// Names of the experiments visible under the scopes (automated and manual
/// evidence); `None` when the principal is unscoped.
pub(crate) fn scoped_experiment_names(
    reader: &Reader,
    scopes: &[String],
) -> Result<Option<std::collections::HashSet<String>>, String> {
    let Some(env) = env_scope_where("target_environment", scopes) else {
        return Ok(None);
    };
    let rows = reader
        .query_json_rows(&format!(
            "SELECT experiment_name AS name FROM spans \
             WHERE span_name = 'resilience.experiment' AND {env} \
             UNION SELECT experiment_name AS name FROM manual_experiments WHERE {env}"
        ))
        .map_err(|e| e.to_string())?;
    Ok(Some(
        rows.iter()
            .filter_map(|r| r.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect(),
    ))
}

/// Restrict a scorecard to in-scope experiments, recomputing the target and
/// portfolio rollups with scoring's equal-weight algorithm (the scoring
/// crate has no scope hook, so the rollup is replayed here).
pub(crate) fn scope_scorecard(
    reader: &Reader,
    mut card: tumult_compliance::scoring::Scorecard,
    scopes: &[String],
) -> Result<tumult_compliance::scoring::Scorecard, String> {
    let Some(in_scope) = scoped_experiment_names(reader, scopes)? else {
        return Ok(card);
    };
    card.experiments.retain(|e| in_scope.contains(&e.name));
    let mut by_target: std::collections::BTreeMap<
        String,
        Vec<&tumult_compliance::scoring::ExperimentScore>,
    > = std::collections::BTreeMap::new();
    for e in &card.experiments {
        by_target
            .entry(e.target.clone().unwrap_or_else(|| "(untargeted)".into()))
            .or_default()
            .push(e);
    }
    let mut targets: Vec<tumult_compliance::scoring::TargetScore> = by_target
        .into_iter()
        .map(|(target, exps)| {
            let score = exps.iter().map(|e| f64::from(e.score)).sum::<f64>() / exps.len() as f64;
            tumult_compliance::scoring::TargetScore {
                target,
                score,
                band: tumult_compliance::scoring::band(score).to_string(),
                runs: exps.iter().map(|e| e.runs).sum(),
                last_run_ns: exps.iter().filter_map(|e| e.last_run_ns).max(),
            }
        })
        .collect();
    targets.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    card.portfolio = if targets.is_empty() {
        0.0
    } else {
        targets.iter().map(|t| t.score).sum::<f64>() / targets.len() as f64
    };
    card.band = tumult_compliance::scoring::band(card.portfolio).to_string();
    card.targets = targets;
    Ok(card)
}

/// Parse a click-to-filter `k=v` parameter into (key, value); the key must
/// be non-empty (the value may be, to filter for empty attrs).
pub(crate) fn attr_kv(s: &str) -> Option<(&str, &str)> {
    let (k, v) = s.split_once('=')?;
    if k.is_empty() {
        None
    } else {
        Some((k, v))
    }
}

/// Click-to-filter predicates on the attribute maps, for the logs list
/// (`log_attrs` / `resource_attrs` columns) and — via `span_alias` — for the
/// traces list (`EXISTS` over any span of the trace). `attr` keeps only
/// rows where the key has exactly the value in either map; `attr_not`
/// excludes them (NULL-safe: rows lacking the key survive `attr_not`).
pub(crate) fn attr_wheres(
    attr: Option<&str>,
    attr_not: Option<&str>,
    key_cols: (&str, &str),
) -> Result<Vec<String>, String> {
    let (a, b) = key_cols;
    let mut wheres = Vec::new();
    if let Some(raw) = attr.filter(|s| !s.is_empty()) {
        let Some((k, v)) = attr_kv(raw) else {
            return Err(format!("invalid attr {raw:?}; expected k=v"));
        };
        wheres.push(format!(
            "({a}[{}] = {v} OR {b}[{}] = {v})",
            sql_string(k),
            sql_string(k),
            v = sql_string(v)
        ));
    }
    if let Some(raw) = attr_not.filter(|s| !s.is_empty()) {
        let Some((k, v)) = attr_kv(raw) else {
            return Err(format!("invalid attr_not {raw:?}; expected k=v"));
        };
        wheres.push(format!(
            "NOT (COALESCE({a}[{}] = {v}, false) OR COALESCE({b}[{}] = {v}, false))",
            sql_string(k),
            sql_string(k),
            v = sql_string(v)
        ));
    }
    Ok(wheres)
}

/// 500 JSON error response. The full error is logged server-side — store
/// errors carry schema, file paths and internal state that must not reach
/// clients, so the body is a fixed generic message.
pub fn internal(msg: String) -> Response {
    tracing::error!(error = %msg, "internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal error"})),
    )
        .into_response()
}

/// Run `f` with a fresh read-only reader on a blocking thread; map any
/// failure to a 500 JSON error (details logged, never returned).
pub(crate) async fn with_reader<T>(
    db_path: &std::path::Path,
    f: impl FnOnce(&Reader) -> Result<T, String> + Send + 'static,
) -> Result<T, Response>
where
    T: Send + 'static,
{
    let db = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let store = Store::at(&db);
        let reader = store.read_only().map_err(|e| e.to_string())?;
        f(&reader)
    })
    .await
    .map_err(|e| internal(format!("query task failed: {e}")))?
    .map_err(internal)
}

/// First row's `v` column as `f64` (`None` when no rows or NULL).
pub(crate) fn scalar(reader: &Reader, sql: &str) -> Result<Option<f64>, String> {
    let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .first()
        .and_then(|r| r.get("v"))
        .and_then(Value::as_f64))
}

/// `{ts, v}` rows as a JSON array (bucketed series).
pub(crate) fn series(reader: &Reader, sql: &str) -> Result<Vec<Value>, String> {
    let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let ts = r.get("ts")?.as_i64()?;
            let v = r.get("v").and_then(Value::as_f64)?;
            Some(json!({"ts": ts, "v": v}))
        })
        .collect())
}

/// Divide two aligned bucketed series pointwise (missing numerator → 0,
/// missing/zero denominator → point dropped).
pub(crate) fn ratio_series(num: &[Value], den: &[Value]) -> Vec<Value> {
    let den: HashMap<i64, f64> = den
        .iter()
        .filter_map(|r| Some((r.get("ts")?.as_i64()?, r.get("v")?.as_f64()?)))
        .collect();
    let mut out: Vec<Value> = den
        .iter()
        .filter(|(_, d)| **d > 0.0)
        .map(|(ts, d)| {
            let n = num
                .iter()
                .find(|r| r.get("ts").and_then(Value::as_i64) == Some(*ts))
                .and_then(|r| r.get("v").and_then(Value::as_f64))
                .unwrap_or(0.0);
            json!({"ts": ts, "v": n / d})
        })
        .collect();
    out.sort_by_key(|r| r.get("ts").and_then(Value::as_i64).unwrap_or(0));
    out
}

pub(crate) fn bucket_expr(bucket_s: i64) -> String {
    let ns = bucket_s * 1_000_000_000;
    format!("(ts_ns // {ns}) * {ns} // 1000000000")
}
