//! SSH session — connection management and command execution.
//!
//! Provides `SshSession` for connecting to remote hosts and executing
//! commands with stdout/stderr capture. Uses `russh` 0.58 internally.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client;
use russh::keys::ssh_key;
use russh::keys::ssh_key::known_hosts::KnownHosts;
use russh::keys::ssh_key::HashAlg;

use crate::config::{AuthMethod, HostKeyPolicy, SshConfig};
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
    ///
    /// # Errors
    ///
    /// Returns [`SshError::ChannelError`] if a channel cannot be opened.
    /// Returns [`SshError::ExecutionFailed`] if the command cannot be sent.
    /// Returns [`SshError::Timeout`] if the command exceeds the configured timeout.
    #[tracing::instrument(skip(self), fields(command_preview = &command[..command.len().min(64)]))]
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
    ///
    /// # Errors
    ///
    /// Returns [`SshError::InvalidPath`] if `remote_path` contains control characters.
    /// Returns [`SshError::UploadFailed`] if the local file cannot be read or the remote write fails.
    /// Returns [`SshError::ChannelError`] if a channel cannot be opened.
    /// Returns [`SshError::Timeout`] if the upload exceeds the configured command timeout.
    #[tracing::instrument(skip(self), fields(remote_path = %remote_path))]
    pub async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<(), SshError> {
        let file_size = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let _span = crate::telemetry::begin_upload(remote_path, file_size);

        let content = tokio::fs::read(local_path)
            .await
            .map_err(|e| SshError::UploadFailed(format!("read local file: {e}")))?;

        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::ChannelError(e.to_string()))?;

        let cmd = format!(
            "cat > {} && chmod 755 {}",
            shell_escape(remote_path)?,
            shell_escape(remote_path)?
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

/// Simple shell escaping for remote paths.
///
/// # Errors
///
/// Returns [`SshError::InvalidPath`] if `s` contains any ASCII control
/// character (U+0000–U+001F or U+007F), which would allow shell command
/// injection via an embedded newline or similar bypass.
fn shell_escape(s: &str) -> Result<String, SshError> {
    if s.chars().any(|c| c.is_ascii_control()) {
        return Err(SshError::InvalidPath { path: s.to_owned() });
    }
    Ok(format!("'{}'", s.replace('\'', "'\\''")))
}

/// Resolve the `known_hosts` file path: use provided path or fall back to `~/.ssh/known_hosts`.
fn resolve_known_hosts_path(override_path: Option<&Path>) -> PathBuf {
    if let Some(p) = override_path {
        return p.to_path_buf();
    }
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("/root"))
        .join(".ssh")
        .join("known_hosts")
}

/// Compute the SHA-256 fingerprint of a public key as a display string.
///
/// Returns a string in the form `SHA256:<base64>`.
fn key_fingerprint(key: &ssh_key::PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Build the host pattern string used in `known_hosts` for a given host and port.
///
/// For port 22, the pattern is just the hostname. For non-standard ports, the
/// pattern uses the bracket notation `[host]:port`.
fn known_hosts_host_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Check whether `entry_patterns` matches the given host and port.
///
/// Supports plain hostname, `[host]:port` bracket notation, and simple `*`/`?` globs.
/// Does not match hashed entries (`|1|…`) — those are silently skipped.
fn entry_matches_host(
    patterns: &ssh_key::known_hosts::HostPatterns,
    host: &str,
    port: u16,
) -> bool {
    let ssh_key::known_hosts::HostPatterns::Patterns(pats) = patterns else {
        // Hashed entries cannot be matched by plain hostname lookup
        return false;
    };
    let target_bracketed = format!("[{host}]:{port}");
    for pat in pats {
        // Negated patterns (starting with '!') count as non-matching for our use case
        if pat.starts_with('!') {
            continue;
        }
        if pat == host && port == 22 {
            return true;
        }
        if pat == &target_bracketed {
            return true;
        }
        // Simple glob matching: '*' matches any hostname segment
        if glob_matches(pat, host) && port == 22 {
            return true;
        }
    }
    false
}

/// Minimal glob matching supporting `*` (any sequence) and `?` (single char).
fn glob_matches(pattern: &str, haystack: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let hay: Vec<char> = haystack.chars().collect();
    glob_match_inner(&pat, &hay)
}

fn glob_match_inner(pat: &[char], hay: &[char]) -> bool {
    match (pat.first(), hay.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // '*' matches zero or more characters
            glob_match_inner(&pat[1..], hay)
                || (!hay.is_empty() && glob_match_inner(pat, &hay[1..]))
        }
        (Some('?'), Some(_)) => glob_match_inner(&pat[1..], &hay[1..]),
        (Some(p), Some(h)) => p == h && glob_match_inner(&pat[1..], &hay[1..]),
        (None, Some(_)) | (Some(_), None) => false,
    }
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

            // On Unix, reject key files with permissions more open than 0o600
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let metadata = tokio::fs::metadata(key_path).await.map_err(|e| {
                    SshError::KeyParseError(format!("failed to read key metadata: {e}"))
                })?;
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o177 != 0 {
                    return Err(SshError::KeyPermissionsTooOpen {
                        path: key_path.display().to_string(),
                        mode,
                    });
                }
            }

            let key_pair =
                russh::keys::load_secret_key(key_path, passphrase.as_deref().map(String::as_str))
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
                    reason: format!("agent connection failed: {e}"),
                })?;

            let identities =
                agent
                    .request_identities()
                    .await
                    .map_err(|e| SshError::AuthenticationFailed {
                        host: config.host.clone(),
                        user: config.user.clone(),
                        reason: format!("agent identities failed: {e}"),
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

/// Client handler for russh, implementing host key verification.
///
/// Supports three policies:
/// - [`HostKeyPolicy::Verify`]: checks against `known_hosts`, rejects unknown/mismatched keys
/// - [`HostKeyPolicy::TrustOnFirstUse`]: accepts and records first-seen keys; verifies thereafter
/// - [`HostKeyPolicy::AcceptAny`]: bypasses verification (insecure — only for ephemeral infra)
struct ClientHandler {
    host: String,
    port: u16,
    known_hosts_path: PathBuf,
    policy: HostKeyPolicy,
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match self.policy {
            HostKeyPolicy::AcceptAny => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    "accepting unverified host key (host_key_policy is AcceptAny)"
                );
                Ok(true)
            }
            HostKeyPolicy::Verify => {
                verify_host_key(
                    &self.host,
                    self.port,
                    &self.known_hosts_path,
                    server_public_key,
                )
                .await
            }
            HostKeyPolicy::TrustOnFirstUse => {
                trust_on_first_use(
                    &self.host,
                    self.port,
                    &self.known_hosts_path,
                    server_public_key,
                )
                .await
            }
        }
    }
}

/// Verify a server key against the `known_hosts` file (strict verification).
///
/// Returns `Ok(true)` if a matching entry is found and the key matches.
/// Returns `Err(SshError::HostKeyNotFound)` if no entry exists for the host.
/// Returns `Err(SshError::HostKeyMismatch)` if an entry exists but the key differs.
async fn verify_host_key(
    host: &str,
    port: u16,
    known_hosts_path: &Path,
    server_key: &ssh_key::PublicKey,
) -> Result<bool, SshError> {
    let actual_fp = key_fingerprint(server_key);

    let file_content = match tokio::fs::read_to_string(known_hosts_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No known_hosts file → treat as not found
            return Err(SshError::HostKeyNotFound {
                host: host.to_string(),
                fingerprint: actual_fp,
            });
        }
        Err(e) => {
            return Err(SshError::KnownHostsIo {
                path: known_hosts_path.display().to_string(),
                reason: e.to_string(),
            });
        }
    };

    find_and_verify_entry(host, port, &file_content, &actual_fp)
}

/// Verify host key or record it on first use (TOFU policy).
///
/// Returns `Ok(true)` if verified or newly recorded.
/// Returns `Err(SshError::HostKeyMismatch)` if a stored key differs from the server's key.
async fn trust_on_first_use(
    host: &str,
    port: u16,
    known_hosts_path: &Path,
    server_key: &ssh_key::PublicKey,
) -> Result<bool, SshError> {
    let actual_fp = key_fingerprint(server_key);

    let file_content = match tokio::fs::read_to_string(known_hosts_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(SshError::KnownHostsIo {
                path: known_hosts_path.display().to_string(),
                reason: e.to_string(),
            });
        }
    };

    if !file_content.is_empty() {
        // Check if there is an existing entry for this host
        let found = KnownHosts::new(&file_content)
            .filter_map(Result::ok)
            .any(|e| entry_matches_host(e.host_patterns(), host, port));

        if found {
            // Entry exists — verify strictly
            return find_and_verify_entry(host, port, &file_content, &actual_fp);
        }
    }

    // No entry found — add to known_hosts (TOFU)
    tracing::info!(
        host = %host,
        port = port,
        fingerprint = %actual_fp,
        "TOFU: adding new host key to known_hosts"
    );
    append_known_hosts_entry(host, port, known_hosts_path, server_key).await?;
    Ok(true)
}

/// Search for a matching entry in `file_content` and verify the key.
fn find_and_verify_entry(
    host: &str,
    port: u16,
    file_content: &str,
    actual_fp: &str,
) -> Result<bool, SshError> {
    for entry_result in KnownHosts::new(file_content) {
        let Ok(entry) = entry_result else { continue };
        if !entry_matches_host(entry.host_patterns(), host, port) {
            continue;
        }
        // Found a matching entry — compare keys
        let stored_fp = key_fingerprint(entry.public_key());
        if stored_fp == actual_fp {
            return Ok(true);
        }
        return Err(SshError::HostKeyMismatch {
            host: host.to_string(),
            expected_fingerprint: stored_fp,
            actual_fingerprint: actual_fp.to_string(),
        });
    }

    Err(SshError::HostKeyNotFound {
        host: host.to_string(),
        fingerprint: actual_fp.to_string(),
    })
}

/// Append a new `known_hosts` entry for the given host and key.
///
/// Creates parent directories if they don't exist.
async fn append_known_hosts_entry(
    host: &str,
    port: u16,
    known_hosts_path: &Path,
    key: &ssh_key::PublicKey,
) -> Result<(), SshError> {
    use tokio::io::AsyncWriteExt as _;

    // Create parent directory if needed
    if let Some(parent) = known_hosts_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SshError::KnownHostsIo {
                    path: parent.display().to_string(),
                    reason: e.to_string(),
                })?;
        }
    }

    let host_pattern = known_hosts_host_pattern(host, port);
    // PublicKey::to_string() gives "algorithm base64" without comment
    let key_str = key.to_string();
    let line = format!("{host_pattern} {key_str}\n");

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(known_hosts_path)
        .await
        .map_err(|e| SshError::KnownHostsIo {
            path: known_hosts_path.display().to_string(),
            reason: e.to_string(),
        })?;

    file.write_all(line.as_bytes())
        .await
        .map_err(|e| SshError::KnownHostsIo {
            path: known_hosts_path.display().to_string(),
            reason: e.to_string(),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A known ed25519 test key pair (public only needed here)
    const TEST_KEY_1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test@host";
    // A different key for mismatch testing
    const TEST_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti user@example.com";

    fn parse_key(s: &str) -> ssh_key::PublicKey {
        ssh_key::PublicKey::from_openssh(s).expect("valid test key")
    }

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
        assert_eq!(
            shell_escape("/tmp/file.sh").expect("valid path"),
            "'/tmp/file.sh'"
        );
    }

    #[test]
    fn shell_escape_path_with_single_quote() {
        assert_eq!(
            shell_escape("/tmp/it's").expect("valid path"),
            "'/tmp/it'\\''s'"
        );
    }

    #[test]
    fn shell_escape_rejects_path_with_embedded_newline() {
        // A path containing '\n' could inject arbitrary shell commands after
        // the escaped argument — e.g. "/tmp/x\nrm -rf /". Ensure the function
        // returns an error instead of producing a bypassable escaped string.
        let result = shell_escape("/tmp/evil\nrm -rf /");
        assert!(
            matches!(result, Err(SshError::InvalidPath { .. })),
            "expected InvalidPath error, got {result:?}"
        );
    }

    #[test]
    fn shell_escape_rejects_path_with_carriage_return() {
        let result = shell_escape("/tmp/evil\r");
        assert!(
            matches!(result, Err(SshError::InvalidPath { .. })),
            "expected InvalidPath error, got {result:?}"
        );
    }

    #[test]
    fn shell_escape_rejects_path_with_nul_byte() {
        let result = shell_escape("/tmp/evil\x00");
        assert!(
            matches!(result, Err(SshError::InvalidPath { .. })),
            "expected InvalidPath error, got {result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn key_permissions_too_open_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let key_path = dir.path().join("id_test");
        std::fs::write(&key_path, "fake-key-content").unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = SshError::KeyPermissionsTooOpen {
            path: key_path.display().to_string(),
            mode: 0o644,
        };
        assert!(err.to_string().contains("permissions too open"));
        assert!(err.to_string().contains("0o644"));

        // Verify the check logic directly: mode & 0o177 != 0 means too open
        let metadata = std::fs::metadata(&key_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_ne!(mode & 0o177, 0, "0o644 should be considered too open");

        // Verify 0o600 passes the check
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let metadata = std::fs::metadata(&key_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode & 0o177, 0, "0o600 should pass the permission check");
    }

    // ── Host key verification tests ───────────────────────────

    /// `AcceptAny` policy: accept without consulting `known_hosts`
    #[test]
    fn check_server_key_accepts_when_accept_any() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");

        let mut handler = ClientHandler {
            host: "testhost".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::AcceptAny,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(result.is_ok(), "AcceptAny should accept");
        assert!(result.unwrap(), "should return true");
    }

    /// Verify policy: matching key → accepted
    #[test]
    fn check_server_key_verifies_matching_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        let key = parse_key(TEST_KEY_1);
        let fp = key_fingerprint(&key);

        // Write known_hosts with matching entry
        let entry_line = format!("testhost {}\n", key.to_string());
        std::fs::write(&known_hosts, &entry_line).unwrap();

        let mut handler = ClientHandler {
            host: "testhost".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::Verify,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(
            result.is_ok(),
            "Verify: matching key should be accepted (fp: {fp})"
        );
        assert!(result.unwrap());
    }

    /// Verify policy: unknown host → `HostKeyNotFound` error
    #[test]
    fn check_server_key_rejects_unknown_key_in_verify_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        // Empty known_hosts
        std::fs::write(&known_hosts, "").unwrap();

        let mut handler = ClientHandler {
            host: "unknown-host".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::Verify,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(result.is_err(), "Verify: unknown host should be rejected");
        assert!(
            matches!(result.unwrap_err(), SshError::HostKeyNotFound { .. }),
            "expected HostKeyNotFound"
        );
    }

    /// Verify policy: key mismatch → `HostKeyMismatch` error
    #[test]
    fn check_server_key_rejects_mismatched_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");

        // Store key 1 in known_hosts
        let stored_key = parse_key(TEST_KEY_1);
        std::fs::write(
            &known_hosts,
            format!("testhost {}\n", stored_key.to_string()),
        )
        .unwrap();

        // Present key 2 as the server key
        let server_key = parse_key(TEST_KEY_2);

        let mut handler = ClientHandler {
            host: "testhost".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::Verify,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &server_key));
        assert!(result.is_err(), "Verify: mismatched key should be rejected");
        assert!(
            matches!(result.unwrap_err(), SshError::HostKeyMismatch { .. }),
            "expected HostKeyMismatch"
        );
    }

    /// TOFU: first connection adds key to `known_hosts`
    #[tokio::test]
    async fn trust_on_first_use_adds_new_host() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        // known_hosts does not exist yet

        let key = parse_key(TEST_KEY_1);
        let mut handler = ClientHandler {
            host: "new-server".to_string(),
            port: 22,
            known_hosts_path: known_hosts.clone(),
            policy: HostKeyPolicy::TrustOnFirstUse,
        };
        let result = client::Handler::check_server_key(&mut handler, &key).await;
        assert!(result.is_ok(), "TOFU: new host should be accepted");
        assert!(result.unwrap());

        // known_hosts should now exist and contain the key
        let contents = tokio::fs::read_to_string(&known_hosts).await.unwrap();
        assert!(
            contents.contains("new-server"),
            "known_hosts should contain host"
        );
        assert!(
            contents.contains("ssh-ed25519"),
            "known_hosts should contain key type"
        );
    }

    /// TOFU: second connection verifies stored key
    #[test]
    fn trust_on_first_use_verifies_known_host() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");

        let key = parse_key(TEST_KEY_1);

        // Simulate first connection: write key to known_hosts
        std::fs::write(&known_hosts, format!("known-server {}\n", key.to_string())).unwrap();

        // Second connection: verify
        let mut handler = ClientHandler {
            host: "known-server".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::TrustOnFirstUse,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(
            result.is_ok(),
            "TOFU: known host with matching key should be accepted"
        );
        assert!(result.unwrap());
    }

    /// TOFU: mismatch on known host → `HostKeyMismatch`
    #[test]
    fn trust_on_first_use_rejects_mismatched_known_host() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");

        let stored_key = parse_key(TEST_KEY_1);
        std::fs::write(
            &known_hosts,
            format!("known-server {}\n", stored_key.to_string()),
        )
        .unwrap();

        let server_key = parse_key(TEST_KEY_2);
        let mut handler = ClientHandler {
            host: "known-server".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::TrustOnFirstUse,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &server_key));
        assert!(result.is_err(), "TOFU: key mismatch should be rejected");
        assert!(
            matches!(result.unwrap_err(), SshError::HostKeyMismatch { .. }),
            "expected HostKeyMismatch"
        );
    }

    /// Non-standard port uses bracket notation in `known_hosts`
    #[test]
    fn check_server_key_handles_non_standard_port() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        let key = parse_key(TEST_KEY_1);

        // Write with bracket notation for port 2222
        std::fs::write(
            &known_hosts,
            format!("[myserver]:2222 {}\n", key.to_string()),
        )
        .unwrap();

        let mut handler = ClientHandler {
            host: "myserver".to_string(),
            port: 2222,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::Verify,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(
            result.is_ok(),
            "should accept key stored with bracket notation for non-standard port"
        );
    }

    /// No `known_hosts` file in Verify mode → `HostKeyNotFound`
    #[test]
    fn check_server_key_missing_known_hosts_in_verify_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("nonexistent_known_hosts");

        let mut handler = ClientHandler {
            host: "host".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::Verify,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SshError::HostKeyNotFound { .. }
        ));
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
    fn host_key_not_found_error_includes_fingerprint() {
        let key = parse_key(TEST_KEY_1);
        let fp = key_fingerprint(&key);
        let err = SshError::HostKeyNotFound {
            host: "myhost".into(),
            fingerprint: fp.clone(),
        };
        let msg = err.to_string();
        assert!(msg.contains("myhost"), "error should contain host");
        assert!(msg.contains(&fp), "error should contain fingerprint");
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

    /// Verify the old `allow_unknown_hosts=true` behaviour still works via `AcceptAny`.
    #[test]
    fn check_server_key_accepts_when_allowed_via_accept_any() {
        let dir = tempfile::TempDir::new().unwrap();
        let known_hosts = dir.path().join("known_hosts");

        let mut handler = ClientHandler {
            host: "testhost".to_string(),
            port: 22,
            known_hosts_path: known_hosts,
            policy: HostKeyPolicy::AcceptAny,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(client::Handler::check_server_key(&mut handler, &key));
        assert!(result.is_ok(), "AcceptAny should accept");
        assert!(result.unwrap(), "should return true when accepting");
    }

    #[test]
    fn known_hosts_host_pattern_port_22() {
        assert_eq!(known_hosts_host_pattern("myserver", 22), "myserver");
    }

    #[test]
    fn known_hosts_host_pattern_nonstandard_port() {
        assert_eq!(
            known_hosts_host_pattern("myserver", 2222),
            "[myserver]:2222"
        );
    }

    #[test]
    fn glob_matches_star() {
        assert!(glob_matches("*.example.com", "host.example.com"));
        assert!(!glob_matches("*.example.com", "example.com"));
    }

    #[test]
    fn glob_matches_question_mark() {
        assert!(glob_matches("host?", "host1"));
        assert!(glob_matches("host?", "hosta"));
        assert!(!glob_matches("host?", "host12"));
    }
}
