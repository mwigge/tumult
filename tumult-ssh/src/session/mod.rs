//! SSH session — connection management and command execution.
//!
//! Provides `SshSession` for connecting to remote hosts and executing
//! commands with stdout/stderr capture. Uses `russh` 0.65 internally.

use std::sync::Arc;

use russh::client;

use crate::config::SshConfig;
use crate::error::SshError;

mod auth;
mod exec;
mod handler;
mod known_hosts;

use auth::authenticate;
use handler::ClientHandler;
use known_hosts::resolve_known_hosts_path;

/// Result of executing a remote command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: u32,
    pub stdout: String,
    pub stderr: String,
}

/// Truncates `command` to at most 64 bytes for tracing, without splitting a
/// multi-byte UTF-8 character.
fn command_preview(command: &str) -> &str {
    let mut end = command.len().min(64);
    while !command.is_char_boundary(end) {
        end -= 1;
    }
    &command[..end]
}

impl CommandResult {
    /// Returns true if the command exited with code 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// An active SSH session to a remote host.
pub struct SshSession {
    handle: client::Handle<ClientHandler>,
    config: SshConfig,
}

impl SshSession {
    /// Connect to a remote host using the provided configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SshError::Timeout`] if the connection or authentication exceeds the configured timeout.
    /// Returns [`SshError::ConnectionFailed`] if the TCP connection cannot be established.
    /// Returns [`SshError::KeyNotFound`] if the key file does not exist.
    /// Returns [`SshError::KeyPermissionsTooOpen`] if the key file has insecure permissions.
    /// Returns [`SshError::KeyParseError`] if the key file cannot be parsed.
    /// Returns [`SshError::AuthenticationFailed`] if the server rejects authentication.
    /// Returns [`SshError::HostKeyNotFound`] if the server key is not in `known_hosts` (Verify policy).
    /// Returns [`SshError::HostKeyMismatch`] if the server key differs from the `known_hosts` entry.
    #[tracing::instrument(skip(config), fields(host = %config.host, port = config.port))]
    pub async fn connect(config: SshConfig) -> Result<Self, SshError> {
        let auth_label = match &config.auth {
            crate::config::AuthMethod::Key { .. } => "key",
            crate::config::AuthMethod::Agent => "agent",
        };
        let _span = crate::telemetry::begin_connect(&config.host, config.port, auth_label);

        let ssh_config = Arc::new(client::Config {
            ..Default::default()
        });

        let known_hosts_path = resolve_known_hosts_path(config.known_hosts_path.as_deref());
        let handler = ClientHandler {
            host: config.host.clone(),
            port: config.port,
            known_hosts_path,
            policy: config.host_key_policy.clone(),
        };
        let addr = format!("{}:{}", config.host, config.port);

        let mut handle = tokio::time::timeout(config.connect_timeout, async {
            client::connect(ssh_config, &addr, handler).await
        })
        .await
        .map_err(|_| SshError::Timeout {
            seconds: config.connect_timeout.as_secs_f64(),
        })?
        .map_err(|e| match e {
            // The handler's host-key rejections are already typed — pass them
            // through instead of flattening them into `ConnectionFailed`.
            e @ (SshError::HostKeyNotFound { .. } | SshError::HostKeyMismatch { .. }) => e,
            e => SshError::ConnectionFailed {
                host: config.host.clone(),
                port: config.port,
                reason: e.to_string(),
            },
        })?;

        // Authenticate (bounded by connect_timeout to prevent auth stalls)
        tokio::time::timeout(config.connect_timeout, authenticate(&mut handle, &config))
            .await
            .map_err(|_| SshError::Timeout {
                seconds: config.connect_timeout.as_secs_f64(),
            })??;

        crate::telemetry::event_auth_success(auth_label);
        Ok(Self { handle, config })
    }

    /// Close the SSH session.
    ///
    /// # Errors
    ///
    /// Returns [`SshError::ChannelError`] if the disconnect message cannot be sent.
    pub async fn close(self) -> Result<(), SshError> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "tumult session end", "en")
            .await
            .map_err(|e| SshError::ChannelError(e.to_string()))?;
        Ok(())
    }

    /// Get the config this session was created with.
    #[must_use]
    pub fn config(&self) -> &SshConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_result_succeeds_only_on_zero_exit() {
        for (exit_code, expected) in [(0, true), (1, false), (137, false)] {
            let result = CommandResult {
                exit_code,
                stdout: String::new(),
                stderr: String::new(),
            };
            assert_eq!(result.success(), expected, "exit code {exit_code}");
        }
    }

    #[test]
    fn error_formats_correctly() {
        let err = SshError::ConnectionFailed {
            host: "db-01".into(),
            port: 22,
            reason: "refused".into(),
        };
        assert!(err.to_string().contains("db-01:22"));

        let err = SshError::AuthenticationFailed {
            host: "web-01".into(),
            user: "deploy".into(),
            reason: "key rejected".into(),
        };
        assert!(err.to_string().contains("deploy@web-01"));
    }

    #[test]
    fn host_key_mismatch_error_includes_both_fingerprints() {
        let err = SshError::HostKeyMismatch {
            host: "myhost".into(),
            expected_fingerprint: "SHA256:expected".into(),
            actual_fingerprint: "SHA256:actual".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("myhost"));
        assert!(msg.contains("SHA256:expected"));
        assert!(msg.contains("SHA256:actual"));
    }
}
