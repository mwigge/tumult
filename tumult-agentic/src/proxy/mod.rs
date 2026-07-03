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

mod config;
mod forward;
mod handler;
mod inject;
mod journal;
mod response;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::Router;

use crate::model::AgenticError;
use crate::scenarios::bundled_packs;

use config::ProxyState;
use handler::handle;

pub use config::ProxyConfig;

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
