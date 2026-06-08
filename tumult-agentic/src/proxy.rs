//! Fault-injecting HTTP proxy for live agentic clients.
//!
//! The scenario-pack and replay paths exercise faults against synthetic
//! baselines. This module exercises them against a *real* agent: it stands up a
//! local reverse proxy in front of a model/provider endpoint and injects the
//! faults of a chosen scenario pack into the live traffic.
//!
//! Every mainstream coding agent can be pointed at a custom base URL, so the
//! same proxy works against all of them:
//!
//! | Client       | Wiring                                                      |
//! |--------------|------------------------------------------------------------|
//! | Claude Code  | `ANTHROPIC_BASE_URL=http://127.0.0.1:8080`                  |
//! | Codex CLI    | `OPENAI_BASE_URL=http://127.0.0.1:8080/v1`                  |
//! | OpenCode     | provider `baseURL` / `OPENAI_BASE_URL=http://127.0.0.1:8080/v1` |
//! | GitHub Copilot | `HTTPS_PROXY=http://127.0.0.1:8080` (or model base URL)   |
//!
//! Faults map onto HTTP behaviour as follows:
//!
//! - `model_latency` / `tool_latency` → delay before forwarding (TTFT damage)
//! - `rate_limit` → synthetic `429` with `retry-after` (no upstream call)
//! - `provider_error` → synthetic provider status code
//! - `model_timeout` → synthetic `504`
//! - `malformed_output` / `output_truncation` / `tool_failure` /
//!   `retrieval_poisoning` → mutate the upstream response body
//! - token/retry/hallucination/context faults are agent-internal and are
//!   recorded but not injectable at the HTTP layer (the proxy forwards as-is)
#![allow(clippy::doc_markdown)] // module doc names many products (OpenCode, etc.)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{header::CONTENT_TYPE, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;

use crate::engine::to_agent_response;
use crate::faults::{apply_fault, FaultEngine, FaultSpec, FaultTargetResponse};
use crate::model::AgenticError;
use crate::scenarios::bundled_packs;

/// Maximum request body the proxy will buffer before forwarding (16 MiB).
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Configuration for a fault-injecting proxy run.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Upstream base URL to forward to, e.g. `https://api.anthropic.com`.
    pub upstream: String,
    /// Bundled scenario pack whose faults are injected into live traffic.
    pub scenario_pack: String,
    /// Optional JSONL journal path; one line is appended per proxied request.
    pub journal_path: Option<PathBuf>,
    /// Base seed for the per-request fault gate (kept reproducible).
    pub seed: u64,
    /// Client this proxy run targets; tags the proxy span's `tumult.client`.
    pub client: tumult_otel::agentic::TumultClient,
}

struct ProxyState {
    upstream: String,
    scenario: String,
    faults: Vec<FaultSpec>,
    contracts: Vec<crate::contracts::ContractSpec>,
    client: reqwest::Client,
    journal_path: Option<PathBuf>,
    seed: u64,
    tumult_client: tumult_otel::agentic::TumultClient,
    counter: AtomicU64,
}

/// Build the proxy [`Router`] for `config`.
///
/// The returned router is unbound; callers bind a listener and pass it to
/// [`serve`] (or `axum::serve`). Splitting build from serve keeps the proxy
/// testable against an ephemeral port.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] when the scenario pack is unknown,
/// and [`AgenticError::Adapter`] when the HTTP client cannot be built.
pub fn router(config: ProxyConfig) -> Result<Router, AgenticError> {
    let pack = bundled_packs()
        .into_iter()
        .find(|pack| pack.name == config.scenario_pack)
        .ok_or_else(|| {
            AgenticError::InvalidConfig(format!("unknown scenario pack: {}", config.scenario_pack))
        })?;

    let client = reqwest::Client::builder()
        .build()
        .map_err(|err| AgenticError::Adapter(format!("proxy client build failed: {err}")))?;

    let state = Arc::new(ProxyState {
        upstream: config.upstream.trim_end_matches('/').to_string(),
        scenario: pack.name.to_string(),
        faults: pack.faults,
        contracts: pack.contracts,
        client,
        journal_path: config.journal_path,
        seed: config.seed,
        tumult_client: config.client,
        counter: AtomicU64::new(0),
    });

    Ok(Router::new().fallback(handle).with_state(state))
}

/// Serve the proxy on `listener` until the process is terminated.
///
/// # Errors
///
/// Returns [`AgenticError`] if the router cannot be built or the server exits
/// with an error.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: ProxyConfig,
) -> Result<(), AgenticError> {
    let router = router(config)?;
    axum::serve(listener, router)
        .await
        .map_err(|err| AgenticError::Adapter(format!("proxy server error: {err}")))
}

/// What a single applied fault does to a proxied request.
enum Injection {
    Delay(Duration),
    ShortCircuit {
        status: u16,
        body: String,
        retry_after_ms: Option<u64>,
    },
    MutateBody(FaultSpec),
    Internal,
}

fn classify(fault: &FaultSpec) -> Injection {
    match fault {
        FaultSpec::ModelLatency { latency_ms, .. } | FaultSpec::ToolLatency { latency_ms, .. } => {
            Injection::Delay(Duration::from_millis(*latency_ms))
        }
        FaultSpec::RateLimit { retry_after_ms, .. } => Injection::ShortCircuit {
            status: 429,
            body: error_body("rate_limit_error"),
            retry_after_ms: Some(*retry_after_ms),
        },
        FaultSpec::ProviderError { code, .. } => Injection::ShortCircuit {
            status: *code,
            body: error_body("provider_error"),
            retry_after_ms: None,
        },
        FaultSpec::ModelTimeout { .. } => Injection::ShortCircuit {
            status: 504,
            body: error_body("model_timeout"),
            retry_after_ms: None,
        },
        FaultSpec::MalformedOutput { .. }
        | FaultSpec::OutputTruncation { .. }
        | FaultSpec::ToolFailure { .. }
        | FaultSpec::RetrievalPoisoning { .. } => Injection::MutateBody(fault.clone()),
        FaultSpec::HallucinatedToolCall { .. }
        | FaultSpec::ContextTruncation { .. }
        | FaultSpec::TokenBudgetExhaustion { .. }
        | FaultSpec::RetryLoopPressure { .. } => Injection::Internal,
    }
}

fn error_body(kind: &str) -> String {
    format!(r#"{{"type":"error","error":{{"type":"{kind}","message":"injected by tumult"}}}}"#)
}

/// Project request headers into a lowercase string map for trace-context
/// extraction (`traceparent`/`tracestate` are matched lowercase).
fn header_map_lower(headers: &axum::http::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

async fn handle(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let request_body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return synthetic(
                StatusCode::PAYLOAD_TOO_LARGE,
                error_body("request_too_large"),
                None,
            )
            .into_response_with_note(&format!("read body failed: {err}"));
        }
    };

    let seq = state.counter.fetch_add(1, Ordering::Relaxed);
    let mut engine = FaultEngine::new(state.seed.wrapping_add(seq));

    let mut delay = Duration::ZERO;
    let mut short_circuit: Option<(u16, String, Option<u64>)> = None;
    let mut body_faults: Vec<FaultSpec> = Vec::new();
    let mut applied: Vec<String> = Vec::new();

    for fault in &state.faults {
        if !engine.should_apply(fault) {
            continue;
        }
        applied.push(fault.fault_type().to_string());
        match classify(fault) {
            Injection::Delay(duration) => delay = delay.saturating_add(duration),
            Injection::ShortCircuit {
                status,
                body,
                retry_after_ms,
            } => {
                if short_circuit.is_none() {
                    short_circuit = Some((status, body, retry_after_ms));
                }
            }
            Injection::MutateBody(spec) => body_faults.push(spec),
            Injection::Internal => {}
        }
    }

    // Start a fault span parented under the client's inbound trace context (or
    // standalone, tagged by tumult.client, when the client did not propagate).
    let parent = tumult_otel::propagation::parse_traceparent(&header_map_lower(&parts.headers));
    let proxy_span = tumult_otel::agentic_span::start_proxy_span(
        &parent,
        state.tumult_client.as_str(),
        &state.scenario,
        parts.method.as_str(),
        parts.uri.path(),
    );

    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    let started = Instant::now();

    // Short-circuit faults never touch the upstream.
    if let Some((status, body, retry_after_ms)) = short_circuit {
        let elapsed = elapsed_ms(started, delay);
        record(&state, &parts, &applied, status, &body, elapsed);
        proxy_span.set_outcome(status, elapsed, &applied);
        proxy_span.end();
        return synthetic(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            body,
            retry_after_ms,
        );
    }

    // Forward to the upstream, then mutate the response body.
    let path_and_query = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), ToString::to_string);
    let url = format!("{}{path_and_query}", state.upstream);

    let upstream = match forward(
        &state.client,
        &parts,
        &url,
        request_body,
        proxy_span.context(),
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            let body = error_body("upstream_unreachable");
            let elapsed = elapsed_ms(started, delay);
            record(&state, &parts, &applied, 502, &body, elapsed);
            proxy_span.set_outcome(502, elapsed, &applied);
            proxy_span.end();
            return synthetic(StatusCode::BAD_GATEWAY, body, None)
                .into_response_with_note(&err.to_string());
        }
    };

    let mut response_body = upstream.body;
    for fault in &body_faults {
        response_body = mutate_body(fault, response_body);
    }

    let elapsed = elapsed_ms(started, delay);
    record(
        &state,
        &parts,
        &applied,
        upstream.status,
        &response_body,
        elapsed,
    );
    proxy_span.set_outcome(upstream.status, elapsed, &applied);
    proxy_span.end();

    let status = StatusCode::from_u16(upstream.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, response_body).into_response();
    if let Some(content_type) = upstream.content_type {
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            response.headers_mut().insert(CONTENT_TYPE, value);
        }
    }
    response
}

struct UpstreamResponse {
    status: u16,
    content_type: Option<String>,
    body: String,
}

async fn forward(
    client: &reqwest::Client,
    parts: &axum::http::request::Parts,
    url: &str,
    body: Bytes,
    trace: &opentelemetry::Context,
) -> Result<UpstreamResponse, AgenticError> {
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .map_err(|err| AgenticError::Adapter(format!("bad method: {err}")))?;
    let mut builder = client.request(method, url);
    for (name, value) in &parts.headers {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let (Ok(rname), Ok(rvalue)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(rname, rvalue);
        }
    }
    // Propagate this proxy span's W3C trace context upstream so a compliant
    // intermediary or server continues the same distributed trace.
    let mut trace_headers: HashMap<String, String> = HashMap::new();
    tumult_otel::propagation::inject_traceparent(trace, &mut trace_headers);
    for (name, value) in trace_headers {
        if let (Ok(rname), Ok(rvalue)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            builder = builder.header(rname, rvalue);
        }
    }
    let response = builder
        .body(body.to_vec())
        .send()
        .await
        .map_err(|err| AgenticError::Adapter(format!("upstream request failed: {err}")))?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| AgenticError::Adapter(format!("upstream body read failed: {err}")))?;
    let body = String::from_utf8_lossy(&bytes).into_owned();

    Ok(UpstreamResponse {
        status,
        content_type,
        body,
    })
}

/// Apply a body-mutating fault by reusing the shared [`apply_fault`] mutator so
/// the proxy and the offline engine produce identical contamination.
fn mutate_body(fault: &FaultSpec, body: String) -> String {
    let response = FaultTargetResponse {
        body,
        latency_ms: 0,
        retry_count: 0,
        tool_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    };
    match apply_fault(fault, response) {
        Ok(outcome) => outcome.response.body,
        Err(_) => "{malformed-json".to_string(),
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            // Force identity encoding so we can read and mutate the body.
            | "accept-encoding"
            // Managed by the proxy span: strip inbound W3C context so we inject
            // our own (avoids duplicate traceparent on the upstream request).
            | "traceparent"
            | "tracestate"
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

fn synthetic(status: StatusCode, body: String, retry_after_ms: Option<u64>) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(ms) = retry_after_ms {
        let seconds = ms.div_ceil(1000).max(1);
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

fn elapsed_ms(started: Instant, delay: Duration) -> u64 {
    let injected = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    let measured = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    injected.saturating_add(measured)
}

/// Emit per-request evidence to the tracing log and, if configured, the JSONL
/// journal. The body is summarised by length and contract verdicts only — no
/// raw payload is persisted, preserving the metadata-only capture default.
fn record(
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

/// Extension to attach a diagnostic note header to a synthetic error response.
trait ResponseNote {
    fn into_response_with_note(self, note: &str) -> Response;
}

impl ResponseNote for Response {
    fn into_response_with_note(mut self, note: &str) -> Response {
        if let Ok(value) = HeaderValue::from_str(note) {
            self.headers_mut().insert("x-tumult-proxy-note", value);
        }
        self
    }
}
