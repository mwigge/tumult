//! MCP bearer-token authentication and bind-address policy.

use subtle::ConstantTimeEq;

/// MCP authentication configuration.
///
/// If `TUMULT_MCP_TOKEN` is set, bearer token authentication is required
/// on all requests. If not set, the server runs without authentication
/// (with a warning logged).
pub struct McpAuth {
    pub(crate) token: Option<String>,
}

impl McpAuth {
    /// Read authentication config from environment.
    pub fn from_env() -> Self {
        let token = std::env::var("TUMULT_MCP_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        if token.is_none() {
            tracing::warn!("TUMULT_MCP_TOKEN not set — MCP server running without authentication");
        }
        Self { token }
    }

    /// Check an Authorization header value. Returns Ok(()) if valid.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ToolError::InvalidInput`] if the token is
    /// missing or does not match the configured bearer token.
    pub fn check(
        &self,
        authorization: Option<&str>,
    ) -> std::result::Result<(), crate::error::ToolError> {
        match &self.token {
            None => Ok(()), // no token configured, allow all
            Some(expected) => match authorization {
                Some(header) => {
                    let prefix = "Bearer ";
                    if let Some(provided) = header.strip_prefix(prefix) {
                        // Use constant-time comparison to prevent timing side-channel attacks.
                        let matches = provided.as_bytes().ct_eq(expected.as_bytes()).into();
                        if matches {
                            Ok(())
                        } else {
                            Err(crate::error::ToolError::InvalidInput(
                                "invalid bearer token".into(),
                            ))
                        }
                    } else {
                        Err(crate::error::ToolError::InvalidInput(
                            "expected Bearer token in Authorization header".into(),
                        ))
                    }
                }
                None => Err(crate::error::ToolError::InvalidInput(
                    "missing Authorization header".into(),
                )),
            },
        }
    }
}

/// Whether a bind host is loopback-only (safe to serve without a token).
///
/// Recognises the common loopback spellings. Anything else (including
/// `0.0.0.0`, `::`, or a routable address) is treated as network-exposed, which
/// the server refuses to do without a configured `TUMULT_MCP_TOKEN`.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    match host.trim().trim_matches(|c| c == '[' || c == ']') {
        "localhost" => true,
        h => h
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_no_token_configured_allows_all() {
        let auth = McpAuth { token: None };
        assert!(auth.check(None).is_ok());
        assert!(auth.check(Some("Bearer anything")).is_ok());
    }

    #[test]
    fn auth_with_token_accepts_valid_bearer() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        assert!(auth.check(Some("Bearer secret123")).is_ok());
    }

    #[test]
    fn auth_with_token_rejects_missing_header() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing Authorization"));
    }

    #[test]
    fn auth_with_token_rejects_wrong_token() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Bearer wrong_token"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid bearer token"));
    }

    #[test]
    fn auth_with_token_rejects_non_bearer_scheme() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Basic dXNlcjpwYXNz"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected Bearer token"));
    }

    /// Verify constant-time comparison is used: tokens that differ only in a
    /// single bit (or by length) are still rejected, and the comparison does
    /// not short-circuit on a matching prefix.
    #[test]
    fn auth_constant_time_comparison() {
        use subtle::ConstantTimeEq;

        let expected = b"super-secret-token";
        // Shorter slice — must not match, even though it is a prefix.
        let short = b"super-secret-toke";
        let matches: bool = short.ct_eq(expected).into();
        assert!(!matches, "short token must not match expected");

        // One-bit-off: change last byte.
        let mut one_off = *expected;
        one_off[expected.len() - 1] ^= 0x01;
        let matches: bool = one_off.ct_eq(expected).into();
        assert!(!matches, "one-bit-different token must not match expected");

        // Longer than expected — different length, must not match.
        let long = b"super-secret-tokenXXXX";
        let matches: bool = long.ct_eq(expected).into();
        assert!(!matches, "longer token must not match expected");

        // Positive case: exact match must succeed.
        let matches: bool = expected.ct_eq(expected).into();
        assert!(matches, "exact match must succeed");

        // End-to-end via McpAuth.check (length-prefix variant).
        let auth = McpAuth {
            token: Some("super-secret-token".into()),
        };
        assert!(auth.check(Some("Bearer super-secret-toke")).is_err());
        assert!(auth.check(Some("Bearer super-secret-tokenXXXX")).is_err());
        assert!(auth.check(Some("Bearer super-secret-token")).is_ok());
    }

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
