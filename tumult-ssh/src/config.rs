//! SSH connection configuration.

use std::path::PathBuf;
use std::time::Duration;

/// Authentication method for SSH connections.
///
/// `Debug` is manually implemented to redact the passphrase field.
#[derive(Clone)]
pub enum AuthMethod {
    /// Authenticate with a private key file.
    Key {
        key_path: PathBuf,
        passphrase: Option<String>,
    },
    /// Authenticate via SSH agent (ssh-agent / pageant).
    Agent,
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key {
                key_path,
                passphrase,
            } => f
                .debug_struct("Key")
                .field("key_path", key_path)
                .field(
                    "passphrase",
                    if passphrase.is_some() {
                        &"[REDACTED]"
                    } else {
                        &"None"
                    },
                )
                .finish(),
            Self::Agent => write!(f, "Agent"),
        }
    }
}

/// Configuration for an SSH connection.
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    pub connect_timeout: Duration,
    pub command_timeout: Option<Duration>,
}

impl SshConfig {
    /// Create a new SSH config with key-based authentication.
    pub fn with_key(host: &str, user: &str, key_path: PathBuf) -> Self {
        Self {
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            auth: AuthMethod::Key {
                key_path,
                passphrase: None,
            },
            connect_timeout: Duration::from_secs(10),
            command_timeout: None,
        }
    }

    /// Create a new SSH config with SSH agent authentication.
    pub fn with_agent(host: &str, user: &str) -> Self {
        Self {
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            auth: AuthMethod::Agent,
            connect_timeout: Duration::from_secs(10),
            command_timeout: None,
        }
    }

    /// Set the SSH port (default: 22).
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the connection timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the command execution timeout.
    pub fn command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = Some(timeout);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_key_sets_defaults() {
        let config = SshConfig::with_key(
            "db-primary",
            "ops",
            PathBuf::from("/home/ops/.ssh/id_ed25519"),
        );
        assert_eq!(config.host, "db-primary");
        assert_eq!(config.port, 22);
        assert_eq!(config.user, "ops");
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert!(config.command_timeout.is_none());
        assert!(matches!(config.auth, AuthMethod::Key { .. }));
    }

    #[test]
    fn with_agent_sets_defaults() {
        let config = SshConfig::with_agent("web-01", "deploy");
        assert_eq!(config.host, "web-01");
        assert_eq!(config.user, "deploy");
        assert!(matches!(config.auth, AuthMethod::Agent));
    }

    #[test]
    fn builder_overrides_port_and_timeouts() {
        let config = SshConfig::with_agent("host", "user")
            .port(2222)
            .connect_timeout(Duration::from_secs(30))
            .command_timeout(Duration::from_secs(60));
        assert_eq!(config.port, 2222);
        assert_eq!(config.connect_timeout, Duration::from_secs(30));
        assert_eq!(config.command_timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn with_key_stores_correct_path() {
        let path = PathBuf::from("/home/ops/.ssh/id_ed25519");
        let config = SshConfig::with_key("host", "user", path.clone());
        match &config.auth {
            AuthMethod::Key {
                key_path,
                passphrase,
            } => {
                assert_eq!(key_path, &path);
                assert!(passphrase.is_none());
            }
            _ => panic!("expected Key auth"),
        }
    }

    #[test]
    fn debug_redacts_passphrase() {
        let auth = AuthMethod::Key {
            key_path: PathBuf::from("/tmp/key"),
            passphrase: Some("s3cret".into()),
        };
        let debug = format!("{:?}", auth);
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("s3cret"));
    }

    #[test]
    fn debug_shows_none_passphrase() {
        let auth = AuthMethod::Key {
            key_path: PathBuf::from("/tmp/key"),
            passphrase: None,
        };
        let debug = format!("{:?}", auth);
        assert!(debug.contains("None"));
        assert!(!debug.contains("REDACTED"));
    }
}
