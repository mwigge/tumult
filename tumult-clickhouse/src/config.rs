//! `ClickHouse` connection configuration.

use std::time::Duration;

/// Configuration for the `ClickHouse` analytics backend.
#[derive(Clone)]
pub struct ClickHouseConfig {
    /// `ClickHouse` HTTP URL (e.g., `http://localhost:8123`)
    pub url: String,
    /// Database name (default: `tumult`)
    pub database: String,
    /// Username (default: `default`)
    pub user: String,
    /// Password (default: empty)
    pub password: String,
    /// Timeout for individual query execution (default: 30s)
    pub query_timeout: Duration,
}

impl std::fmt::Debug for ClickHouseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickHouseConfig")
            .field("url", &self.url)
            .field("database", &self.database)
            .field("user", &self.user)
            // Redact password to prevent accidental leakage in logs or panic output.
            .field("password", &"[REDACTED]")
            .field("query_timeout", &self.query_timeout)
            .finish()
    }
}

impl ClickHouseConfig {
    /// Load config from environment variables.
    ///
    /// | Variable | Default |
    /// |----------|---------|
    /// | `TUMULT_CLICKHOUSE_URL` | `http://localhost:8123` |
    /// | `TUMULT_CLICKHOUSE_DATABASE` | `tumult` |
    /// | `TUMULT_CLICKHOUSE_USER` | `default` |
    /// | `TUMULT_CLICKHOUSE_PASSWORD` | (empty) |
    pub fn from_env() -> Self {
        let query_timeout_secs = std::env::var("TUMULT_CLICKHOUSE_QUERY_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
        let url = std::env::var("TUMULT_CLICKHOUSE_URL")
            .unwrap_or_else(|_| "http://localhost:8123".into());
        let password = std::env::var("TUMULT_CLICKHOUSE_PASSWORD").unwrap_or_default();
        if !password.is_empty() && url.starts_with("http://") {
            tracing::warn!(
                "ClickHouse password is set with an HTTP (non-TLS) URL; \
                 credentials will be sent in cleartext"
            );
        }
        Self {
            url,
            database: std::env::var("TUMULT_CLICKHOUSE_DATABASE")
                .unwrap_or_else(|_| "tumult".into()),
            user: std::env::var("TUMULT_CLICKHOUSE_USER").unwrap_or_else(|_| "default".into()),
            password,
            query_timeout: Duration::from_secs(query_timeout_secs),
        }
    }

    /// Check if `ClickHouse` backend is configured via environment.
    #[must_use]
    pub fn is_configured() -> bool {
        std::env::var("TUMULT_CLICKHOUSE_URL").is_ok()
    }
}

impl Default for ClickHouseConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8123".into(),
            database: "tumult".into(),
            user: "default".into(),
            password: String::new(),
            query_timeout: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = ClickHouseConfig::default();
        assert_eq!(config.url, "http://localhost:8123");
        assert_eq!(config.database, "tumult");
        assert_eq!(config.user, "default");
        assert!(config.password.is_empty());
        assert_eq!(config.query_timeout, Duration::from_secs(30));
    }

    #[test]
    fn from_env_uses_defaults_when_unset() {
        // Clear any existing env vars to test defaults
        std::env::remove_var("TUMULT_CLICKHOUSE_URL");
        let config = ClickHouseConfig::from_env();
        assert_eq!(config.url, "http://localhost:8123");
        assert_eq!(config.database, "tumult");
    }

    #[test]
    fn debug_redacts_password() {
        let config = ClickHouseConfig {
            url: "http://localhost:8123".into(),
            database: "tumult".into(),
            user: "alice".into(),
            password: "super-secret".into(),
            query_timeout: std::time::Duration::from_secs(30),
        };
        let debug_output = format!("{config:?}");
        assert!(
            !debug_output.contains("super-secret"),
            "Debug output must not contain plaintext password"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output must show [REDACTED] for password"
        );
    }
}
