//! `GET /api/entities` and `GET /api/entities/{key}/history`.
//!
//! Both read what entity events left in the store (`entity_versions` and
//! the `entities_current` view). An entity belongs to the environment its
//! sender named, and a scoped principal reads only the entities of its
//! environments; an entity that names none is outside every scope. Every
//! value a request supplies reaches the store as a bound parameter.
//!
//! Both answer one page at a time (see `page`): `total` is the number of
//! rows the request matches, `count` the number returned, `truncated`
//! whether rows were left out, and `next_cursor` where to go on.
//!
//! `DELETE /api/entities?environment=…[&type=…]` removes the entities of
//! one environment with all their rows (`purge_entities`). It needs the
//! Admin role, runs on the daemon's single writer and is written to the
//! log there; a purge that is refused or fails is written to the log too.

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::{Reader, SqlParam};

use crate::auth::Principal;
use crate::error::{forbidden, unavailable, ApiError};
use crate::sql_util::{in_scopes, internal, invalid_range, scopes_param, windows, with_reader};
use crate::ApiState;

pub(crate) mod page;

use page::{decode_cursor, encode_cursor, page_sql, paged, row_limit, Page, MAX_PAGE_BYTES};

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

/// The filters of the entity list over `$1` to `$4`: type, liveness, start
/// of the range, scopes. The range is measured on the daemon's clock, as
/// liveness is: on when the entity was last received.
fn list_filters() -> String {
    format!(
        "(CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) \
         AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) \
         AND last_received_ns >= CAST($3 AS BIGINT) AND {}",
        in_scopes(4, "environment")
    )
}

/// The order of the entity list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    /// By key, then environment. Nothing an entity reports moves it, so a
    /// walk with the cursor meets every entity that exists throughout the
    /// walk exactly once.
    Key,
    /// Last received first. An entity that reports during a walk moves to
    /// the front and is not met again by a walk already past it.
    Recent,
}

impl Sort {
    fn parse(sort: Option<&str>) -> Result<Self, String> {
        match sort.unwrap_or_default() {
            "" | "key" => Ok(Self::Key),
            "recent" => Ok(Self::Recent),
            other => Err(format!("invalid sort {other:?}; expected key|recent")),
        }
    }

    fn order(self) -> &'static str {
        match self {
            Self::Key => "entity_key, environment",
            Self::Recent => "last_received_ns DESC, entity_key, environment",
        }
    }

    /// The rows after the position bound from `$5` on.
    fn after(self) -> &'static str {
        match self {
            Self::Key => {
                "(CAST($5 AS VARCHAR) IS NULL \
                 OR entity_key > CAST($5 AS VARCHAR) \
                 OR (entity_key = CAST($5 AS VARCHAR) \
                     AND environment > CAST($6 AS VARCHAR)))"
            }
            Self::Recent => {
                "(CAST($5 AS BIGINT) IS NULL \
                 OR last_received_ns < CAST($5 AS BIGINT) \
                 OR (last_received_ns = CAST($5 AS BIGINT) \
                     AND (entity_key > CAST($6 AS VARCHAR) \
                          OR (entity_key = CAST($6 AS VARCHAR) \
                              AND environment > CAST($7 AS VARCHAR)))))"
            }
        }
    }

    /// The parameters of [`Self::after`] for `cursor`, which must be one
    /// this order gave out.
    fn position_params(self, cursor: Option<&str>) -> Result<Vec<SqlParam>, String> {
        match self {
            Self::Key => Ok(key_position_params(decode_cursor(cursor)?).into()),
            Self::Recent => Ok(recent_position_params(decode_cursor(cursor)?).into()),
        }
    }

    /// Where `row` stands in this order, for the cursor.
    fn position(self, row: &Value) -> Option<Value> {
        let key = row["entity_key"].as_str()?;
        let environment = row["environment"].as_str()?;
        match self {
            Self::Key => Some(json!([key, environment])),
            Self::Recent => Some(json!([row["last_received_ns"].as_i64()?, key, environment])),
        }
    }
}

/// The filters of the history over `$1` to `$3`: key, scopes, environment.
fn history_filters() -> String {
    format!(
        "entity_key = CAST($1 AS VARCHAR) AND {} \
         AND (CAST($3 AS VARCHAR) IS NULL OR environment = CAST($3 AS VARCHAR))",
        in_scopes(2, "environment")
    )
}

/// The order of the history: newest first, then environment. A stored
/// version never changes its place in it.
const HISTORY_ORDER: &str = "valid_from_ns DESC, environment";

/// The rows of the history after the position in `$4` and `$5`.
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
    sort: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct HistoryParams {
    environment: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

/// Where a page of the entity list by key ended: key, environment.
type KeyPosition = (String, String);

/// Where a page of the entity list by receipt ended: time of receipt, key,
/// environment.
type RecentPosition = (i64, String, String);

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

/// The parameters of the list by key: a null and an empty text for no
/// cursor.
fn key_position_params(position: Option<KeyPosition>) -> [SqlParam; 2] {
    let Some((key, environment)) = position else {
        return [SqlParam::Null, SqlParam::Text(String::new())];
    };
    [SqlParam::Text(key), SqlParam::Text(environment)]
}

/// The parameters of the list by receipt.
fn recent_position_params(position: Option<RecentPosition>) -> [SqlParam; 3] {
    let Some((received_ns, key, environment)) = position else {
        return [
            SqlParam::Null,
            SqlParam::Text(String::new()),
            SqlParam::Text(String::new()),
        ];
    };
    [
        SqlParam::Int(received_ns),
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

/// One page of a statement built by [`page_sql`], which leaves the rows
/// beyond the page in the store.
fn page_of(reader: &Reader, sql: &str, params: &[SqlParam]) -> Result<Page, String> {
    let rows = reader
        .query_json_rows_bound(sql, params)
        .map_err(|e| e.to_string())?;
    let page = paged(rows);
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

fn history_position(row: &Value) -> Option<Value> {
    Some(json!([
        row["valid_from_ns"].as_i64()?,
        row["environment"].as_str()?,
    ]))
}

/// The statement of one page of the entity list in the order of `sort`.
fn list_page_sql(sort: Sort, limit: u32, budget: usize) -> String {
    let matched = format!(
        "SELECT {COLUMNS}, liveness FROM entities_current WHERE {} AND {}",
        list_filters(),
        sort.after()
    );
    page_sql(&matched, sort.order(), limit, budget)
}

/// The statement of one page of the history.
fn history_page_sql(limit: u32, budget: usize) -> String {
    let matched = format!(
        "SELECT {COLUMNS} FROM entity_versions WHERE {} AND {HISTORY_AFTER}",
        history_filters()
    );
    page_sql(&matched, HISTORY_ORDER, limit, budget)
}

/// The current row of every entity. `type` and `liveness` match exactly;
/// `range` keeps the entities last received inside it and, left out,
/// keeps them all. The order is by key and environment, in which a walk
/// with the cursor is complete; `sort=recent` lists the entity received
/// last first, without that promise (see [`Sort`]).
pub(crate) async fn entities(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<EntitiesParams>,
) -> Result<Json<Value>, ApiError> {
    let sort = Sort::parse(params.sort.as_deref()).map_err(bad_request)?;
    let position = sort
        .position_params(params.cursor.as_deref())
        .map_err(bad_request)?;
    let filters = vec![
        text_param(params.entity_type.filter(|s| !s.is_empty())),
        liveness_param(params.liveness).map_err(bad_request)?,
        since_param(params.range.as_deref()).map_err(bad_request)?,
        scopes_param(&principal.env_scopes),
    ];
    let count_sql = format!(
        "SELECT count(*) AS total FROM entities_current WHERE {}",
        list_filters()
    );
    let page_sql = list_page_sql(sort, row_limit(params.limit), MAX_PAGE_BYTES);
    let body = with_reader(&state.db_path, move |reader| {
        let total = total_of(reader, &count_sql, &filters)?;
        let mut bound = filters;
        bound.extend(position);
        let page = page_of(reader, &page_sql, &bound)?;
        Ok(json!({
            "count": page.rows.len(),
            "total": total,
            "truncated": page.truncated,
            "next_cursor": next_cursor(&page, |row| sort.position(row))?,
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
    let page_sql = history_page_sql(row_limit(params.limit), MAX_PAGE_BYTES);
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
        let page = page_of(reader, &page_sql, &bound)?;
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

#[derive(Deserialize, Default)]
pub(crate) struct PurgeParams {
    environment: Option<String>,
    #[serde(rename = "type")]
    entity_type: Option<String>,
}

impl PurgeParams {
    /// The type asked for; `None` for every type.
    fn entity_type(&self) -> Option<&str> {
        self.entity_type.as_deref().filter(|s| !s.is_empty())
    }
}

pub(crate) const INSUFFICIENT_ROLE: &str = "insufficient role";
const NO_ENVIRONMENT: &str = "environment is required";
const OUTSIDE_SCOPES: &str = "the environment is outside the principal's scopes";
const NOT_WIRED: &str = "entity storage is not wired (no ingest handle)";

/// Write to the log that `principal` asked for a purge and was refused.
fn log_refused(principal: &Principal, params: &PurgeParams, reason: &'static str) {
    tumult_ingest::entity_events::purge_refused(
        &principal.username,
        params.environment.as_deref(),
        params.entity_type(),
        reason,
    );
}

/// For the auth middleware, which turns a request away for its role
/// before any handler runs: a purge refused there is written to the log
/// like one refused here. A query that cannot be read names nothing.
pub(crate) fn purge_refused_for_role(method: &str, uri: &Uri, principal: &Principal) {
    if method != "DELETE" || uri.path() != "/api/entities" {
        return;
    }
    let params = Query::<PurgeParams>::try_from_uri(uri)
        .map_or_else(|_| PurgeParams::default(), |Query(params)| params);
    log_refused(principal, &params, INSUFFICIENT_ROLE);
}

/// Whether `principal` may remove the entities of `environment`: every
/// environment for an unscoped principal, its own for a scoped one. The
/// entities that name no environment are outside every scope.
fn may_purge(principal: &Principal, environment: &str) -> bool {
    principal.env_scopes.is_empty() || principal.env_scopes.iter().any(|s| s == environment)
}

/// Remove the entities of `environment`, of one `type` or of all, with
/// their history. `environment` must be given; given empty it names the
/// entities that name no environment. An entity that still reports is
/// stored again with its next report, as a new entity. Answers what was
/// removed: `entities` and `versions` (rows). Needs the Admin role (the
/// route table); a request that is refused or fails is logged as a
/// warning with who asked for what.
pub(crate) async fn purge_entities(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<PurgeParams>,
) -> Result<Json<Value>, ApiError> {
    let Some(environment) = params.environment.clone() else {
        log_refused(&principal, &params, NO_ENVIRONMENT);
        return Err(bad_request(NO_ENVIRONMENT.into()));
    };
    if !may_purge(&principal, &environment) {
        log_refused(&principal, &params, OUTSIDE_SCOPES);
        return Err(forbidden(OUTSIDE_SCOPES));
    }
    let entity_type = params.entity_type().map(str::to_owned);
    let request = tumult_ingest::EntityPurgeRequest {
        environment: environment.clone(),
        entity_type: entity_type.clone(),
        by: principal.username.clone(),
    };
    let Some(ingest) = state.ingest_handle() else {
        tumult_ingest::entity_events::purge_failed(&request, NOT_WIRED);
        return Err(unavailable(NOT_WIRED));
    };
    let purged = ingest
        .purge_entities(request)
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({
        "environment": environment,
        "type": entity_type,
        "entities": purged.entities,
        "versions": purged.versions,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scoped_principal_purges_its_own_environments_only() {
        let mut principal = Principal::synthetic();
        assert!(may_purge(&principal, "prod"));
        assert!(may_purge(&principal, ""));
        principal.env_scopes = vec!["staging".into()];
        assert!(may_purge(&principal, "staging"));
        assert!(!may_purge(&principal, "prod"));
        assert!(!may_purge(&principal, ""));
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

    #[test]
    fn no_cursor_binds_a_null_position() {
        assert_eq!(key_position_params(None)[0], SqlParam::Null);
        assert_eq!(recent_position_params(None)[0], SqlParam::Null);
        assert_eq!(history_position_params(None)[0], SqlParam::Null);
        assert_eq!(
            key_position_params(Some(("k".into(), "e".into()))),
            [SqlParam::Text("k".into()), SqlParam::Text("e".into())]
        );
        assert_eq!(
            recent_position_params(Some((7, "k".into(), "e".into()))),
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
        let no_key = |row: &Value| Sort::Key.position(row);
        assert!(next_cursor(&part, no_key).is_err());
    }

    #[test]
    fn the_sort_is_by_key_unless_recent_is_asked_for() {
        assert_eq!(Sort::parse(None), Ok(Sort::Key));
        assert_eq!(Sort::parse(Some("")), Ok(Sort::Key));
        assert_eq!(Sort::parse(Some("key")), Ok(Sort::Key));
        assert_eq!(Sort::parse(Some("recent")), Ok(Sort::Recent));
        assert!(Sort::parse(Some("Recent")).is_err());
    }

    #[test]
    fn a_position_is_what_the_order_sorts_by() {
        let row = json!({"entity_key": "k", "environment": "e", "last_received_ns": 7});
        assert_eq!(Sort::Key.position(&row), Some(json!(["k", "e"])));
        assert_eq!(Sort::Recent.position(&row), Some(json!([7, "k", "e"])));
        assert_eq!(Sort::Recent.position(&json!({"entity_key": "k"})), None);
        let cursor = encode_cursor(&json!(["k", "e"])).unwrap();
        assert_eq!(
            Sort::Key.position_params(Some(&cursor)),
            Ok(vec![SqlParam::Text("k".into()), SqlParam::Text("e".into())])
        );
        assert_eq!(
            Sort::Recent.position_params(Some(&cursor)),
            Err("invalid cursor".to_owned()),
            "a cursor belongs to the order that gave it"
        );
        assert_eq!(Sort::Recent.position_params(None).map(|p| p.len()), Ok(3));
    }

    /// 2100-01-01: ahead of the clock, so what is stored here is live.
    const FUTURE: i64 = 4_102_444_800_000_000_000;

    fn heavy(id: usize, bytes: usize) -> tumult_lake::EntityEventRow {
        tumult_lake::EntityEventRow {
            kind: tumult_lake::EntityEventKind::State,
            ts_ns: FUTURE + i64::try_from(id).unwrap(),
            received_at_ns: FUTURE,
            entity_type: "proc".into(),
            entity_id: vec![("process.pid".into(), format!("{id:03}"))],
            description: json!({"process.command_line": "x".repeat(bytes)}),
            relationships: json!([]),
            interval_s: Some(0),
            delete_reason: None,
            wire_form: tumult_lake::WireForm::Spec,
            observer: "anonymous".into(),
            schema_url: None,
            environment: "prod".into(),
        }
    }

    /// A lake with `count` entities of about `bytes` each.
    fn lake(count: usize, bytes: usize) -> (tempfile::TempDir, tumult_lake::Store) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let events: Vec<_> = (0..count).map(|id| heavy(id, bytes)).collect();
        store
            .writer()
            .unwrap()
            .apply_entity_events(&events, &tumult_lake::EntityLimits::default())
            .unwrap();
        (dir, store)
    }

    fn list_params(sort: Sort) -> Vec<SqlParam> {
        let mut params = vec![
            SqlParam::Null,
            SqlParam::Null,
            SqlParam::Int(i64::MIN),
            scopes_param(&[]),
        ];
        params.extend(sort.position_params(None).unwrap());
        params
    }

    /// The store hands over the rows of the page and no more: what is
    /// beyond the budget is cut by the statement, not after every row of
    /// `limit` was read into memory.
    #[test]
    fn the_statement_itself_stops_at_the_byte_budget() {
        let (_dir, store) = lake(40, 10_000);
        let reader = store.read_only().unwrap();
        for sort in [Sort::Key, Sort::Recent] {
            let sql = list_page_sql(sort, 1000, 25_000);
            let rows = reader
                .query_json_rows_bound(&sql, &list_params(sort))
                .unwrap();
            assert_eq!(rows.len(), 3, "{sort:?}: three rows reach 25 000 bytes");
            let page = paged(rows);
            assert!(page.truncated, "{sort:?}");
            assert_eq!(page.rows[0].get("page_candidates"), None);
            assert_eq!(page.rows[0].get("page_bytes_before"), None);
        }
        let sql = list_page_sql(Sort::Key, 1000, 1);
        let rows = reader
            .query_json_rows_bound(&sql, &list_params(Sort::Key))
            .unwrap();
        assert_eq!(rows.len(), 1, "a page holds one row at least");
        let sql = list_page_sql(Sort::Key, 1000, MAX_PAGE_BYTES);
        let rows = reader
            .query_json_rows_bound(&sql, &list_params(Sort::Key))
            .unwrap();
        assert_eq!(rows.len(), 40);
        assert!(!paged(rows).truncated, "everything fitted");
    }

    #[test]
    fn the_history_statement_stops_at_the_byte_budget_too() {
        let (_dir, store) = lake(1, 10_000);
        let later: Vec<_> = (1..=9)
            .map(|n| {
                let mut event = heavy(0, 10_000 + n);
                event.ts_ns = FUTURE + i64::try_from(n).unwrap();
                event
            })
            .collect();
        store
            .writer()
            .unwrap()
            .apply_entity_events(&later, &tumult_lake::EntityLimits::default())
            .unwrap();
        let reader = store.read_only().unwrap();
        let mut params = vec![
            SqlParam::Text("proc|process.pid=000".into()),
            scopes_param(&[]),
            SqlParam::Null,
        ];
        params.extend(history_position_params(None));
        let rows = reader
            .query_json_rows_bound(&history_page_sql(1000, 25_000), &params)
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(paged(rows).truncated);
        let rows = reader
            .query_json_rows_bound(&history_page_sql(4, MAX_PAGE_BYTES), &params)
            .unwrap();
        assert_eq!(rows.len(), 4, "the row limit holds");
        let page = paged(rows);
        assert!(page.truncated);
        let starts: Vec<i64> = page
            .rows
            .iter()
            .map(|row| row["valid_from_ns"].as_i64().unwrap() - FUTURE)
            .collect();
        assert_eq!(starts, [9, 8, 7, 6], "newest first");
    }
}
