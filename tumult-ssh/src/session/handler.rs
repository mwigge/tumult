//! russh client handler implementing host-key verification policies.

use std::path::PathBuf;

use russh::client;
use russh::keys::PublicKeyOrCertificate;

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
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Host certificates are never advertised by this client (russh leaves
        // `Preferred::host_key_certificates` empty by default), so a
        // certificate here is unexpected: fail closed rather than trust the
        // key embedded in a certificate whose CA is not verified.
        let PublicKeyOrCertificate::PublicKey {
            key: server_public_key,
            ..
        } = server_public_key
        else {
            tracing::warn!(
                name: "ssh.host_key.certificate_rejected",
                { server.address = %self.host, server.port = self.port },
                "rejecting host certificate, host certificates are not supported"
            );
            return Ok(false);
        };
        match self.policy {
            HostKeyPolicy::AcceptAny => {
                tracing::warn!(
                    name: "ssh.host_key.unverified",
                    { server.address = %self.host, server.port = self.port },
                    "accepting unverified host key, host_key_policy is AcceptAny"
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
    use russh::keys::ssh_key;

    // A known ed25519 test key pair (public only needed here)
    const TEST_KEY_1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test@host";
    // A different key for mismatch testing
    const TEST_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti user@example.com";

    // Host certificate for TEST_KEY_1, signed by a throwaway test CA.
    const TEST_HOST_CERT_1: &str = "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29tAAAAIJE2ArOL4wpGqfMBxuqjUi2Gwj0NvgFqvyEsmtG89Td1AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJlAAAAAAAAAAAAAAACAAAACHRlc3Rob3N0AAAADAAAAAh0ZXN0aG9zdAAAAABeC9LwAAAAAPKlFXAAAAAAAAAAAAAAAAAAAAAzAAAAC3NzaC1lZDI1NTE5AAAAILFI+cSXADdmR//QPS1aMYcTmLClaLlLtuyrSKWZIEARAAAAUwAAAAtzc2gtZWQyNTUxOQAAAEDTEAeNcZCg7vo/i3qHs1ZOUinF1OD0PyK8EHywU5mrZA4TzA8TZFm/WWhA6U9reGi7fctEPNPJJJD5HIAcPQUG test@host";

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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
        assert!(result.is_ok(), "AcceptAny should accept");
        assert!(result.unwrap(), "should return true");
    }

    /// A host certificate is rejected under every policy, even when the key
    /// it embeds is already trusted in `known_hosts`.
    #[test]
    fn check_server_key_rejects_host_certificate_under_every_policy() {
        let cert = ssh_key::Certificate::from_openssh(TEST_HOST_CERT_1).expect("valid test cert");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        for policy in [
            HostKeyPolicy::AcceptAny,
            HostKeyPolicy::Verify,
            HostKeyPolicy::TrustOnFirstUse,
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            let known_hosts = dir.path().join("known_hosts");
            let entry_line = format!("testhost {}\n", parse_key(TEST_KEY_1).to_string());
            std::fs::write(&known_hosts, &entry_line).unwrap();

            let mut handler = ClientHandler {
                host: "testhost".to_string(),
                port: 22,
                known_hosts_path: known_hosts.clone(),
                policy,
            };
            let presented = PublicKeyOrCertificate::from(cert.clone());
            let result = rt.block_on(client::Handler::check_server_key(&mut handler, &presented));
            assert!(
                matches!(result, Ok(false)),
                "host certificate must be rejected"
            );
            assert_eq!(
                std::fs::read_to_string(&known_hosts).unwrap(),
                entry_line,
                "known_hosts must be left untouched"
            );
        }
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &server_key.clone().into(),
        ));
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
        let result = client::Handler::check_server_key(&mut handler, &key.clone().into()).await;
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &server_key.clone().into(),
        ));
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
        assert!(
            matches!(result, Ok(true)),
            "bracket entry for port 2222 must verify, got {result:?}"
        );

        // The same entry must not vouch for the host on any other port.
        for other_port in [22, 2223] {
            handler.port = other_port;
            let result = rt.block_on(client::Handler::check_server_key(
                &mut handler,
                &key.clone().into(),
            ));
            assert!(
                matches!(result, Err(SshError::HostKeyNotFound { .. })),
                "port {other_port} must not match a [myserver]:2222 entry, got {result:?}"
            );
        }
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
        let result = rt.block_on(client::Handler::check_server_key(
            &mut handler,
            &key.clone().into(),
        ));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SshError::HostKeyNotFound { .. }
        ));
    }

    #[test]
    fn accepting_an_unverified_host_key_is_a_named_warning() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut handler = ClientHandler {
            host: "db-1.internal".to_string(),
            port: 2222,
            known_hosts_path: dir.path().join("known_hosts"),
            policy: HostKeyPolicy::AcceptAny,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
            rt.block_on(client::Handler::check_server_key(
                &mut handler,
                &key.clone().into(),
            ))
            .unwrap();
        });

        let event = &logs.named("ssh.host_key.unverified")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["server.address"], "db-1.internal");
        assert_eq!(event["server.port"], 2222);
        assert!(event.get("host").is_none(), "legacy field: {event}");
    }

    #[test]
    fn trusting_a_host_key_on_first_use_is_a_named_event() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut handler = ClientHandler {
            host: "db-1.internal".to_string(),
            port: 22,
            known_hosts_path: dir.path().join("known_hosts"),
            policy: HostKeyPolicy::TrustOnFirstUse,
        };
        let key = parse_key(TEST_KEY_1);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
            rt.block_on(client::Handler::check_server_key(
                &mut handler,
                &key.clone().into(),
            ))
            .unwrap();
        });

        let event = &logs.named("ssh.host_key.trusted_on_first_use")[0];
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["server.address"], "db-1.internal");
        assert_eq!(event["server.port"], 22);
        assert_eq!(event["ssh.host_key.fingerprint"], key_fingerprint(&key));
    }
}
