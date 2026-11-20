//! Diagnostic log events shared by more than one command.
//!
//! Command *results* are printed to stdout; anything that is a diagnostic
//! about the run itself is a structured event from this module (or beside
//! the code that raises it) so it reaches stderr and the OTLP log pipeline
//! with a stable name and fields. See `docs/guides/logging.md`.

use std::fmt::Display;
use std::path::Path;

use tumult_otel::logging::{err, sanitize_url};

/// Which resolved map a key that cannot become an env var name came from.
#[derive(Clone, Copy, Debug)]
pub(super) enum EnvKey {
    /// `configuration`, injected as `TUMULT_CONFIG_*`.
    Config,
    /// `secrets`, injected as `TUMULT_SECRET_*`.
    Secret,
}

/// A journal in a directory scan could not be read and was left out.
pub(super) fn journal_skipped<E: Display + ?Sized>(path: &Path, error: &E) {
    tracing::warn!(
        name: "cli.journal.skipped",
        { file.path = %path.display(), error.message = %err(error) },
        "journal skipped"
    );
}

/// A configuration or secret key does not form a valid env var name, so it
/// is usable in templates but not injected into provider subprocesses.
///
/// Only the key is logged: the maps these keys come from carry resolved
/// secret values, which must never reach a log.
pub(super) fn env_key_skipped(kind: EnvKey, key: &str) {
    match kind {
        EnvKey::Config => tracing::warn!(
            name: "cli.config_key.skipped",
            { config.key = key },
            "configuration key is not a valid env var name, not injected as TUMULT_CONFIG_*"
        ),
        EnvKey::Secret => tracing::warn!(
            name: "cli.secret_key.skipped",
            { config.key = key },
            "secret key is not a valid env var name, not injected as TUMULT_SECRET_*"
        ),
    }
}

/// Logs every key in `keys` as skipped.
pub(super) fn env_keys_skipped(kind: EnvKey, keys: &[String]) {
    for key in keys {
        env_key_skipped(kind, key);
    }
}

/// The journal was written but could not be added to the analytics store.
pub(super) fn auto_ingest_failed(error: &anyhow::Error) {
    tracing::warn!(
        name: "cli.ingest.failed",
        { error.message = %err(error) },
        "journal not ingested into the analytics store"
    );
}

/// The run writes its journal to `ClickHouse`, a store that Tumult 3.1
/// removes.
pub(super) fn clickhouse_deprecated() {
    tracing::warn!(
        name: "cli.clickhouse.deprecated",
        "the ClickHouse store is deprecated and Tumult 3.1 removes it; unset TUMULT_CLICKHOUSE_URL to write to the lake"
    );
}

/// The daemon named by `TUMULT_DAEMON_URL` never answered; the CLI falls
/// back to writing the store directly. The URL is logged without userinfo
/// or query.
pub(super) fn daemon_unreachable(base: &str, reason: &str) {
    tracing::warn!(
        name: "cli.daemon.unreachable",
        { url.full = %sanitize_url(base), error.message = reason },
        "daemon unreachable, writing the store directly"
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tracing::Level;
    use tumult_otel::logging::testing::capture_json;

    use super::{auto_ingest_failed, env_key_skipped, journal_skipped, EnvKey};

    #[test]
    fn a_skipped_journal_names_the_file_and_the_cause() {
        let logs = capture_json(Level::WARN, || {
            journal_skipped(Path::new("/journals/bad.toon"), &"unexpected end of input");
        });
        let event = &logs.named("cli.journal.skipped")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["file.path"], "/journals/bad.toon");
        assert_eq!(event["error.message"], "unexpected end of input");
    }

    #[test]
    fn a_skipped_env_key_names_the_key_and_its_kind_never_a_value() {
        let logs = capture_json(Level::WARN, || {
            env_key_skipped(EnvKey::Config, "db host");
            env_key_skipped(EnvKey::Secret, "api key");
        });
        let config = &logs.named("cli.config_key.skipped")[0];
        assert_eq!(config["level"], "WARN");
        assert_eq!(config["config.key"], "db host");
        let secret = &logs.named("cli.secret_key.skipped")[0];
        assert_eq!(secret["config.key"], "api key");
        for event in &logs.events {
            assert!(event.get("config.value").is_none(), "{event}");
        }
    }

    #[test]
    fn a_failed_auto_ingest_keeps_the_whole_error_chain() {
        let error =
            anyhow::anyhow!("lock held by pid 42").context("failed to open analytics store");
        let logs = capture_json(Level::WARN, || auto_ingest_failed(&error));
        let event = &logs.named("cli.ingest.failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(
            event["error.message"],
            "failed to open analytics store: lock held by pid 42"
        );
    }
}
