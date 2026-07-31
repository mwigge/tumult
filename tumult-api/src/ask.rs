//! `POST /api/ask` — natural-language analytics over the store.
//!
//! Flow: exact **golden** question → curated SQL (no LLM needed); otherwise
//! LLM → [`sql_guard`] validation → per-user **environment scoping** →
//! `LIMIT` injection → execution on a **locked-down** connection (read-only
//! *and* `enable_external_access = false`, so even guard bypasses cannot
//! reach the server file system or network). When no LLM is reachable the
//! endpoint degrades to `{ "configured": false }` so the UI can show a setup
//! hint instead of an error. DuckDB has no statement-timeout setting, so
//! execution is bounded by a wall-clock timeout around the blocking task
//! instead.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;
use tumult_intelligence::llm::{AiError, Message, Role};
use tumult_intelligence::locked_reader::LockedReader;
use tumult_intelligence::sql_guard;

use crate::auth::Principal;
use crate::ApiState;

/// Tables LLM-generated SQL may touch (enforced by [`sql_guard`]).
const ALLOWED_TABLES: &[&str] = &[
    "spans",
    "logs",
    "metric_sums",
    "metric_gauges",
    "metric_histograms",
    "experiment_runs",
    "import_batches",
];

const MAX_QUESTION_CHARS: usize = 1000;
const ROW_LIMIT: u64 = 500;
const LLM_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Curated question → SQL pairs. These answer the demo's showcase questions
/// with zero LLM involvement and double as few-shot examples in the prompt.
const GOLDEN: &[(&str, &str)] = &[
    (
        "how many experiments ran",
        "SELECT COUNT(*) AS experiments FROM spans \
         WHERE span_name = 'resilience.experiment'",
    ),
    (
        "what is the pass rate",
        "SELECT SUM(CAST(value AS DOUBLE)) FILTER (WHERE outcome_status = 'success') \
         / NULLIF(SUM(CAST(value AS DOUBLE)), 0) AS pass_rate \
         FROM metric_sums WHERE metric_name = 'tumult.experiments.total'",
    ),
    (
        "which experiments failed",
        "SELECT s.experiment_id, s.experiment_name, l.log_attrs['status'] AS status \
         FROM spans s JOIN logs l \
           ON l.log_attrs['experiment_id'] = s.experiment_id \
          AND l.body = 'experiment.completed' \
         WHERE s.span_name = 'resilience.experiment' \
           AND l.log_attrs['status'] != 'Completed' \
         ORDER BY s.ts_ns DESC",
    ),
    (
        "what faults were injected",
        "SELECT fault_type, COUNT(*) AS count FROM spans \
         WHERE fault_type IS NOT NULL \
         GROUP BY fault_type ORDER BY count DESC",
    ),
    (
        "experiments per day",
        "SELECT (ts_ns // 86400000000000) * 86400000000000 // 1000000000 AS day_s, \
         COUNT(*) AS experiments FROM spans \
         WHERE span_name = 'resilience.experiment' \
         GROUP BY 1 ORDER BY 1",
    ),
    (
        "show recent experiments",
        "SELECT s.experiment_id, s.experiment_name, s.ts_ns AS started_ns, \
         l.log_attrs['status'] AS status \
         FROM spans s LEFT JOIN logs l \
           ON l.log_attrs['experiment_id'] = s.experiment_id \
          AND l.body = 'experiment.completed' \
         WHERE s.span_name = 'resilience.experiment' \
         ORDER BY s.ts_ns DESC LIMIT 20",
    ),
];

/// Lowercase, collapse whitespace, strip trailing punctuation.
fn normalize(question: &str) -> String {
    let collapsed: String = question.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(['?', '.', '!'])
        .to_ascii_lowercase()
}

/// Exact-match a question against the golden bank (after normalisation).
fn golden_sql(question: &str) -> Option<&'static str> {
    let q = normalize(question);
    GOLDEN.iter().find(|(gq, _)| *gq == q).map(|(_, sql)| *sql)
}

/// System prompt: schema, tumult data semantics, output rules and the golden
/// bank as few-shot examples.
fn schema_prompt() -> String {
    let examples = GOLDEN
        .iter()
        .map(|(q, sql)| format!("Q: {q}\nSQL: {sql}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "You translate natural-language questions into DuckDB SQL over a \
         chaos-engineering telemetry store.\n\
         \n\
         TABLES\n\
         - spans(ts_ns BIGINT epoch-ns, trace_id, span_id, parent_span_id, span_name, \
         duration_ns, status_code, service_name, experiment_id, experiment_name, \
         outcome_status, fault_type, fault_subtype, target_system, target_technology, \
         hypothesis_met BOOL, recovery_time_s DOUBLE, span_attrs MAP(VARCHAR,VARCHAR))\n\
         - logs(ts_ns, severity_text, body, trace_id, span_id, log_attrs MAP(VARCHAR,VARCHAR))\n\
         - metric_sums(ts_ns, metric_name, value DOUBLE, experiment_name, outcome_status, plugin_name)\n\
         - metric_gauges(same shape as metric_sums)\n\
         - metric_histograms(ts_ns, metric_name, count, sum, min, max, experiment_name, outcome_status)\n\
         \n\
         SEMANTICS\n\
         - One experiment run = one span with span_name = 'resilience.experiment'.\n\
         - Real tumult runs report their outcome in a logs row with \
         body = 'experiment.completed'; the status lives in \
         log_attrs['status'] ('Completed'|'Deviated'|'Failed'), keyed by \
         log_attrs['experiment_id']. Join spans to it for outcomes — root \
         spans themselves carry no outcome.\n\
         - Pass/fail counters: metric_sums rows with \
         metric_name = 'tumult.experiments.total' and \
         outcome_status = 'success'|'failure'; deviations: \
         metric_name = 'tumult.hypothesis.deviations.total'.\n\
         \n\
         RULES\n\
         - Reply with ONE raw SQL SELECT (or WITH) statement. No markdown, no \
         comments, no semicolons, no explanation.\n\
         - Only these tables: {ALLOWED_TABLES:?}.\n\
         - Map subscript syntax: log_attrs['key']. Epoch-ns arithmetic uses \
         integer division (//).\n\
         \n\
         EXAMPLES\n\
         {examples}"
    )
}

/// Strip markdown code fences and any trailing semicolon from an LLM reply.
fn extract_sql(reply: &str) -> String {
    let trimmed = reply.trim();
    let inner = match trimmed.find("```") {
        Some(start) => {
            let after = &trimmed[start + 3..];
            let after = after
                .strip_prefix("sql")
                .or_else(|| after.strip_prefix("SQL"))
                .unwrap_or(after);
            match after.find("```") {
                Some(end) => &after[..end],
                None => after,
            }
        }
        None => trimmed,
    };
    inner.trim().trim_end_matches(';').trim().to_string()
}

/// Quote a string literal for inline SQL (single-quote doubling) — mirrors
/// `sql_string` in `crate::lib`.
fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Confine a validated query to the principal's environment scopes (same
/// contract as `env_scope_where` in `crate::lib`): every `spans` reference —
/// the only allow-listed table with an environment column — is wrapped in a
/// `target_environment IN (…)` subquery. Any other referenced table cannot
/// be scope-filtered, so [`sql_guard::scope_tables`] fails closed on it.
/// Empty scopes mean every environment (the query passes through untouched).
fn apply_env_scopes(sql: &str, scopes: &[String]) -> Result<String, sql_guard::SqlGuardError> {
    if scopes.is_empty() {
        return Ok(sql.to_string());
    }
    let envs = scopes
        .iter()
        .map(|s| sql_string(s))
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = format!("target_environment IN ({envs})");
    sql_guard::scope_tables(sql, &[("spans", predicate.as_str())])
}

#[derive(Deserialize)]
pub struct AskRequest {
    question: String,
}

pub async fn ask(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<AskRequest>,
) -> Response {
    let question = req.question.trim();
    if question.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "question must not be empty"})),
        )
            .into_response();
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("question too long (max {MAX_QUESTION_CHARS} chars)")})),
        )
            .into_response();
    }

    let (sql, source) = match golden_sql(question) {
        Some(sql) => (sql.to_string(), "golden"),
        None => {
            let messages = [
                Message {
                    role: Role::System,
                    content: schema_prompt(),
                },
                Message {
                    role: Role::User,
                    content: question.to_string(),
                },
            ];
            let reply = match tokio::time::timeout(LLM_TIMEOUT, state.llm.chat(&messages)).await {
                Ok(Ok(reply)) => reply,
                // Unreachable LLM (connection refused / timeout) means the
                // feature is not set up — tell the UI gracefully.
                Ok(Err(AiError::Http(e))) if e.is_connect() || e.is_timeout() => {
                    return Json(json!({"configured": false})).into_response();
                }
                Ok(Err(e)) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"configured": true, "error": format!("LLM call failed: {e}")})),
                    )
                        .into_response();
                }
                Err(_elapsed) => return Json(json!({"configured": false})).into_response(),
            };
            let sql = extract_sql(&reply);
            if let Err(e) = sql_guard::validate_generated_sql(&sql, ALLOWED_TABLES) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "configured": true,
                        "error": format!("generated SQL rejected by the guard: {e}"),
                        "sql": sql,
                    })),
                )
                    .into_response();
            }
            (sql, "llm")
        }
    };
    // Golden SQL passes the same guard (defence in depth + tests pin this).
    if let Err(e) = sql_guard::validate_generated_sql(&sql, ALLOWED_TABLES) {
        return internal(format!("internal SQL failed the guard: {e}"), &sql);
    }
    // Per-user environment scoping: confine `spans` to the principal's
    // environments; tables without an environment column fail closed for
    // scoped principals (no cross-environment existence leak).
    let sql = match apply_env_scopes(&sql, &principal.env_scopes) {
        Ok(sql) => sql,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "configured": true,
                    "error": format!("query cannot be confined to your environment scopes: {e}"),
                    "sql": sql,
                })),
            )
                .into_response();
        }
    };
    let sql = sql_guard::inject_limit(&sql, ROW_LIMIT);

    let db = state.db_path.as_ref().clone();
    let run_sql = sql.clone();
    let run = tokio::task::spawn_blocking(move || {
        // Locked-down reader: read-only AND external access disabled, so a
        // query that somehow passed the guard still cannot read server files
        // (`read_text` works even under access_mode=READ_ONLY without this).
        let reader = LockedReader::open(&db).map_err(|e| e.to_string())?;
        reader.query_json_rows(&run_sql).map_err(|e| e.to_string())
    });
    match tokio::time::timeout(QUERY_TIMEOUT, run).await {
        Ok(Ok(Ok(rows))) => Json(json!({
            "configured": true,
            "source": source,
            "sql": sql,
            "rows": rows,
        }))
        .into_response(),
        Ok(Ok(Err(e))) => internal(format!("query failed: {e}"), &sql),
        Ok(Err(e)) => internal(format!("query task failed: {e}"), &sql),
        Err(_elapsed) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({"configured": true, "error": "query timed out", "sql": sql})),
        )
            .into_response(),
    }
}

fn internal(msg: String, sql: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"configured": true, "error": msg, "sql": sql})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_bank_matches_normalised_questions() {
        assert!(golden_sql("How many experiments ran?").is_some());
        assert!(golden_sql("  what is the pass rate! ").is_some());
        assert!(golden_sql("something entirely different").is_none());
    }

    #[test]
    fn golden_sql_passes_the_guard() {
        for (_, sql) in GOLDEN {
            sql_guard::validate_generated_sql(sql, ALLOWED_TABLES)
                .unwrap_or_else(|e| panic!("golden SQL rejected: {e}\n{sql}"));
        }
    }

    #[test]
    fn extract_sql_strips_fences_and_semicolons() {
        assert_eq!(extract_sql("SELECT 1;"), "SELECT 1");
        assert_eq!(extract_sql("```sql\nSELECT 1;\n```"), "SELECT 1");
        assert_eq!(extract_sql("```\nSELECT 1\n```"), "SELECT 1");
    }

    #[test]
    fn guard_rejects_read_text_exfiltration() {
        // The reported attack: env secrets via DuckDB file functions over an
        // allow-listed table.
        let sql = "SELECT read_text('/proc/self/environ') FROM spans";
        assert!(matches!(
            sql_guard::validate_generated_sql(sql, ALLOWED_TABLES),
            Err(sql_guard::SqlGuardError::FunctionNotAllowed(_))
        ));
    }

    #[test]
    fn guard_accepts_legit_aggregate_query() {
        let sql = "SELECT fault_type, COUNT(*) AS count FROM spans \
                   WHERE fault_type IS NOT NULL GROUP BY fault_type ORDER BY count DESC";
        sql_guard::validate_generated_sql(sql, ALLOWED_TABLES).unwrap();
    }

    #[test]
    fn env_scopes_pass_through_when_empty() {
        let sql = "SELECT COUNT(*) FROM spans";
        assert_eq!(apply_env_scopes(sql, &[]).unwrap(), sql);
    }

    #[test]
    fn env_scopes_wrap_spans_and_escape_values() {
        let scoped = apply_env_scopes(
            "SELECT COUNT(*) FROM spans",
            &["dev".to_string(), "it's".to_string()],
        )
        .unwrap();
        assert_eq!(
            scoped,
            "SELECT COUNT(*) FROM (SELECT * FROM spans WHERE \
             target_environment IN ('dev', 'it''s')) AS spans"
        );
    }

    #[test]
    fn env_scopes_fail_closed_on_unscopable_tables() {
        // logs/metric_sums carry no environment column: a scoped principal
        // must not reach them at all.
        for sql in [
            "SELECT COUNT(*) FROM logs",
            "SELECT AVG(value) FROM metric_sums",
            "SELECT * FROM spans s JOIN logs l ON true",
        ] {
            assert!(
                matches!(
                    apply_env_scopes(sql, &["dev".to_string()]),
                    Err(sql_guard::SqlGuardError::TableNotScopable(_))
                ),
                "{sql} must fail closed for a scoped principal"
            );
        }
    }

    /// One root experiment span in a specific target environment.
    fn env_span(id: &str, env: &str, ts: i64) -> tumult_lake::SpanRow {
        tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}"),
            parent_span_id: None,
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: 1_000_000_000,
            status_code: "Unset".into(),
            status_message: String::new(),
            service_name: "tumult".into(),
            service_version: None,
            experiment_id: Some(id.into()),
            experiment_name: Some(format!("{id}-name")),
            outcome_status: None,
            fault_type: None,
            fault_subtype: None,
            fault_severity: None,
            blast_radius: None,
            target_system: Some("database".into()),
            target_technology: None,
            target_environment: Some(env.into()),
            plugin_name: None,
            hypothesis_met: None,
            recovery_time_s: None,
            span_attrs: vec![],
            resource_attrs: vec![],
            events: "[]".into(),
        }
    }

    #[test]
    fn scoped_query_returns_only_in_scope_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lake.duckdb");
        let store = tumult_lake::Store::open(&db).unwrap();
        store
            .writer()
            .unwrap()
            .insert_spans(&[
                env_span("exp-dev", "dev", 1_000_000_000),
                env_span("exp-prod", "prod", 2_000_000_000),
            ])
            .unwrap();
        drop(store);

        let sql = apply_env_scopes(
            "SELECT experiment_id FROM spans ORDER BY ts_ns",
            &["dev".to_string()],
        )
        .unwrap();
        let reader = LockedReader::open(&db).unwrap();
        let rows = reader.query_json_rows(&sql).unwrap();
        assert_eq!(rows, vec![json!({"experiment_id": "exp-dev"})]);

        // The unscoped principal sees both environments.
        let rows = reader
            .query_json_rows("SELECT experiment_id FROM spans ORDER BY ts_ns")
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn execution_path_blocks_file_reads() {
        // Even if the guard were bypassed, the locked-down connection itself
        // refuses external access.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lake.duckdb");
        drop(tumult_lake::Store::open(&db).unwrap());
        let reader = LockedReader::open(&db).unwrap();
        assert!(reader
            .query_json_rows("SELECT read_text('/proc/self/environ') AS v")
            .is_err());
    }
}
