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
            // The URL may embed credentials as userinfo or query parameters.
            .field("url", &crate::telemetry::redacted_url(&self.url))
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
    use tumult_test_utils::EnvGuard;

    use super::*;

    const VARIABLES: [&str; 5] = [
        "TUMULT_CLICKHOUSE_URL",
        "TUMULT_CLICKHOUSE_DATABASE",
        "TUMULT_CLICKHOUSE_USER",
        "TUMULT_CLICKHOUSE_PASSWORD",
        "TUMULT_CLICKHOUSE_QUERY_TIMEOUT_SECS",
    ];

    fn fields(config: &ClickHouseConfig) -> (&str, &str, &str, &str, Duration) {
        (
            &config.url,
            &config.database,
            &config.user,
            &config.password,
            config.query_timeout,
        )
    }

    #[test]
    fn from_env_falls_back_to_the_documented_defaults() {
        let mut env = EnvGuard::lock();
        for variable in VARIABLES {
            env.remove(variable);
        }

        let config = ClickHouseConfig::from_env();

        assert_eq!(
            fields(&config),
            (
                "http://localhost:8123",
                "tumult",
                "default",
                "",
                Duration::from_secs(30)
            )
        );
        assert_eq!(fields(&config), fields(&ClickHouseConfig::default()));
        assert!(!ClickHouseConfig::is_configured());
    }

    #[test]
    fn from_env_reads_every_variable() {
        let mut env = EnvGuard::lock();
        env.set("TUMULT_CLICKHOUSE_URL", "https://ch.example:8443")
            .set("TUMULT_CLICKHOUSE_DATABASE", "chaos")
            .set("TUMULT_CLICKHOUSE_USER", "alice")
            .set("TUMULT_CLICKHOUSE_PASSWORD", "hunter2")
            .set("TUMULT_CLICKHOUSE_QUERY_TIMEOUT_SECS", "7");

        let config = ClickHouseConfig::from_env();

        assert_eq!(
            fields(&config),
            (
                "https://ch.example:8443",
                "chaos",
                "alice",
                "hunter2",
                Duration::from_secs(7)
            )
        );
        assert!(ClickHouseConfig::is_configured());
    }

    #[test]
    fn from_env_ignores_an_unparseable_timeout() {
        let mut env = EnvGuard::lock();
        env.set("TUMULT_CLICKHOUSE_QUERY_TIMEOUT_SECS", "soon");

        assert_eq!(
            ClickHouseConfig::from_env().query_timeout,
            Duration::from_secs(30)
        );
    }

    #[test]
    fn debug_redacts_credentials_embedded_in_the_url() {
        let config = ClickHouseConfig {
            url: "http://admin:s3cret@ch:8123/db?password=hunter2".into(),
            ..ClickHouseConfig::default()
        };

        let debug_output = format!("{config:?}");

        assert!(
            debug_output.contains(r#"url: "http://ch:8123""#),
            "{debug_output}"
        );
        assert!(!debug_output.contains("s3cret"), "{debug_output}");
        assert!(!debug_output.contains("hunter2"), "{debug_output}");
    }

    #[test]
    fn debug_prints_a_constant_when_the_userinfo_runs_into_the_path_or_query() {
        for url in [
            "http://u:123/456@h",
            "http://admin:8123?x@ch",
            "http://admin:123#x@ch:8123",
            "http://admin/pw@ch:8123",
        ] {
            let config = ClickHouseConfig {
                url: url.into(),
                ..ClickHouseConfig::default()
            };

            let debug_output = format!("{config:?}");

            assert!(
                debug_output.contains(r#"url: "invalid-url""#),
                "{url}: {debug_output}"
            );
        }
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
