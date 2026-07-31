//! `POST /api/ask` — natural-language analytics over the store.
//!
//! Flow: exact **golden** question → curated SQL (no LLM needed); otherwise
//! LLM → [`sql_guard`] validation → `LIMIT` injection → execution on a
//! read-only connection. When no LLM is reachable the endpoint degrades to
//! `{ "configured": false }` so the UI can show a setup hint instead of an
//! error. DuckDB has no statement-timeout setting, so execution is bounded
//! by a wall-clock timeout around the blocking task instead.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tumult_intelligence::llm::{AiError, Message, Role};
use tumult_intelligence::sql_guard;
use tumult_lake::Store;

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

#[derive(Deserialize)]
pub struct AskRequest {
    question: String,
}

pub async fn ask(State(state): State<ApiState>, Json(req): Json<AskRequest>) -> Response {
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
    let sql = sql_guard::inject_limit(&sql, ROW_LIMIT);

    let db = state.db_path.as_ref().clone();
    let run_sql = sql.clone();
    let run = tokio::task::spawn_blocking(move || {
        let store = Store::at(&db);
        let reader = store.read_only().map_err(|e| e.to_string())?;
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
}
