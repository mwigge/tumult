//! SSH authentication (public-key and agent-based).

use std::sync::Arc;

use russh::client;

use crate::config::{AuthMethod, SshConfig};
use crate::error::SshError;

use super::handler::ClientHandler;

/// Authenticate using the configured method.
pub(super) async fn authenticate(
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
