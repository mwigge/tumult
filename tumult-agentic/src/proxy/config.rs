//! Proxy configuration and shared request-handling state.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

use crate::faults::FaultSpec;

/// Maximum request body the proxy will buffer before forwarding (16 MiB).
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

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

pub(crate) struct ProxyState {
    pub(crate) upstream: String,
    pub(crate) scenario: String,
    pub(crate) faults: Vec<FaultSpec>,
    pub(crate) contracts: Vec<crate::contracts::ContractSpec>,
    pub(crate) client: reqwest::Client,
    pub(crate) journal_path: Option<PathBuf>,
    pub(crate) seed: u64,
    pub(crate) tumult_client: tumult_otel::agentic::TumultClient,
    pub(crate) counter: AtomicU64,
}
