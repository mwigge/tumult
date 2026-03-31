//! ClickHouse connection configuration.

use std::time::Duration;

/// Configuration for the ClickHouse analytics backend.
#[derive(Debug, Clone)]
pub struct ClickHouseConfig {
    /// ClickHouse HTTP URL (e.g., `http://localhost:8123`)
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
        Self {
            url: std::env::var("TUMULT_CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".into()),
            database: std::env::var("TUMULT_CLICKHOUSE_DATABASE")
                .unwrap_or_else(|_| "tumult".into()),
            user: std::env::var("TUMULT_CLICKHOUSE_USER").unwrap_or_else(|_| "default".into()),
            password: std::env::var("TUMULT_CLICKHOUSE_PASSWORD").unwrap_or_default(),
            query_timeout: Duration::from_secs(query_timeout_secs),
        }
    }

    /// Check if ClickHouse backend is configured via environment.
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
}
