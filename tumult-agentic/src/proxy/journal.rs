//! Per-request evidence recording to the tracing log and JSONL journal.

use crate::engine::to_agent_response;
use crate::faults::FaultTargetResponse;

use super::config::ProxyState;

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

    tracing::info!(
        scenario = %state.scenario,
        method = %parts.method,
        path = %parts.uri.path(),
        status,
        latency_ms,
        faults = %faults,
        contracts = %verdicts.join(","),
        body_bytes = body.len(),
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
        append_journal_line(path, &line);
    }
}

fn append_journal_line(path: &std::path::Path, line: &str) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}
