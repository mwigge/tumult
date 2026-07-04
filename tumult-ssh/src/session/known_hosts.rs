//! `known_hosts` parsing, host-pattern matching, and key verification.

use std::path::{Path, PathBuf};

use russh::keys::ssh_key;
use russh::keys::ssh_key::known_hosts::KnownHosts;
use russh::keys::ssh_key::HashAlg;

use crate::error::SshError;

/// Resolve the `known_hosts` file path: use provided path or fall back to `~/.ssh/known_hosts`.
pub(super) fn resolve_known_hosts_path(override_path: Option<&Path>) -> PathBuf {
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
pub(super) fn key_fingerprint(key: &ssh_key::PublicKey) -> String {
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

/// Verify a server key against the `known_hosts` file (strict verification).
///
/// Returns `Ok(true)` if a matching entry is found and the key matches.
/// Returns `Err(SshError::HostKeyNotFound)` if no entry exists for the host.
/// Returns `Err(SshError::HostKeyMismatch)` if an entry exists but the key differs.
pub(super) async fn verify_host_key(
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
pub(super) async fn trust_on_first_use(
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

    // Flush before returning: `tokio::fs::File` buffers internally and does NOT
    // flush on drop, so without this the just-appended entry can still be in
    // buffer when a subsequent read (a later verify in the same process, or a
    // test reading the file back) runs — a race that widens under load.
    file.flush().await.map_err(|e| SshError::KnownHostsIo {
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

    fn parse_key(s: &str) -> ssh_key::PublicKey {
        ssh_key::PublicKey::from_openssh(s).expect("valid test key")
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
