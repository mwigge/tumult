//! Per-request evidence recording to the tracing log and JSONL journal.

use crate::engine::to_agent_response;
use crate::faults::FaultTargetResponse;

use super::config::ProxyState;

/// Longest request path written to a log event, in characters.
const MAX_LOGGED_PATH_CHARS: usize = 256;

/// Emit per-request evidence to the tracing log and, if configured, the JSONL
/// journal. The body is summarised by length and contract verdicts only — no
/// raw payload is persisted, preserving the metadata-only capture default.
pub(crate) fn record(
    state: &ProxyState,
    parts: &axum::http::request::Parts,
    applied: &[String],
    status: u16,
    body: &str,
    latency_ms: u64,
) {
    let observed = FaultTargetResponse {
        body: body.to_string(),
        latency_ms,
        retry_count: 0,
        tool_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    };
    let agent = to_agent_response(&observed);
    let verdicts: Vec<String> = state
        .contracts
        .iter()
        .map(|contract| {
            let outcome = crate::contracts::evaluate_contract(&state.scenario, contract, &agent);
            format!(
                "{}={}",
                outcome.contract_type,
                if outcome.passed { "pass" } else { "fail" }
            )
        })
        .collect();

    let faults = if applied.is_empty() {
        "none".to_string()
    } else {
        applied.join(",")
    };

    // The path is request input: it is cut before it is logged, and the
    // query string (which can carry credentials) is never included.
    tracing::info!(
        name: "agentic.proxy.request",
        {
            agentic.scenario = %state.scenario,
            http.request.method = %parts.method,
            url.path = bounded_path(parts.uri.path()),
            http.response.status_code = status,
            latency_ms,
            agentic.faults = %faults,
            agentic.contracts = %verdicts.join(","),
            http.response.body.size = body.len(),
        },
        "proxied request"
    );

    if let Some(path) = &state.journal_path {
        let line = format!(
            r#"{{"scenario":"{}","method":"{}","path":"{}","status":{},"latency_ms":{},"faults":[{}],"contracts":[{}],"body_bytes":{}}}"#,
            state.scenario,
            parts.method,
            parts.uri.path(),
            status,
            latency_ms,
            applied
                .iter()
                .map(|fault| format!("\"{fault}\""))
                .collect::<Vec<_>>()
                .join(","),
            verdicts
                .iter()
                .map(|verdict| format!("\"{verdict}\""))
                .collect::<Vec<_>>()
                .join(","),
            body.len(),
        );
        if let Err(error) = append_journal_line(path, &line) {
            // Journal lines are evidence: a dropped one must leave a trace.
            tracing::error!(
                name: "agentic.proxy.journal.write_failed",
                {
                    file.path = %path.display(),
                    error.type = "io",
                    error.message = %tumult_otel::logging::err(&error),
                },
                "proxy journal line not written"
            );
        }
    }
}

/// The first [`MAX_LOGGED_PATH_CHARS`] characters of `path`.
fn bounded_path(path: &str) -> &str {
    match path.char_indices().nth(MAX_LOGGED_PATH_CHARS) {
        Some((end, _)) => &path[..end],
        None => path,
    }
}

/// Appends `line` to the JSONL journal at `path`, creating the file and its
/// parent directory when missing.
fn append_journal_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    use tracing::Level;
    use tumult_otel::logging::testing::capture_json;

    use super::{record, ProxyState, MAX_LOGGED_PATH_CHARS};

    fn state(journal_path: Option<PathBuf>) -> ProxyState {
        ProxyState {
            upstream: "http://127.0.0.1:1".to_string(),
            scenario: "rate-limit-storm".to_string(),
            faults: Vec::new(),
            contracts: Vec::new(),
            client: reqwest::Client::new(),
            journal_path,
            seed: 7,
            tumult_client: tumult_otel::agentic::TumultClient::Unknown,
            counter: AtomicU64::new(0),
        }
    }

    fn request_parts(uri: &str) -> axum::http::request::Parts {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .body(())
            .unwrap()
            .into_parts()
            .0
    }

    #[test]
    fn a_proxied_request_is_a_named_event_with_semantic_fields() {
        let state = state(None);
        let parts = request_parts("/v1/messages?api_key=sk-secret");
        let logs = capture_json(Level::INFO, || {
            record(&state, &parts, &["latency".to_string()], 429, "{}", 12);
        });

        let event = &logs.named("agentic.proxy.request")[0];
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["agentic.scenario"], "rate-limit-storm");
        assert_eq!(event["http.request.method"], "POST");
        assert_eq!(event["url.path"], "/v1/messages");
        assert_eq!(event["http.response.status_code"], 429);
        assert_eq!(event["latency_ms"], 12);
        assert_eq!(event["agentic.faults"], "latency");
        assert_eq!(event["http.response.body.size"], 2);
        for legacy in ["method", "path", "status"] {
            assert!(
                event.get(legacy).is_none(),
                "legacy field {legacy}: {event}"
            );
        }
        assert!(!logs.raw.contains("sk-secret"), "{}", logs.raw);
    }

    #[test]
    fn an_oversized_request_path_is_cut_in_the_log_event() {
        let state = state(None);
        let hostile = format!("/{}", "a".repeat(4 * MAX_LOGGED_PATH_CHARS));
        let parts = request_parts(&hostile);
        let logs = capture_json(Level::INFO, || {
            record(&state, &parts, &[], 200, "", 1);
        });

        let logged = logs.named("agentic.proxy.request")[0]["url.path"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(logged.chars().count(), MAX_LOGGED_PATH_CHARS);
        assert!(hostile.starts_with(&logged));
    }

    #[test]
    fn only_the_journal_line_that_cannot_be_written_is_an_error_event() {
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"file").unwrap();
        let unwritable = blocker.join("proxy.jsonl");
        let writable = dir.path().join("nested").join("proxy.jsonl");
        let parts = request_parts("/v1/messages");

        // One request into each journal, under the same capture.
        let logs = capture_json(Level::INFO, || {
            record(&state(Some(writable.clone())), &parts, &[], 200, "", 1);
            record(&state(Some(unwritable.clone())), &parts, &[], 200, "", 1);
        });

        assert_eq!(logs.named("agentic.proxy.request").len(), 2);
        let failures = logs.named("agentic.proxy.journal.write_failed");
        assert_eq!(failures.len(), 1, "{}", logs.raw);
        let event = failures[0];
        assert_eq!(event["level"], "ERROR");
        assert_eq!(event["file.path"], unwritable.to_str().unwrap());
        assert_eq!(event["error.type"], "io");
        assert!(event["error.message"].is_string(), "{event}");
        let written = std::fs::read_to_string(&writable).unwrap();
        assert_eq!(written.lines().count(), 1, "{written}");
    }
}
