//! Declarative per-client profiles.
//!
//! Each agentic client wires into the proxy slightly differently — which base
//! URL environment variable points it at the proxy, whether it already emits
//! native OpenTelemetry, and whether the proxy should nest model/tool spans
//! under the client's trace or merely correlate them. This module captures that
//! matrix declaratively so the proxy and CLI can resolve behaviour from a single
//! source of truth.
#![allow(clippy::doc_markdown)] // doc names products (OpenCode, Copilot, Codex)

use tumult_otel::agentic::TumultClient;

/// How the proxy relates its own spans to a client's trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceTier {
    /// Nest proxy spans under the client's inbound trace context.
    Nest,
    /// Emit standalone spans correlated by attributes rather than parentage.
    Correlate,
}

/// Declarative wiring + telemetry profile for a single agentic client.
#[derive(Debug, Clone, Copy)]
pub struct ClientProfile {
    /// The client this profile describes.
    pub client: TumultClient,
    /// Environment variable that points the client's base URL at the proxy, if
    /// the client supports base-URL redirection (otherwise `None`).
    pub base_url_env: Option<&'static str>,
    /// Whether the client already emits native OpenTelemetry.
    pub native_otel: bool,
    /// How model spans relate to the client's trace.
    pub model_surface: TraceTier,
    /// How tool spans relate to the client's trace.
    pub tool_surface: TraceTier,
}

/// Resolve the [`ClientProfile`] for a given client.
#[must_use]
pub fn profile_for(client: TumultClient) -> ClientProfile {
    match client {
        TumultClient::ClaudeCode => ClientProfile {
            client,
            base_url_env: Some("ANTHROPIC_BASE_URL"),
            native_otel: true,
            model_surface: TraceTier::Nest,
            tool_surface: TraceTier::Nest,
        },
        TumultClient::Codex => ClientProfile {
            client,
            base_url_env: Some("OPENAI_BASE_URL"),
            native_otel: true,
            model_surface: TraceTier::Correlate,
            tool_surface: TraceTier::Correlate,
        },
        TumultClient::OpenCode => ClientProfile {
            client,
            base_url_env: Some("OPENAI_BASE_URL"),
            native_otel: true,
            model_surface: TraceTier::Correlate,
            tool_surface: TraceTier::Nest,
        },
        TumultClient::Copilot => ClientProfile {
            client,
            base_url_env: None,
            native_otel: true,
            model_surface: TraceTier::Correlate,
            tool_surface: TraceTier::Correlate,
        },
        TumultClient::Unknown => ClientProfile {
            client,
            base_url_env: None,
            native_otel: false,
            model_surface: TraceTier::Correlate,
            tool_surface: TraceTier::Correlate,
        },
    }
}

/// Parse a client selector string into a [`TumultClient`].
///
/// Recognises the canonical kebab-case names plus the `claude` alias. Matching
/// is case-insensitive; anything unrecognised maps to [`TumultClient::Unknown`].
#[must_use]
pub fn parse_client(s: &str) -> TumultClient {
    match s.trim().to_ascii_lowercase().as_str() {
        "claude-code" | "claude" => TumultClient::ClaudeCode,
        "codex" => TumultClient::Codex,
        "copilot" => TumultClient::Copilot,
        "opencode" => TumultClient::OpenCode,
        _ => TumultClient::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_nests_both_surfaces() {
        let profile = profile_for(TumultClient::ClaudeCode);
        assert_eq!(profile.model_surface, TraceTier::Nest);
        assert_eq!(profile.tool_surface, TraceTier::Nest);
        assert_eq!(profile.base_url_env, Some("ANTHROPIC_BASE_URL"));
        assert!(profile.native_otel);
    }

    #[test]
    fn codex_correlates_both_surfaces() {
        let profile = profile_for(TumultClient::Codex);
        assert_eq!(profile.model_surface, TraceTier::Correlate);
        assert_eq!(profile.tool_surface, TraceTier::Correlate);
        assert_eq!(profile.base_url_env, Some("OPENAI_BASE_URL"));
        assert!(profile.native_otel);
    }

    #[test]
    fn opencode_nests_tools_but_correlates_model() {
        let profile = profile_for(TumultClient::OpenCode);
        assert_eq!(profile.model_surface, TraceTier::Correlate);
        assert_eq!(profile.tool_surface, TraceTier::Nest);
        assert_eq!(profile.base_url_env, Some("OPENAI_BASE_URL"));
    }

    #[test]
    fn copilot_has_no_base_url_env() {
        let profile = profile_for(TumultClient::Copilot);
        assert_eq!(profile.base_url_env, None);
        assert_eq!(profile.model_surface, TraceTier::Correlate);
        assert_eq!(profile.tool_surface, TraceTier::Correlate);
        assert!(profile.native_otel);
    }

    #[test]
    fn unknown_has_no_native_otel() {
        let profile = profile_for(TumultClient::Unknown);
        assert_eq!(profile.base_url_env, None);
        assert!(!profile.native_otel);
        assert_eq!(profile.model_surface, TraceTier::Correlate);
        assert_eq!(profile.tool_surface, TraceTier::Correlate);
    }

    #[test]
    fn parse_client_maps_canonical_names() {
        assert_eq!(parse_client("claude-code"), TumultClient::ClaudeCode);
        assert_eq!(parse_client("claude"), TumultClient::ClaudeCode);
        assert_eq!(parse_client("codex"), TumultClient::Codex);
        assert_eq!(parse_client("copilot"), TumultClient::Copilot);
        assert_eq!(parse_client("opencode"), TumultClient::OpenCode);
    }

    #[test]
    fn parse_client_is_case_insensitive() {
        assert_eq!(parse_client("Claude-Code"), TumultClient::ClaudeCode);
        assert_eq!(parse_client("CODEX"), TumultClient::Codex);
        assert_eq!(parse_client("  OpenCode  "), TumultClient::OpenCode);
    }

    #[test]
    fn parse_client_unrecognised_is_unknown() {
        assert_eq!(parse_client("gemini"), TumultClient::Unknown);
        assert_eq!(parse_client(""), TumultClient::Unknown);
        assert_eq!(parse_client("unknown"), TumultClient::Unknown);
    }
}
