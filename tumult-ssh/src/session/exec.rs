//! Command execution and file upload over an established SSH session.

use std::path::Path;

use crate::error::SshError;

use super::{command_preview, shell_escape, CommandResult, SshSession};

impl SshSession {
    /// Execute a command on the remote host.
    ///
    /// # Errors
    ///
    /// Returns [`SshError::ChannelError`] if a channel cannot be opened.
    /// Returns [`SshError::ExecutionFailed`] if the command cannot be sent.
    /// Returns [`SshError::Timeout`] if the whole exchange exceeds the
    /// configured timeout — a *total* deadline, so a command that streams
    /// output forever still times out.
    #[tracing::instrument(skip(self), fields(command_preview = command_preview(command)))]
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

        // Read the channel to completion. The timeout wraps the WHOLE loop as
        // one total deadline; a per-message timeout would reset on every
        // received chunk and never fire for a command streaming output forever.
        let read_loop = async {
            loop {
                match channel.wait().await {
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
            (stdout, stderr, exit_code, exit_signal)
        };

        let (stdout, stderr, exit_code, exit_signal) =
            if let Some(timeout) = self.config.command_timeout {
                tokio::time::timeout(timeout, read_loop)
                    .await
                    .map_err(|_| SshError::Timeout {
                        seconds: timeout.as_secs_f64(),
                    })?
            } else {
                read_loop.await
            };

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
        let file_size = tokio::fs::metadata(local_path).await.map_or(0, |m| m.len());
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
}
