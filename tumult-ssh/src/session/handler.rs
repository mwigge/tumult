//! russh client handler implementing host-key verification policies.

use std::path::PathBuf;

use russh::client;
use russh::keys::ssh_key;

use crate::config::HostKeyPolicy;
use crate::error::SshError;

use super::known_hosts::{trust_on_first_use, verify_host_key};

/// Client handler for russh, implementing host key verification.
///
/// Supports three policies:
/// - [`HostKeyPolicy::Verify`]: checks against `known_hosts`, rejects unknown/mismatched keys
/// - [`HostKeyPolicy::TrustOnFirstUse`]: accepts and records first-seen keys; verifies thereafter
/// - [`HostKeyPolicy::AcceptAny`]: bypasses verification (insecure — only for ephemeral infra)
pub(super) struct ClientHandler {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) known_hosts_path: PathBuf,
    pub(super) policy: HostKeyPolicy,
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

#[cfg(test)]
mod tests {
    use super::super::known_hosts::key_fingerprint;
    use super::*;

    // A known ed25519 test key pair (public only needed here)
    const TEST_KEY_1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test@host";
    // A different key for mismatch testing
    const TEST_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti user@example.com";

    fn parse_key(s: &str) -> ssh_key::PublicKey {
        ssh_key::PublicKey::from_openssh(s).expect("valid test key")
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
}
