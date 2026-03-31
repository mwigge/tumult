//! SSH session — connection management and command execution.
//!
//! Provides `SshSession` for connecting to remote hosts and executing
//! commands with stdout/stderr capture. Uses `russh` 0.58 internally.

use std::path::Path;
use std::sync::Arc;

use russh::client;
use russh::keys::ssh_key;

use crate::config::{AuthMethod, SshConfig};
use crate::error::SshError;

/// Result of executing a remote command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: u32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    /// Returns true if the command exited with code 0.
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
    pub async fn connect(config: SshConfig) -> Result<Self, SshError> {
        let auth_label = match &config.auth {
            crate::config::AuthMethod::Key { .. } => "key",
            crate::config::AuthMethod::Agent => "agent",
        };
        let _span = crate::telemetry::begin_connect(&config.host, config.port, auth_label);

        let ssh_config = Arc::new(client::Config {
            ..Default::default()
        });

        let handler = ClientHandler;
        let addr = format!("{}:{}", config.host, config.port);

        let mut handle = tokio::time::timeout(config.connect_timeout, async {
            client::connect(ssh_config, &addr, handler).await
        })
        .await
        .map_err(|_| SshError::Timeout {
            seconds: config.connect_timeout.as_secs_f64(),
        })?
        .map_err(|e| SshError::ConnectionFailed {
            host: config.host.clone(),
            port: config.port,
            reason: e.to_string(),
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

    /// Execute a command on the remote host.
    pub async fn execute(&self, command: &str) -> Result<CommandResult, SshError> {
        let _span = crate::telemetry::begin_execute(
            command,
            self.config.command_timeout.map(|d| d.as_secs_f64()),
        );
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::ChannelError(e.to_string()))?;

        channel
            .exec(true, command.to_string())
            .await
            .map_err(|e| SshError::ExecutionFailed(e.to_string()))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;
        let mut exit_signal: Option<String> = None;

        loop {
            let msg = if let Some(timeout) = self.config.command_timeout {
                tokio::time::timeout(timeout, channel.wait())
                    .await
                    .map_err(|_| SshError::Timeout {
                        seconds: timeout.as_secs_f64(),
                    })?
            } else {
                channel.wait().await
            };

            match msg {
                Some(russh::ChannelMsg::Data { data }) => {
                    stdout.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExtendedData { data, ext }) => {
                    if ext == 1 {
                        stderr.extend_from_slice(&data);
                    }
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = Some(exit_status);
                }
                Some(russh::ChannelMsg::ExitSignal {
                    signal_name,
                    core_dumped,
                    ..
                }) => {
                    let sig = format!(
                        "killed by signal: {:?}{}",
                        signal_name,
                        if core_dumped { " (core dumped)" } else { "" }
                    );
                    exit_signal = Some(sig);
                }
                // Don't break on Eof — ExitStatus may arrive after Eof per RFC 4254
                Some(russh::ChannelMsg::Eof) => {}
                None => break,
                _ => {}
            }
        }

        // Determine exit code: explicit status > signal > default failure
        let code = exit_code.unwrap_or(if exit_signal.is_some() { 137 } else { 1 });

        // Append signal info to stderr if present
        let mut stderr_str = String::from_utf8_lossy(&stderr).trim().to_string();
        if let Some(sig) = exit_signal {
            if !stderr_str.is_empty() {
                stderr_str.push('\n');
            }
            stderr_str.push_str(&sig);
        }

        let result = CommandResult {
            exit_code: code,
            stdout: String::from_utf8_lossy(&stdout).trim().to_string(),
            stderr: stderr_str,
        };
        crate::telemetry::event_command_completed(
            i64::from(result.exit_code),
            result.stdout.len(),
            result.stderr.len(),
        );
        Ok(result)
    }

    /// Upload a file to the remote host via SSH channel.
    ///
    /// Uses `cat > path` on the remote end. Requires a POSIX shell.
    /// The file is written with mode 755 (executable).
    pub async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<(), SshError> {
        let file_size = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let _span = crate::telemetry::begin_upload(remote_path, file_size);

        let content = tokio::fs::read(local_path)
            .await
            .map_err(|e| SshError::UploadFailed(format!("read local file: {}", e)))?;

        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::ChannelError(e.to_string()))?;

        let cmd = format!(
            "cat > {} && chmod 755 {}",
            shell_escape(remote_path),
            shell_escape(remote_path)
        );
        channel
            .exec(true, cmd)
            .await
            .map_err(|e| SshError::UploadFailed(e.to_string()))?;

        channel
            .data(&content[..])
            .await
            .map_err(|e| SshError::UploadFailed(e.to_string()))?;

        channel
            .eof()
            .await
            .map_err(|e| SshError::UploadFailed(e.to_string()))?;

        // Wait for completion with timeout
        let wait_fut = async {
            let mut got_exit_status = false;
            let mut exit_ok = true;

            loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        got_exit_status = true;
                        exit_ok = exit_status == 0;
                    }
                    Some(russh::ChannelMsg::Eof) => {}
                    None => break,
                    _ => {}
                }
            }

            if got_exit_status && !exit_ok {
                return Err(SshError::UploadFailed(
                    "remote write exited with non-zero status".to_string(),
                ));
            }
            Ok(())
        };

        if let Some(timeout) = self.config.command_timeout {
            tokio::time::timeout(timeout, wait_fut)
                .await
                .map_err(|_| SshError::Timeout {
                    seconds: timeout.as_secs_f64(),
                })??;
        } else {
            wait_fut.await?;
        }

        Ok(())
    }

    /// Close the SSH session.
    pub async fn close(self) -> Result<(), SshError> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "tumult session end", "en")
            .await
            .map_err(|e| SshError::ChannelError(e.to_string()))?;
        Ok(())
    }

    /// Get the config this session was created with.
    pub fn config(&self) -> &SshConfig {
        &self.config
    }
}

/// Simple shell escaping for remote paths.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Authenticate using the configured method.
async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    config: &SshConfig,
) -> Result<(), SshError> {
    match &config.auth {
        AuthMethod::Key {
            key_path,
            passphrase,
        } => {
            if !key_path.exists() {
                return Err(SshError::KeyNotFound {
                    path: key_path.display().to_string(),
                });
            }
            let key_pair = russh::keys::load_secret_key(key_path, passphrase.as_deref())
                .map_err(|e| SshError::KeyParseError(e.to_string()))?;

            let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key_pair), None);

            let auth_result = handle
                .authenticate_publickey(&config.user, key_with_alg)
                .await
                .map_err(|e| SshError::AuthenticationFailed {
                    host: config.host.clone(),
                    user: config.user.clone(),
                    reason: e.to_string(),
                })?;

            if !matches!(auth_result, russh::client::AuthResult::Success) {
                return Err(SshError::AuthenticationFailed {
                    host: config.host.clone(),
                    user: config.user.clone(),
                    reason: "key rejected by server".to_string(),
                });
            }
        }
        AuthMethod::Agent => {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| SshError::AuthenticationFailed {
                    host: config.host.clone(),
                    user: config.user.clone(),
                    reason: format!("agent connection failed: {}", e),
                })?;

            let identities =
                agent
                    .request_identities()
                    .await
                    .map_err(|e| SshError::AuthenticationFailed {
                        host: config.host.clone(),
                        user: config.user.clone(),
                        reason: format!("agent identities failed: {}", e),
                    })?;

            let mut authenticated = false;
            for identity in &identities {
                let pubkey = identity.public_key().into_owned();
                let result = handle
                    .authenticate_publickey_with(&config.user, pubkey, None, &mut agent)
                    .await;
                if let Ok(russh::client::AuthResult::Success) = result {
                    authenticated = true;
                    break;
                }
            }

            if !authenticated {
                return Err(SshError::AuthenticationFailed {
                    host: config.host.clone(),
                    user: config.user.clone(),
                    reason: "no agent identity accepted".to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Client handler for russh.
///
/// **SECURITY WARNING**: Currently accepts all host keys without verification.
/// This makes connections vulnerable to MITM attacks. Acceptable for:
/// - Trusted internal networks
/// - Ephemeral cloud instances where host keys change on every provision
/// - Development/testing environments
///
/// NOT acceptable for production use over untrusted networks.
/// TODO: Implement known_hosts verification with opt-in/opt-out configuration.
struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_result_success_on_zero_exit() {
        let result = CommandResult {
            exit_code: 0,
            stdout: "hello".into(),
            stderr: String::new(),
        };
        assert!(result.success());
    }

    #[test]
    fn command_result_failure_on_nonzero_exit() {
        let result = CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "error".into(),
        };
        assert!(!result.success());
    }

    #[test]
    fn command_result_failure_on_signal_exit() {
        let result = CommandResult {
            exit_code: 137,
            stdout: String::new(),
            stderr: "killed".into(),
        };
        assert!(!result.success());
        assert_eq!(result.exit_code, 137);
    }

    #[test]
    fn shell_escape_simple_path() {
        assert_eq!(shell_escape("/tmp/file.sh"), "'/tmp/file.sh'");
    }

    #[test]
    fn shell_escape_path_with_single_quote() {
        assert_eq!(shell_escape("/tmp/it's"), "'/tmp/it'\\''s'");
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
}
