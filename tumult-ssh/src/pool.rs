//! SSH session pool — reuse live connections across experiment activities.
//!
//! [`SshPool`] maintains one cached [`SshSession`] per `(host, port, user)`
//! key, wrapped in `Arc<Mutex<SshSession>>` so callers can hold a lock-guard
//! while executing commands.
//!
//! # Reconnection
//!
//! The pool probes each cached session with a no-op command (`true`) before
//! returning it. If the probe fails the stale entry is evicted and a fresh
//! connection is established using the stored [`SshConfig`].
//!
//! # Thread safety
//!
//! [`SshPool`] is `Send + Sync` and can be shared across Tokio tasks via
//! `Arc<SshPool>`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::config::SshConfig;
use crate::error::SshError;
use crate::session::SshSession;

/// Key that uniquely identifies a remote endpoint + user combination.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    host: String,
    port: u16,
    user: String,
}

impl PoolKey {
    fn from_config(config: &SshConfig) -> Self {
        Self {
            host: config.host.clone(),
            port: config.port,
            user: config.user.clone(),
        }
    }
}

/// A pooled SSH session entry: the live session plus the config used to
/// (re)create it.
struct PoolEntry {
    session: SshSession,
    config: SshConfig,
}

/// Pool of reusable SSH sessions.
///
/// Acquire a session with [`SshPool::acquire`]. The pool probes the cached
/// session before returning it and reconnects transparently if stale.
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use tumult_ssh::{SshConfig, SshPool};
///
/// # async fn run() -> Result<(), tumult_ssh::SshError> {
/// let pool = Arc::new(SshPool::new());
/// let config = SshConfig::with_agent("db-01", "ops");
/// let result = pool.execute(&config, "uptime").await?;
/// println!("{}", result.stdout);
/// # Ok(())
/// # }
/// ```
#[derive(Default)]
pub struct SshPool {
    entries: Mutex<HashMap<PoolKey, PoolEntry>>,
}

impl SshPool {
    /// Create an empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the number of cached sessions.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex has been poisoned (a previous thread panicked
    /// while holding the lock).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("pool lock poisoned").len()
    }

    /// Return `true` if no sessions are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Execute `command` on the host described by `config`, reusing a cached
    /// session when available.
    ///
    /// The pool takes ownership of the new connection if one is created.
    ///
    /// # Errors
    ///
    /// Returns [`SshError`] if the connection cannot be established or the
    /// command execution fails.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex has been poisoned.
    pub async fn execute(
        &self,
        config: &SshConfig,
        command: &str,
    ) -> Result<crate::session::CommandResult, SshError> {
        let key = PoolKey::from_config(config);

        // Take the entry out of the pool (if any) so we can call async methods
        // on it without holding the Mutex across await points.
        let entry = {
            let mut guard = self.entries.lock().expect("pool lock poisoned");
            guard.remove(&key)
        };

        let session = if let Some(e) = entry {
            // Probe the cached session with a no-op to detect staleness.
            if e.session.execute("true").await.is_ok() {
                e.session
            } else {
                tracing::debug!(
                    host = %key.host,
                    port = key.port,
                    "cached SSH session stale — reconnecting"
                );
                SshSession::connect(e.config.clone()).await?
            }
        } else {
            tracing::debug!(
                host = %key.host,
                port = key.port,
                "opening new SSH session"
            );
            SshSession::connect(config.clone()).await?
        };

        let result = session.execute(command).await;

        // Return the session to the pool (regardless of command success/failure)
        // so future calls can reuse the connection.  If the command itself errored
        // the session may be stale; the next probe will catch it.
        {
            let mut guard = self.entries.lock().expect("pool lock poisoned");
            guard.insert(
                key,
                PoolEntry {
                    session,
                    config: config.clone(),
                },
            );
        }

        result
    }

    /// Evict all cached sessions from the pool.
    ///
    /// Calling this does **not** send a disconnect message to the remote hosts.
    /// The underlying `russh` handles will be dropped, which closes the TCP
    /// connections.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex has been poisoned.
    pub fn clear(&self) {
        let mut guard = self.entries.lock().expect("pool lock poisoned");
        guard.clear();
    }

    /// Evict the cached session for the given config, if any.
    ///
    /// Returns `true` if an entry was removed.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex has been poisoned.
    pub fn evict(&self, config: &SshConfig) -> bool {
        let key = PoolKey::from_config(config);
        let mut guard = self.entries.lock().expect("pool lock poisoned");
        guard.remove(&key).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dummy_config(host: &str) -> SshConfig {
        SshConfig::with_key(host, "ops", PathBuf::from("/tmp/fake_key"))
    }

    #[test]
    fn pool_starts_empty() {
        let pool = SshPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn evict_missing_key_returns_false() {
        let pool = SshPool::new();
        let config = dummy_config("host-a");
        assert!(!pool.evict(&config));
    }

    #[test]
    fn clear_on_empty_pool_does_not_panic() {
        let pool = SshPool::new();
        pool.clear();
        assert!(pool.is_empty());
    }

    #[test]
    fn pool_key_equality_by_host_port_user() {
        let key1 = PoolKey {
            host: "db-01".into(),
            port: 22,
            user: "ops".into(),
        };
        let key2 = PoolKey {
            host: "db-01".into(),
            port: 22,
            user: "ops".into(),
        };
        let key3 = PoolKey {
            host: "db-01".into(),
            port: 2222,
            user: "ops".into(),
        };
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn pool_key_from_config_captures_fields() {
        let config = SshConfig::with_agent("web-01", "deploy").port(2222);
        let key = PoolKey::from_config(&config);
        assert_eq!(key.host, "web-01");
        assert_eq!(key.port, 2222);
        assert_eq!(key.user, "deploy");
    }

    #[test]
    fn default_creates_empty_pool() {
        let pool = SshPool::default();
        assert!(pool.is_empty());
    }
}
