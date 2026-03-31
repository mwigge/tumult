//! SSH connection configuration.

use std::path::PathBuf;
use std::time::Duration;

/// Policy for verifying the SSH server's host key against `known_hosts`.
///
/// Controls how `tumult-ssh` handles host key verification during connection
/// establishment. The default policy is [`HostKeyPolicy::Verify`].
#[non_exhaustive]
#[derive(Default, Debug, Clone, PartialEq)]
pub enum HostKeyPolicy {
    /// Verify the server key against the `known_hosts` file. Rejects connections
    /// for unknown or mismatched keys.
    ///
    /// This is the default and most secure policy.
    #[default]
    Verify,
    /// Trust On First Use: accept and record unknown keys, then verify on
    /// subsequent connections.
    ///
    /// Useful when connecting to freshly provisioned hosts whose keys are
    /// not yet in `known_hosts`.
    TrustOnFirstUse,
    /// Accept any server key without verification.
    ///
    /// **Security**: This policy is insecure and makes connections vulnerable
    /// to MITM attacks. Only use for ephemeral test infrastructure where
    /// host keys change on every provision and network security is ensured
    /// by other means.
    AcceptAny,
}

/// Authentication method for SSH connections.
///
/// `Debug` is manually implemented to redact the passphrase field.
#[non_exhaustive]
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
    /// Policy governing host key verification.
    ///
    /// Defaults to [`HostKeyPolicy::Verify`], which checks the server key
    /// against `known_hosts_path`. Set to [`HostKeyPolicy::AcceptAny`] only
    /// for ephemeral or trusted environments where MITM risk is acceptable.
    pub host_key_policy: HostKeyPolicy,
    /// Path to the `known_hosts` file used for host key verification.
    ///
    /// Defaults to `~/.ssh/known_hosts` when `None`.
    pub known_hosts_path: Option<PathBuf>,
}

impl SshConfig {
    /// Create a new SSH config with key-based authentication.
    #[must_use]
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
            host_key_policy: HostKeyPolicy::Verify,
            known_hosts_path: None,
        }
    }

    /// Create a new SSH config with SSH agent authentication.
    #[must_use]
    pub fn with_agent(host: &str, user: &str) -> Self {
        Self {
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            auth: AuthMethod::Agent,
            connect_timeout: Duration::from_secs(10),
            command_timeout: None,
            host_key_policy: HostKeyPolicy::Verify,
            known_hosts_path: None,
        }
    }

    /// Set the SSH port (default: 22).
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the connection timeout.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the command execution timeout.
    #[must_use]
    pub fn command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = Some(timeout);
        self
    }

    /// Set the host key verification policy.
    #[must_use]
    pub fn host_key_policy(mut self, policy: HostKeyPolicy) -> Self {
        self.host_key_policy = policy;
        self
    }

    /// Override the path to the `known_hosts` file.
    ///
    /// When not set, defaults to `~/.ssh/known_hosts` at connection time.
    #[must_use]
    pub fn known_hosts_path(mut self, path: PathBuf) -> Self {
        self.known_hosts_path = Some(path);
        self
    }

    /// Allow connecting to hosts with unrecognized server keys.
    ///
    /// **Security**: Defaults to `false`. Only enable for trusted networks or
    /// ephemeral instances where host keys change on every provision.
    ///
    /// This is a convenience wrapper: `true` maps to [`HostKeyPolicy::AcceptAny`],
    /// `false` maps to [`HostKeyPolicy::Verify`].
    #[must_use]
    pub fn allow_unknown_hosts(mut self, allow: bool) -> Self {
        self.host_key_policy = if allow {
            HostKeyPolicy::AcceptAny
        } else {
            HostKeyPolicy::Verify
        };
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
    fn host_key_policy_defaults_to_verify() {
        let config = SshConfig::with_agent("host", "user");
        assert_eq!(
            config.host_key_policy,
            HostKeyPolicy::Verify,
            "host_key_policy should default to Verify for security"
        );

        let config = SshConfig::with_key("host", "user", PathBuf::from("/tmp/key"));
        assert_eq!(
            config.host_key_policy,
            HostKeyPolicy::Verify,
            "host_key_policy should default to Verify for security"
        );
    }

    // Kept for backwards compatibility — allow_unknown_hosts(true) maps to AcceptAny
    #[test]
    fn allow_unknown_hosts_defaults_to_false() {
        let config = SshConfig::with_agent("host", "user");
        assert_ne!(
            config.host_key_policy,
            HostKeyPolicy::AcceptAny,
            "host_key_policy should not default to AcceptAny"
        );

        let config = SshConfig::with_key("host", "user", PathBuf::from("/tmp/key"));
        assert_ne!(
            config.host_key_policy,
            HostKeyPolicy::AcceptAny,
            "host_key_policy should not default to AcceptAny"
        );
    }

    #[test]
    fn allow_unknown_hosts_still_works() {
        // Backwards-compat: .allow_unknown_hosts(true) maps to AcceptAny
        let config = SshConfig::with_agent("host", "user").allow_unknown_hosts(true);
        assert_eq!(config.host_key_policy, HostKeyPolicy::AcceptAny);

        // .allow_unknown_hosts(false) maps to Verify
        let config = SshConfig::with_agent("host", "user").allow_unknown_hosts(false);
        assert_eq!(config.host_key_policy, HostKeyPolicy::Verify);
    }

    #[test]
    fn host_key_policy_builder() {
        let config =
            SshConfig::with_agent("host", "user").host_key_policy(HostKeyPolicy::TrustOnFirstUse);
        assert_eq!(config.host_key_policy, HostKeyPolicy::TrustOnFirstUse);
    }

    #[test]
    fn known_hosts_path_builder() {
        let path = PathBuf::from("/custom/known_hosts");
        let config = SshConfig::with_agent("host", "user").known_hosts_path(path.clone());
        assert_eq!(config.known_hosts_path, Some(path));
    }

    #[test]
    fn known_hosts_path_defaults_to_none() {
        let config = SshConfig::with_agent("host", "user");
        assert!(config.known_hosts_path.is_none());
    }

    #[test]
    fn debug_redacts_passphrase() {
        let auth = AuthMethod::Key {
            key_path: PathBuf::from("/tmp/key"),
            passphrase: Some("s3cret".into()),
        };
        let debug = format!("{auth:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("s3cret"));
    }

    #[test]
    fn debug_shows_none_passphrase() {
        let auth = AuthMethod::Key {
            key_path: PathBuf::from("/tmp/key"),
            passphrase: None,
        };
        let debug = format!("{auth:?}");
        assert!(debug.contains("None"));
        assert!(!debug.contains("REDACTED"));
    }
}
