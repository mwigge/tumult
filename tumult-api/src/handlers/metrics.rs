//! `GET /api/timeseries`, `GET /api/metrics` and the raw-metric explorer
//! (`/api/metrics/catalog`, `/api/metrics/query`).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::Reader;

use crate::auth::Principal;
use crate::sql_util::{
    and_pred, bucket_expr, env_metric_exists, env_table_predicate, internal, parse_interval,
    sql_string, w, windows, with_reader,
};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/timeseries

#[derive(Deserialize)]
pub(crate) struct TimeseriesParams {
    metric: Option<String>,
    interval: Option<String>,
    range: Option<String>,
}

pub(crate) async fn timeseries(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<TimeseriesParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Some(metric) = params.metric.filter(|m| !m.is_empty()) else {
        return Err(bad("missing query parameter: metric".into()));
    };
    let interval = params.interval.unwrap_or_else(|| "1h".into());
    let Some(bucket_s) = parse_interval(&interval) else {
        return Err(bad(format!(
            "invalid interval {interval:?}; expected 5m|1h|1d"
        )));
    };
    let range = params.range.unwrap_or_else(|| "24h".into());
    let Some((cur, _)) = windows(&range) else {
        return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
    };

    let metrics_dir = state.metrics_dir.as_ref().clone();
    let defs = tumult_metrics::load_dir(&metrics_dir)
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let Some(def) = defs.iter().find(|d| d.name == metric) else {
        let available = defs
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {metric:?} not found; available: {available}")})),
        )
            .into_response());
    };
    let sql =
        tumult_metrics::to_sql_bucketed(def, bucket_s * 1_000_000_000, &[], Some((cur.0, cur.1)))
            .map_err(|e| internal(e.to_string()))?;
    // Scope the source rows before aggregation: the SQL generator has no
    // predicate hook, so the table reference becomes a filtered subquery
    // (the source table is a validated `[a-z0-9_.]` identifier).
    let sql = match env_table_predicate(&def.source_table, &principal.env_scopes) {
        Some(pred) => sql.replacen(
            &format!("FROM {}", def.source_table),
            &format!("FROM (SELECT * FROM {} WHERE {pred})", def.source_table),
            1,
        ),
        None => sql,
    };
    let description = def.description.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;
        Ok(json!({
            "metric": metric,
            "description": description,
            "interval": interval,
            "range": range,
            "points": rows,
        }))
    })
    .await?;
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /api/metrics

pub(crate) async fn list_metrics(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let defs = tumult_metrics::load_dir(state.metrics_dir.as_ref())
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let metrics: Vec<Value> = defs
        .iter()
        .map(|d| json!({"name": d.name, "description": d.description}))
        .collect();
    Ok(Json(json!({"metrics": metrics})))
}

// ---------------------------------------------------------------------------
// GET /api/metrics/catalog + /api/metrics/query

/// Raw metric tables and their UI-facing type names, in preference order
/// (a name reused across tables resolves to the first match).
const METRIC_TABLES: [(&str, &str); 3] = [
    ("metric_sums", "sum"),
    ("metric_gauges", "gauge"),
    ("metric_histograms", "histogram"),
];

/// Catalog of raw metrics: name → table types it appears in + attribute
/// keys seen on its points (sampled, capped per table). Scoped principals
/// see only metrics of experiments inside their environments.
fn load_catalog(reader: &Reader, scopes: &[String]) -> Result<Vec<Value>, String> {
    let mut names: std::collections::BTreeMap<
        String,
        (Vec<String>, std::collections::BTreeSet<String>),
    > = std::collections::BTreeMap::new();
    for (table, kind) in METRIC_TABLES {
        let env = env_metric_exists(table, scopes)
            .map_or_else(String::new, |pred| format!(" WHERE {pred}"));
        let rows = reader
            .query_json_rows(&format!(
                "SELECT metric_name, map_keys(attrs) AS k FROM {table}{env} LIMIT 5000"
            ))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let Some(name) = row.get("metric_name").and_then(Value::as_str) else {
                continue;
            };
            let entry = names.entry(name.to_string()).or_default();
            let kind = kind.to_string();
            if !entry.0.contains(&kind) {
                entry.0.push(kind);
            }
            if let Some(keys) = row.get("k").and_then(Value::as_array) {
                for key in keys.iter().filter_map(Value::as_str) {
                    entry.1.insert(key.to_string());
                }
            }
        }
    }
    Ok(names
        .into_iter()
        .map(|(name, (types, dims))| {
            json!({
                "name": name,
                "types": types,
                "dimensions": dims.into_iter().collect::<Vec<_>>(),
            })
        })
        .collect())
}

pub(crate) async fn metrics_catalog(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, Response> {
    let scopes = principal.env_scopes.clone();
    let metrics = with_reader(&state.db_path, move |reader| load_catalog(reader, &scopes)).await?;
    Ok(Json(json!({"metrics": metrics})))
}

/// Attribute keys may become part of a SQL expression, so they get the
/// same strict charset as metric identifiers (the metrics crate's own
/// check is private).
pub fn valid_attr_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Approximate a quantile from histogram bucket counts with linear
/// interpolation inside the target bucket. `bounds` has one fewer entry
/// than `counts` (OTel explicit bounds); a quantile landing in the
/// overflow bucket clamps to the last explicit bound.
pub fn hist_quantile(counts: &[f64], bounds: &[f64], q: f64) -> Option<f64> {
    let total: f64 = counts.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let target = q * total;
    let mut cum = 0.0;
    for (i, &c) in counts.iter().enumerate() {
        if c <= 0.0 {
            continue;
        }
        if cum + c >= target {
            let lo = if i == 0 {
                0.0
            } else {
                bounds.get(i - 1).copied().unwrap_or(0.0)
            };
            let hi = bounds
                .get(i)
                .copied()
                .unwrap_or_else(|| bounds.last().copied().unwrap_or(lo));
            return Some(lo + (target - cum) / c * (hi - lo));
        }
        cum += c;
    }
    bounds.last().copied()
}

#[derive(Deserialize)]
pub(crate) struct MetricQueryParams {
    name: Option<String>,
    group_by: Option<String>,
    range: Option<String>,
    interval: Option<String>,
}

/// Running aggregation for one histogram (bucket, group) cell.
#[derive(Default)]
struct HistAcc {
    count: u64,
    sum: f64,
    buckets: Vec<f64>,
    bounds: Vec<f64>,
}

pub(crate) async fn metrics_query(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<MetricQueryParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Some(name) = params.name.filter(|n| !n.is_empty()) else {
        return Err(bad("missing query parameter: name".into()));
    };
    if name.len() > 200 {
        return Err(bad("metric name too long".into()));
    }
    let interval = params.interval.unwrap_or_else(|| "1h".into());
    let Some(bucket_s) = parse_interval(&interval) else {
        return Err(bad(format!(
            "invalid interval {interval:?}; expected 5m|1h|1d"
        )));
    };
    let range = params.range.unwrap_or_else(|| "24h".into());
    let Some((cur, _)) = windows(&range) else {
        return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
    };
    let group_by = match params.group_by.as_deref().filter(|g| !g.is_empty()) {
        Some(g) if valid_attr_key(g) => Some(g.to_string()),
        Some(g) => {
            return Err(bad(format!(
                "invalid group_by {g:?}; expected lowercase [a-z0-9_.]+"
            )))
        }
        None => None,
    };

    let metric_name = name.clone();
    let scopes = principal.env_scopes.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let catalog = load_catalog(reader, &scopes)?;
        let Some(entry) = catalog
            .iter()
            .find(|m| m.get("name").and_then(Value::as_str) == Some(metric_name.as_str()))
        else {
            return Ok(None);
        };
        let types: Vec<&str> = entry
            .get("types")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some((table, kind)) = METRIC_TABLES.iter().find(|(_, k)| types.contains(k)) else {
            return Ok(None);
        };
        let grp_sel = group_by.as_ref().map_or(String::new(), |g| {
            format!(", attrs[{}] AS grp", sql_string(g))
        });
        let window = w(cur.0, cur.1);
        let name_sql = sql_string(&metric_name);
        let env = and_pred(env_metric_exists(table, &scopes));

        // Pivot (bucket, group) rows into one series per group.
        let mut groups: std::collections::BTreeMap<Option<String>, Vec<Value>> =
            std::collections::BTreeMap::new();
        if *kind == "histogram" {
            // Fetch raw rows and aggregate in Rust: elementwise bucket sums
            // per (bucket, group), then interpolate p95. Bounds are assumed
            // stable per metric — mixed schemas degrade to first-seen.
            let rows = reader
                .query_json_rows(&format!(
                    "SELECT {} AS ts, count, sum, bucket_counts, explicit_bounds{grp_sel} \
                     FROM {table} WHERE metric_name = {name_sql} AND {window}{env} \
                     ORDER BY 1 LIMIT 10000",
                    bucket_expr(bucket_s)
                ))
                .map_err(|e| e.to_string())?;
            let mut cells: std::collections::BTreeMap<(i64, Option<String>), HistAcc> =
                std::collections::BTreeMap::new();
            for row in &rows {
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                let grp = row.get("grp").and_then(Value::as_str).map(str::to_owned);
                let acc = cells.entry((ts, grp)).or_default();
                acc.count += row.get("count").and_then(Value::as_u64).unwrap_or(0);
                acc.sum += row.get("sum").and_then(Value::as_f64).unwrap_or(0.0);
                if let Some(bc) = row.get("bucket_counts").and_then(Value::as_array) {
                    for (i, b) in bc.iter().enumerate() {
                        let v = b.as_f64().unwrap_or(0.0);
                        if i < acc.buckets.len() {
                            acc.buckets[i] += v;
                        } else {
                            acc.buckets.push(v);
                        }
                    }
                }
                if acc.bounds.is_empty() {
                    if let Some(eb) = row.get("explicit_bounds").and_then(Value::as_array) {
                        acc.bounds = eb.iter().filter_map(Value::as_f64).collect();
                    }
                }
            }
            for ((ts, grp), acc) in cells {
                if acc.count == 0 {
                    continue;
                }
                groups.entry(grp).or_default().push(json!({
                    "ts": ts,
                    "avg": acc.sum / acc.count as f64,
                    "p95": hist_quantile(&acc.buckets, &acc.bounds, 0.95),
                }));
            }
        } else {
            let agg = if *kind == "sum" {
                "SUM(value)"
            } else {
                "AVG(value)"
            };
            let grp_by = if group_by.is_some() { ", 3" } else { "" };
            let rows = reader
                .query_json_rows(&format!(
                    "SELECT {} AS ts, {agg} AS v{grp_sel} FROM {table} \
                     WHERE metric_name = {name_sql} AND {window}{env} GROUP BY 1{grp_by} ORDER BY 1",
                    bucket_expr(bucket_s)
                ))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                let grp = row.get("grp").and_then(Value::as_str).map(str::to_owned);
                groups.entry(grp).or_default().push(json!({
                    "ts": ts,
                    "v": row.get("v").cloned().unwrap_or(Value::Null),
                }));
            }
        }

        let series: Vec<Value> = groups
            .into_iter()
            .map(|(grp, mut points)| {
                points.sort_by_key(|p| p.get("ts").and_then(Value::as_i64).unwrap_or(0));
                json!({"group": grp, "points": points})
            })
            .collect();
        Ok(Some(json!({
            "name": metric_name,
            "type": kind,
            "interval": interval,
            "range": range,
            "group_by": group_by,
            "series": series,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {name:?} not found; see /api/metrics/catalog")})),
        )
            .into_response()),
    }
}
