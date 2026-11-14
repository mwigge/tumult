//! `GET /api/entities` and `GET /api/entities/{key}/history`.
//!
//! Both read what entity events left in the store (`entity_versions` and
//! the `entities_current` view). An entity belongs to the environment its
//! sender named, and a scoped principal reads only the entities of its
//! environments; an entity that names none is outside every scope. Every
//! value a request supplies reaches the store as a bound parameter.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::{Reader, SqlParam};

use crate::auth::Principal;
use crate::error::ApiError;
use crate::sql_util::{in_scopes, invalid_range, scopes_param, windows, with_reader};
use crate::ApiState;

/// Rows returned when the request names no `limit`.
const DEFAULT_LIMIT: u32 = 200;
/// The most rows one request returns.
const MAX_LIMIT: u32 = 1000;

/// The values `liveness` takes in `entities_current`.
const LIVENESS: [&str; 3] = ["live", "expired", "gone"];

/// The columns of one entity row. The two JSON columns are read as text
/// and parsed by [`with_json_columns`].
const COLUMNS: &str = "entity_key, environment, entity_type, entity_id, \
     CAST(description AS VARCHAR) AS description, \
     CAST(relationships AS VARCHAR) AS relationships, \
     valid_from_ns, last_seen_ns, interval_s, deleted_at_ns, delete_reason, \
     wire_form, observer, schema_url";

/// When an entity was last heard of: its last report or its deletion.
const LAST_ACTIVITY: &str = "greatest(last_seen_ns, coalesce(deleted_at_ns, last_seen_ns))";

#[derive(Deserialize)]
pub(crate) struct EntitiesParams {
    #[serde(rename = "type")]
    entity_type: Option<String>,
    liveness: Option<String>,
    range: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct HistoryParams {
    limit: Option<u32>,
}

fn bad_request(msg: String) -> ApiError {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg})))
        .into_response()
        .into()
}

fn row_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn text_param(value: Option<String>) -> SqlParam {
    value.map_or(SqlParam::Null, SqlParam::Text)
}

/// The `liveness` filter as a parameter: null when the request names none.
fn liveness_param(liveness: Option<String>) -> Result<SqlParam, String> {
    let Some(liveness) = liveness.filter(|s| !s.is_empty()) else {
        return Ok(SqlParam::Null);
    };
    if !LIVENESS.contains(&liveness.as_str()) {
        return Err(format!(
            "invalid liveness {liveness:?}; expected {}",
            LIVENESS.join("|")
        ));
    }
    Ok(SqlParam::Text(liveness))
}

/// The start of `range` as a parameter: the beginning of time when the
/// request names none.
fn since_param(range: Option<&str>) -> Result<SqlParam, String> {
    let Some(range) = range.filter(|s| !s.is_empty()) else {
        return Ok(SqlParam::Int(i64::MIN));
    };
    let Some(((from, _), _)) = windows(range) else {
        return Err(invalid_range(range));
    };
    Ok(SqlParam::Int(from))
}

/// Replace the text of the `description` and `relationships` columns with
/// the JSON it holds.
fn with_json_columns(mut rows: Vec<Value>) -> Result<Vec<Value>, String> {
    for row in &mut rows {
        for column in ["description", "relationships"] {
            let Some(text) = row.get(column).and_then(Value::as_str) else {
                continue;
            };
            let parsed: Value = serde_json::from_str(text)
                .map_err(|e| format!("entity column {column} is not JSON: {e}"))?;
            row[column] = parsed;
        }
    }
    Ok(rows)
}

fn entity_rows(reader: &Reader, sql: &str, params: &[SqlParam]) -> Result<Vec<Value>, String> {
    let rows = reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())?;
    with_json_columns(rows)
}

/// The current row of every entity, most recently heard of first.
/// `type` and `liveness` match exactly; `range` keeps the entities last
/// heard of inside it and, left out, keeps them all.
pub(crate) async fn entities(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<EntitiesParams>,
) -> Result<Json<Value>, ApiError> {
    let bound = vec![
        text_param(params.entity_type.filter(|s| !s.is_empty())),
        liveness_param(params.liveness).map_err(bad_request)?,
        since_param(params.range.as_deref()).map_err(bad_request)?,
        scopes_param(&principal.env_scopes),
    ];
    let sql = format!(
        "SELECT {COLUMNS}, liveness FROM entities_current \
         WHERE (CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) \
         AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) \
         AND {LAST_ACTIVITY} >= CAST($3 AS BIGINT) AND {} \
         ORDER BY {LAST_ACTIVITY} DESC, entity_key, environment LIMIT {}",
        in_scopes(4, "environment"),
        row_limit(params.limit)
    );
    let rows = with_reader(&state.db_path, move |reader| {
        entity_rows(reader, &sql, &bound)
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "entities": rows})))
}

/// Every stored version of one entity, newest first. An entity the
/// principal may not read is not found, the same as one that never was.
pub(crate) async fn entity_history(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(key): Path<String>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let bound = vec![
        SqlParam::Text(key.clone()),
        scopes_param(&principal.env_scopes),
    ];
    let sql = format!(
        "SELECT {COLUMNS} FROM entity_versions \
         WHERE entity_key = CAST($1 AS VARCHAR) AND {} \
         ORDER BY valid_from_ns DESC, environment LIMIT {}",
        in_scopes(2, "environment"),
        row_limit(params.limit)
    );
    let rows = with_reader(&state.db_path, move |reader| {
        entity_rows(reader, &sql, &bound)
    })
    .await?;
    if rows.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "entity not found"})),
        )
            .into_response()
            .into());
    }
    Ok(Json(
        json!({"entity_key": key, "count": rows.len(), "versions": rows}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_two_hundred_by_default_and_at_most_a_thousand() {
        assert_eq!(row_limit(None), 200);
        assert_eq!(row_limit(Some(0)), 1);
        assert_eq!(row_limit(Some(7)), 7);
        assert_eq!(row_limit(Some(u32::MAX)), 1000);
    }

    #[test]
    fn an_absent_or_empty_filter_binds_null() {
        assert_eq!(liveness_param(None), Ok(SqlParam::Null));
        assert_eq!(liveness_param(Some(String::new())), Ok(SqlParam::Null));
        assert_eq!(
            liveness_param(Some("gone".into())),
            Ok(SqlParam::Text("gone".into()))
        );
        assert!(liveness_param(Some("LIVE".into())).is_err());
        assert_eq!(since_param(None), Ok(SqlParam::Int(i64::MIN)));
        assert_eq!(since_param(Some("")), Ok(SqlParam::Int(i64::MIN)));
        assert!(since_param(Some("1y")).is_err());
    }

    #[test]
    fn json_columns_are_parsed_and_a_broken_one_is_an_error() {
        let rows = vec![json!({"description": "{\"a\":1}", "relationships": null})];
        let parsed = with_json_columns(rows).unwrap();
        assert_eq!(parsed[0]["description"], json!({"a": 1}));
        assert_eq!(parsed[0]["relationships"], Value::Null);
        assert!(with_json_columns(vec![json!({"description": "{"})]).is_err());
    }
}
