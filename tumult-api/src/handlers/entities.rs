//! `GET /api/entities` and `GET /api/entities/{key}/history`.
//!
//! Both read what entity events left in the store (`entity_versions` and
//! the `entities_current` view). An entity belongs to the environment its
//! sender named, and a scoped principal reads only the entities of its
//! environments; an entity that names none is outside every scope. Every
//! value a request supplies reaches the store as a bound parameter.
//!
//! Both answer one page at a time (see [`page`]): `total` is the number of
//! rows the request matches, `count` the number returned, `truncated`
//! whether rows were left out, and `next_cursor` where to go on.

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

mod page;

use page::{cut, decode_cursor, encode_cursor, row_limit, Page, MAX_PAGE_BYTES};

/// The values `liveness` takes in `entities_current`.
const LIVENESS: [&str; 3] = ["live", "expired", "gone"];

/// The columns of one entity row. The two JSON columns are read as text
/// and parsed by [`with_json_columns`].
const COLUMNS: &str = "entity_key, environment, entity_type, entity_id, \
     CAST(description AS VARCHAR) AS description, \
     CAST(relationships AS VARCHAR) AS relationships, \
     valid_from_ns, last_seen_ns, last_received_ns, interval_s, deleted_at_ns, \
     delete_reason, \
     wire_form, observer, schema_url";

/// When an entity was last heard of: its last report or its deletion.
const LAST_ACTIVITY: &str = "greatest(last_seen_ns, coalesce(deleted_at_ns, last_seen_ns))";

/// The filters of the entity list over `$1` to `$4`: type, liveness, start
/// of the range, scopes.
fn list_filters() -> String {
    format!(
        "(CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) \
         AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) \
         AND {LAST_ACTIVITY} >= CAST($3 AS BIGINT) AND {}",
        in_scopes(4, "environment")
    )
}

/// The rows of the entity list after the position in `$5` to `$7`, in the
/// order of the listing: last heard of first, then key, then environment.
const LIST_AFTER: &str = "(CAST($5 AS BIGINT) IS NULL \
     OR last_activity_ns < CAST($5 AS BIGINT) \
     OR (last_activity_ns = CAST($5 AS BIGINT) \
         AND (entity_key > CAST($6 AS VARCHAR) \
              OR (entity_key = CAST($6 AS VARCHAR) \
                  AND environment > CAST($7 AS VARCHAR)))))";

/// The filters of the history over `$1` to `$3`: key, scopes, environment.
fn history_filters() -> String {
    format!(
        "entity_key = CAST($1 AS VARCHAR) AND {} \
         AND (CAST($3 AS VARCHAR) IS NULL OR environment = CAST($3 AS VARCHAR))",
        in_scopes(2, "environment")
    )
}

/// The rows of the history after the position in `$4` and `$5`, in its
/// order: newest first, then environment.
const HISTORY_AFTER: &str = "(CAST($4 AS BIGINT) IS NULL \
     OR valid_from_ns < CAST($4 AS BIGINT) \
     OR (valid_from_ns = CAST($4 AS BIGINT) \
         AND environment > CAST($5 AS VARCHAR)))";

#[derive(Deserialize)]
pub(crate) struct EntitiesParams {
    #[serde(rename = "type")]
    entity_type: Option<String>,
    liveness: Option<String>,
    range: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct HistoryParams {
    environment: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

/// Where a page of the entity list ended: last activity, key, environment.
type ListPosition = (i64, String, String);

/// Where a page of the history ended: start of the version, environment.
type HistoryPosition = (i64, String);

fn bad_request(msg: String) -> ApiError {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg})))
        .into_response()
        .into()
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

/// The parameters of [`LIST_AFTER`]: nulls and empty texts for no cursor.
fn list_position_params(position: Option<ListPosition>) -> [SqlParam; 3] {
    let Some((activity_ns, key, environment)) = position else {
        return [
            SqlParam::Null,
            SqlParam::Text(String::new()),
            SqlParam::Text(String::new()),
        ];
    };
    [
        SqlParam::Int(activity_ns),
        SqlParam::Text(key),
        SqlParam::Text(environment),
    ]
}

/// The parameters of [`HISTORY_AFTER`].
fn history_position_params(position: Option<HistoryPosition>) -> [SqlParam; 2] {
    let Some((valid_from_ns, environment)) = position else {
        return [SqlParam::Null, SqlParam::Text(String::new())];
    };
    [SqlParam::Int(valid_from_ns), SqlParam::Text(environment)]
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

/// One page of `sql`, which asks for one row more than `limit`.
fn page_of(reader: &Reader, sql: &str, params: &[SqlParam], limit: u32) -> Result<Page, String> {
    let rows = reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())?;
    let page = cut(rows, limit, MAX_PAGE_BYTES);
    Ok(Page {
        rows: with_json_columns(page.rows)?,
        truncated: page.truncated,
    })
}

/// The single number `sql` selects as `total`.
fn total_of(reader: &Reader, sql: &str, params: &[SqlParam]) -> Result<u64, String> {
    let rows = reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .first()
        .and_then(|row| row["total"].as_u64())
        .unwrap_or(0))
}

/// The cursor that continues after the last row of `page`: null when the
/// page left nothing out.
fn next_cursor(page: &Page, position: impl Fn(&Value) -> Option<Value>) -> Result<Value, String> {
    if !page.truncated {
        return Ok(Value::Null);
    }
    let last = page.rows.last().and_then(position);
    let last = last.ok_or("the last row of the page has no position")?;
    Ok(Value::String(encode_cursor(&last)?))
}

fn list_position(row: &Value) -> Option<Value> {
    Some(json!([
        row["last_activity_ns"].as_i64()?,
        row["entity_key"].as_str()?,
        row["environment"].as_str()?,
    ]))
}

fn history_position(row: &Value) -> Option<Value> {
    Some(json!([
        row["valid_from_ns"].as_i64()?,
        row["environment"].as_str()?,
    ]))
}

/// The current row of every entity, most recently heard of first (each
/// row says when in `last_activity_ns`). `type` and `liveness` match
/// exactly; `range` keeps the entities last heard of inside it and, left
/// out, keeps them all.
pub(crate) async fn entities(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<EntitiesParams>,
) -> Result<Json<Value>, ApiError> {
    let position: Option<ListPosition> =
        decode_cursor(params.cursor.as_deref()).map_err(bad_request)?;
    let filters = vec![
        text_param(params.entity_type.filter(|s| !s.is_empty())),
        liveness_param(params.liveness).map_err(bad_request)?,
        since_param(params.range.as_deref()).map_err(bad_request)?,
        scopes_param(&principal.env_scopes),
    ];
    let limit = row_limit(params.limit);
    let count_sql = format!(
        "SELECT count(*) AS total FROM entities_current WHERE {}",
        list_filters()
    );
    let page_sql = format!(
        "WITH matched AS (\
             SELECT {COLUMNS}, liveness, {LAST_ACTIVITY} AS last_activity_ns \
             FROM entities_current WHERE {}) \
         SELECT * FROM matched WHERE {LIST_AFTER} \
         ORDER BY last_activity_ns DESC, entity_key, environment LIMIT {}",
        list_filters(),
        u64::from(limit) + 1
    );
    let body = with_reader(&state.db_path, move |reader| {
        let total = total_of(reader, &count_sql, &filters)?;
        let mut bound = filters;
        bound.extend(list_position_params(position));
        let page = page_of(reader, &page_sql, &bound, limit)?;
        Ok(json!({
            "count": page.rows.len(),
            "total": total,
            "truncated": page.truncated,
            "next_cursor": next_cursor(&page, list_position)?,
            "entities": page.rows,
        }))
    })
    .await?;
    Ok(Json(body))
}

fn not_found() -> ApiError {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "entity not found"})),
    )
        .into_response()
        .into()
}

/// The stored versions of one entity, newest first, each with the
/// environment it belongs to. `environments` counts the versions per
/// environment the principal may read; `environment` keeps the versions of
/// one. An entity the principal may not read is not found, the same as one
/// that never was.
pub(crate) async fn entity_history(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(key): Path<String>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let position: Option<HistoryPosition> =
        decode_cursor(params.cursor.as_deref()).map_err(bad_request)?;
    let filters = vec![
        SqlParam::Text(key.clone()),
        scopes_param(&principal.env_scopes),
        text_param(params.environment),
    ];
    let limit = row_limit(params.limit);
    let environments_sql = format!(
        "SELECT environment, count(*) AS versions FROM entity_versions \
         WHERE entity_key = CAST($1 AS VARCHAR) AND {} \
         GROUP BY environment ORDER BY environment",
        in_scopes(2, "environment")
    );
    let count_sql = format!(
        "SELECT count(*) AS total FROM entity_versions WHERE {}",
        history_filters()
    );
    let page_sql = format!(
        "SELECT {COLUMNS} FROM entity_versions WHERE {} AND {HISTORY_AFTER} \
         ORDER BY valid_from_ns DESC, environment LIMIT {}",
        history_filters(),
        u64::from(limit) + 1
    );
    let body = with_reader(&state.db_path, move |reader| {
        let environments = reader
            .query_json_rows_bound(&environments_sql, &filters[..2])
            .map_err(|e| e.to_string())?;
        if environments.is_empty() {
            return Ok(None);
        }
        let total = total_of(reader, &count_sql, &filters)?;
        let mut bound = filters;
        bound.extend(history_position_params(position));
        let page = page_of(reader, &page_sql, &bound, limit)?;
        Ok(Some(json!({
            "entity_key": key,
            "count": page.rows.len(),
            "total": total,
            "truncated": page.truncated,
            "next_cursor": next_cursor(&page, history_position)?,
            "environments": environments,
            "versions": page.rows,
        })))
    })
    .await?;
    body.map(Json).ok_or_else(not_found)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn no_cursor_binds_a_null_position() {
        assert_eq!(list_position_params(None)[0], SqlParam::Null);
        assert_eq!(history_position_params(None)[0], SqlParam::Null);
        assert_eq!(
            list_position_params(Some((7, "k".into(), "e".into()))),
            [
                SqlParam::Int(7),
                SqlParam::Text("k".into()),
                SqlParam::Text("e".into())
            ]
        );
        assert_eq!(
            history_position_params(Some((7, "e".into()))),
            [SqlParam::Int(7), SqlParam::Text("e".into())]
        );
    }

    #[test]
    fn a_page_that_left_nothing_out_has_no_cursor() {
        let whole = Page {
            rows: vec![json!({"valid_from_ns": 1, "environment": "prod"})],
            truncated: false,
        };
        assert_eq!(next_cursor(&whole, history_position), Ok(Value::Null));
        let part = Page {
            truncated: true,
            ..whole
        };
        let cursor = next_cursor(&part, history_position).unwrap();
        let back: Option<HistoryPosition> = decode_cursor(cursor.as_str()).unwrap();
        assert_eq!(back, Some((1, "prod".to_owned())));
        assert!(next_cursor(&part, list_position).is_err());
    }
}
