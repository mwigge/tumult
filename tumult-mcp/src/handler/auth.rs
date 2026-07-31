//! MCP bearer-token authentication, role-based access control, and bind policy.
//!
//! Authentication is **fail-closed**: once any token is configured (via the
//! TOML auth config file or the legacy `TUMULT_MCP_TOKEN` env var) every
//! request must present a valid bearer token, an unknown token is rejected
//! (never elevated), and a malformed config refuses all requests rather than
//! running wide open. Only when *no* auth is configured at all does the server
//! run open — intended for loopback local development, and still gated by the
//! secure-by-default bind guard in [`crate::server::serve`].

use subtle::ConstantTimeEq;

pub use tumult_auth::Role;

/// Environment variable naming the TOML auth config file.
const AUTH_CONFIG_ENV: &str = "TUMULT_MCP_AUTH_CONFIG";
/// Legacy single-token environment variable (maps to the `operator` role).
const TOKEN_ENV: &str = "TUMULT_MCP_TOKEN";

/// Internal authentication state.
enum AuthMode {
    /// No auth configured — every request is allowed (loopback dev only).
    Open,
    /// One or more `token → role` mappings; every request must present a
    /// valid bearer token.
    Tokens(Vec<(String, Role)>),
    /// A config was present but failed to load — reject every request so a
    /// broken config never runs wide open.
    Denied,
}

/// MCP authentication configuration: a set of bearer tokens, each mapped to a
/// [`Role`].
///
/// Built via [`McpAuth::load`] (fallible, used at server startup) or
/// [`McpAuth::from_env`] (infallible, fail-closed on error). The legacy
/// single-token deployment (`TUMULT_MCP_TOKEN`) maps to a single `operator`
/// token so existing setups keep full access.
pub struct McpAuth {
    mode: AuthMode,
}

impl McpAuth {
    /// An unauthenticated configuration — all requests allowed. Loopback dev
    /// only (the bind guard refuses a network-exposed HTTP bind in this mode).
    #[must_use]
    pub fn none() -> Self {
        Self {
            mode: AuthMode::Open,
        }
    }

    /// A single `operator` token — the legacy `TUMULT_MCP_TOKEN` shape.
    #[must_use]
    pub fn single_operator(token: String) -> Self {
        Self {
            mode: AuthMode::Tokens(vec![(token, Role::Operator)]),
        }
    }

    /// Build from an explicit `token → role` table (order preserved).
    #[must_use]
    pub fn from_tokens(tokens: Vec<(String, Role)>) -> Self {
        if tokens.is_empty() {
            Self::none()
        } else {
            Self {
                mode: AuthMode::Tokens(tokens),
            }
        }
    }

    /// Whether any auth is configured (a token map or a failed-to-load config).
    ///
    /// The bind guard uses this to decide whether a non-loopback HTTP bind is
    /// permitted: an `Open` config on a network address is refused.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !matches!(self.mode, AuthMode::Open)
    }

    /// Resolve authentication config from the environment, fail-closed.
    ///
    /// Priority:
    /// 1. TOML auth config file — path from `TUMULT_MCP_AUTH_CONFIG`, or the
    ///    default `~/.tumult/mcp-auth.toml` when it exists.
    /// 2. Legacy single token `TUMULT_MCP_TOKEN` → `operator`.
    /// 3. Nothing configured → open (loopback dev).
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when a configured auth config file is
    /// missing, unreadable, or malformed. Startup must abort on this error
    /// rather than run without authentication.
    pub fn load() -> std::result::Result<Self, String> {
        // 1a. Explicit config path — a set-but-broken path is a hard error.
        if let Some(path) = std::env::var(AUTH_CONFIG_ENV)
            .ok()
            .filter(|s| !s.is_empty())
        {
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read MCP auth config '{path}': {e}"))?;
            return Ok(Self::from_tokens(parse_auth_config(&contents)?));
        }
        // 1b. Default path — only when it actually exists.
        if let Some(default_path) = default_config_path() {
            if default_path.exists() {
                let contents = std::fs::read_to_string(&default_path).map_err(|e| {
                    format!(
                        "failed to read MCP auth config '{}': {e}",
                        default_path.display()
                    )
                })?;
                return Ok(Self::from_tokens(parse_auth_config(&contents)?));
            }
        }
        // 2. Legacy single token → operator.
        if let Some(token) = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty()) {
            return Ok(Self::single_operator(token));
        }
        // 3. No auth configured.
        tracing::warn!(
            "no MCP auth configured (no {AUTH_CONFIG_ENV}, no {TOKEN_ENV}) — server running \
             without authentication; loopback binds only"
        );
        Ok(Self::none())
    }

    /// Infallible variant used by [`Default`] / [`crate::handler::TumultHandler`]
    /// constructors. On a config-load error it logs and returns a **deny-all**
    /// configuration (fail-closed) rather than panicking or running open.
    #[must_use]
    pub fn from_env() -> Self {
        Self::load().unwrap_or_else(|e| {
            tracing::error!("MCP auth config error: {e}; refusing all requests (fail-closed)");
            Self {
                mode: AuthMode::Denied,
            }
        })
    }

    /// Authenticate a request and resolve its [`Role`].
    ///
    /// Returns:
    /// - `Ok(None)` when no auth is configured (open mode — caller treats this
    ///   as full access for loopback dev).
    /// - `Ok(Some(role))` when a valid bearer token maps to `role`.
    /// - `Err(..)` when auth is configured and the token is missing, malformed,
    ///   or unknown.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ToolError::InvalidInput`] for a missing header,
    /// a non-`Bearer` scheme, or a token absent from the map.
    pub fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> std::result::Result<Option<Role>, crate::error::ToolError> {
        match &self.mode {
            AuthMode::Open => Ok(None),
            AuthMode::Denied => Err(crate::error::ToolError::InvalidInput(
                "authentication configuration failed to load; refusing all requests".into(),
            )),
            AuthMode::Tokens(tokens) => {
                let header = authorization.ok_or_else(|| {
                    crate::error::ToolError::InvalidInput("missing Authorization header".into())
                })?;
                let provided = header.strip_prefix("Bearer ").ok_or_else(|| {
                    crate::error::ToolError::InvalidInput(
                        "expected Bearer token in Authorization header".into(),
                    )
                })?;
                resolve_role(tokens, provided).map(Some).ok_or_else(|| {
                    crate::error::ToolError::InvalidInput("invalid bearer token".into())
                })
            }
        }
    }

    /// Check an Authorization header value without resolving the role.
    ///
    /// Preserves the pre-RBAC contract used by resource requests, which require
    /// authentication but not a specific role.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ToolError::InvalidInput`] if the token is
    /// missing, malformed, or not in the configured map.
    pub fn check(
        &self,
        authorization: Option<&str>,
    ) -> std::result::Result<(), crate::error::ToolError> {
        self.authenticate(authorization).map(|_| ())
    }
}

impl Default for McpAuth {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Constant-time resolution of a presented token against the configured map.
///
/// Every entry is compared (no early return) so the timing reveals only the
/// number of configured tokens and the presented length — never *which* token
/// matched. Comparison uses `subtle::ConstantTimeEq`.
fn resolve_role(tokens: &[(String, Role)], provided: &str) -> Option<Role> {
    let mut matched: Option<Role> = None;
    for (token, role) in tokens {
        let is_match: bool = provided.as_bytes().ct_eq(token.as_bytes()).into();
        if is_match {
            matched = Some(*role);
        }
    }
    matched
}

/// Default auth config path: `~/.tumult/mcp-auth.toml`.
fn default_config_path() -> Option<std::path::PathBuf> {
    dirs_next::home_dir().map(|home| home.join(".tumult").join("mcp-auth.toml"))
}

/// TOML shape of the auth config file:
///
/// ```toml
/// [[tokens]]
/// token = "<secret>"
/// role  = "viewer"   # or "operator" / "approver" / "admin"
/// ```
#[derive(serde::Deserialize)]
struct AuthConfigFile {
    #[serde(default)]
    tokens: Vec<AuthTokenEntry>,
}

#[derive(serde::Deserialize)]
struct AuthTokenEntry {
    token: String,
    role: String,
}

/// Parse and validate the auth config file contents into `token → role` pairs.
///
/// Fail-closed: an empty file, an empty token, an unknown role, or a duplicate
/// token is an error (the server must not start with an ambiguous config).
fn parse_auth_config(contents: &str) -> std::result::Result<Vec<(String, Role)>, String> {
    let parsed: AuthConfigFile =
        toml::from_str(contents).map_err(|e| format!("invalid MCP auth config: {e}"))?;
    if parsed.tokens.is_empty() {
        return Err("MCP auth config contains no [[tokens]] entries".into());
    }
    let mut tokens: Vec<(String, Role)> = Vec::with_capacity(parsed.tokens.len());
    for entry in parsed.tokens {
        if entry.token.is_empty() {
            return Err("MCP auth config has a [[tokens]] entry with an empty token".into());
        }
        let role = Role::parse(&entry.role).ok_or_else(|| {
            format!(
                "MCP auth config has an unknown role '{}' (expected 'viewer', 'operator', \
                 'approver', or 'admin')",
                entry.role
            )
        })?;
        if tokens.iter().any(|(t, _)| t == &entry.token) {
            return Err("MCP auth config has a duplicate token".into());
        }
        tokens.push((entry.token, role));
    }
    Ok(tokens)
}

/// Whether a bind host is loopback-only (safe to serve without a token).
///
/// Thin wrapper over [`tumult_auth::host_is_loopback`], kept so existing
/// callers (and the `handler::host_is_loopback` re-export) don't move.
/// Anything that is not loopback (including `0.0.0.0`, `::`, or a routable
/// address) is treated as network-exposed, which the server refuses to do
/// without configured authentication.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    tumult_auth::host_is_loopback(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Role ordering & parsing ───────────────────────────────────

    #[test]
    fn operator_role_satisfies_viewer_requirement() {
        // Operator ⊇ Viewer: Operator meets a Viewer requirement, and a
        // Viewer does not meet an Operator requirement.
        assert!(Role::Operator >= Role::Viewer);
        assert!(Role::Operator >= Role::Operator);
        assert!(Role::Viewer >= Role::Viewer);
        assert!(Role::Viewer < Role::Operator);
    }

    #[test]
    fn role_parse_is_case_insensitive_and_fails_closed() {
        assert_eq!(Role::parse("viewer"), Some(Role::Viewer));
        assert_eq!(Role::parse("OPERATOR"), Some(Role::Operator));
        assert_eq!(Role::parse(" Operator "), Some(Role::Operator));
        assert_eq!(Role::parse("approver"), Some(Role::Approver));
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("superuser"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn approver_and_admin_satisfy_operator_requirement() {
        // The MCP gate has two tiers (viewer / operator); approver and admin
        // tokens pass every operator-gated tool by the derived ordering.
        assert!(Role::Approver >= Role::Operator);
        assert!(Role::Admin >= Role::Operator);
        assert!(Role::Viewer < Role::Operator);
    }

    // ── Open (no auth) ────────────────────────────────────────────

    #[test]
    fn open_mode_allows_all_and_is_not_configured() {
        let auth = McpAuth::none();
        assert!(!auth.is_configured());
        assert_eq!(auth.authenticate(None).unwrap(), None);
        assert_eq!(auth.authenticate(Some("Bearer anything")).unwrap(), None);
        assert!(auth.check(None).is_ok());
    }

    // ── Single operator token (legacy) ────────────────────────────

    #[test]
    fn single_operator_token_maps_to_operator() {
        let auth = McpAuth::single_operator("secret123".into());
        assert!(auth.is_configured());
        assert_eq!(
            auth.authenticate(Some("Bearer secret123")).unwrap(),
            Some(Role::Operator)
        );
    }

    #[test]
    fn configured_auth_rejects_missing_header() {
        let auth = McpAuth::single_operator("secret123".into());
        let err = auth.authenticate(None).unwrap_err();
        assert!(err.to_string().contains("missing Authorization"));
    }

    #[test]
    fn configured_auth_rejects_wrong_token() {
        let auth = McpAuth::single_operator("secret123".into());
        let err = auth.authenticate(Some("Bearer wrong_token")).unwrap_err();
        assert!(err.to_string().contains("invalid bearer token"));
    }

    #[test]
    fn configured_auth_rejects_non_bearer_scheme() {
        let auth = McpAuth::single_operator("secret123".into());
        let err = auth.authenticate(Some("Basic dXNlcjpwYXNz")).unwrap_err();
        assert!(err.to_string().contains("expected Bearer token"));
    }

    // ── Multi-token map (viewer + operator) ───────────────────────

    #[test]
    fn token_map_resolves_distinct_roles() {
        let auth = McpAuth::from_tokens(vec![
            ("view-tok".into(), Role::Viewer),
            ("op-tok".into(), Role::Operator),
        ]);
        assert_eq!(
            auth.authenticate(Some("Bearer view-tok")).unwrap(),
            Some(Role::Viewer)
        );
        assert_eq!(
            auth.authenticate(Some("Bearer op-tok")).unwrap(),
            Some(Role::Operator)
        );
        assert!(auth
            .authenticate(Some("Bearer nope"))
            .unwrap_err()
            .to_string()
            .contains("invalid bearer token"));
    }

    #[test]
    fn from_tokens_empty_is_open() {
        assert!(!McpAuth::from_tokens(vec![]).is_configured());
    }

    // ── Denied (fail-closed) ──────────────────────────────────────

    #[test]
    fn denied_mode_rejects_everything_but_stays_configured() {
        let auth = McpAuth {
            mode: AuthMode::Denied,
        };
        assert!(auth.is_configured());
        assert!(auth.authenticate(Some("Bearer whatever")).is_err());
        assert!(auth.authenticate(None).is_err());
    }

    // ── Config parsing ────────────────────────────────────────────

    #[test]
    fn parse_config_valid_four_roles() {
        let toml = r#"
            [[tokens]]
            token = "viewer-secret"
            role  = "viewer"

            [[tokens]]
            token = "operator-secret"
            role  = "operator"

            [[tokens]]
            token = "approver-secret"
            role  = "approver"

            [[tokens]]
            token = "admin-secret"
            role  = "admin"
        "#;
        let tokens = parse_auth_config(toml).expect("valid config must parse");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0], ("viewer-secret".into(), Role::Viewer));
        assert_eq!(tokens[1], ("operator-secret".into(), Role::Operator));
        assert_eq!(tokens[2], ("approver-secret".into(), Role::Approver));
        assert_eq!(tokens[3], ("admin-secret".into(), Role::Admin));
    }

    #[test]
    fn parse_config_rejects_unknown_role() {
        let toml = r#"
            [[tokens]]
            token = "x"
            role  = "superuser"
        "#;
        let err = parse_auth_config(toml).unwrap_err();
        assert!(err.contains("unknown role"), "got: {err}");
    }

    #[test]
    fn parse_config_rejects_empty_token() {
        let toml = r#"
            [[tokens]]
            token = ""
            role  = "viewer"
        "#;
        let err = parse_auth_config(toml).unwrap_err();
        assert!(err.contains("empty token"), "got: {err}");
    }

    #[test]
    fn parse_config_rejects_duplicate_token() {
        let toml = r#"
            [[tokens]]
            token = "dup"
            role  = "viewer"

            [[tokens]]
            token = "dup"
            role  = "operator"
        "#;
        let err = parse_auth_config(toml).unwrap_err();
        assert!(err.contains("duplicate token"), "got: {err}");
    }

    #[test]
    fn parse_config_rejects_empty_and_malformed() {
        assert!(parse_auth_config("").unwrap_err().contains("no [[tokens]]"));
        assert!(parse_auth_config("not = valid = toml")
            .unwrap_err()
            .contains("invalid MCP auth config"));
        // Missing required field (role).
        let err = parse_auth_config("[[tokens]]\ntoken = \"x\"\n").unwrap_err();
        assert!(err.contains("invalid MCP auth config"), "got: {err}");
    }

    // ── Constant-time comparison (unchanged security posture) ─────

    #[test]
    fn auth_constant_time_comparison() {
        let auth = McpAuth::single_operator("super-secret-token".into());
        // Prefix, one-bit-off, and length-mismatch tokens must all be rejected.
        assert!(auth.authenticate(Some("Bearer super-secret-toke")).is_err());
        assert!(auth
            .authenticate(Some("Bearer super-secret-tokenXXXX"))
            .is_err());
        let mut one_off = *b"super-secret-token";
        one_off[17] ^= 0x01;
        let one_off = format!("Bearer {}", std::str::from_utf8(&one_off).unwrap());
        assert!(auth.authenticate(Some(&one_off)).is_err());
        assert!(auth.authenticate(Some("Bearer super-secret-token")).is_ok());
    }

    // ── Bind policy ───────────────────────────────────────────────

    #[test]
    fn loopback_hosts_are_recognised() {
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("::1"));
        assert!(host_is_loopback("[::1]"));
    }

    #[test]
    fn network_exposed_hosts_are_not_loopback() {
        assert!(!host_is_loopback("0.0.0.0"));
        assert!(!host_is_loopback("::"));
        assert!(!host_is_loopback("192.168.1.10"));
        assert!(!host_is_loopback("10.0.0.5"));
    }
}
