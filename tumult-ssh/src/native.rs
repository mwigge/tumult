//! Native dispatch — [`NativeExecutor`] implementation for `tumult-ssh`.
//!
//! Exposes remote command execution to the experiment runner. The host key
//! policy is read from the `host_key_policy` activity argument and defaults
//! to this crate's secure default, [`HostKeyPolicy::Verify`]; connecting to
//! ephemeral targets with unverifiable keys requires an explicit
//! `host_key_policy: accept-any` opt-in.

use std::path::PathBuf;
use std::time::Duration;

use tumult_plugin::native::{arg_num, arg_str, NativeArgs, NativeError, NativeExecutor};

use crate::config::{AuthMethod, HostKeyPolicy, SshConfig};
use crate::session::SshSession;

/// Functions `tumult-ssh` provides to the experiment runner.
const FUNCTIONS: &[&str] = &["execute"];

/// [`NativeExecutor`] for the `tumult-ssh` plugin.
pub struct SshExecutor;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for SshExecutor {
    fn name(&self) -> &'static str {
        "tumult-ssh"
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        match function {
            "execute" => execute_command(args).await,
            _ => Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            )),
        }
    }
}

/// Parse the optional `host_key_policy` argument.
///
/// Accepted values are `verify`, `trust-on-first-use`, and `accept-any`;
/// an absent argument yields the crate's secure default
/// ([`HostKeyPolicy::Verify`]).
fn parse_host_key_policy(args: &NativeArgs) -> Result<HostKeyPolicy, NativeError> {
    let Some(value) = args.get("host_key_policy") else {
        return Ok(HostKeyPolicy::default());
    };
    let value = value
        .as_str()
        .ok_or_else(|| NativeError::invalid_argument("host_key_policy", "expected a string"))?;
    match value {
        "verify" => Ok(HostKeyPolicy::Verify),
        "trust-on-first-use" => Ok(HostKeyPolicy::TrustOnFirstUse),
        "accept-any" => Ok(HostKeyPolicy::AcceptAny),
        other => Err(NativeError::invalid_argument(
            "host_key_policy",
            format!(
                "unknown policy `{other}` (expected `verify`, `trust-on-first-use`, or `accept-any`)"
            ),
        )),
    }
}

/// Build an [`SshConfig`] from activity arguments.
///
/// Requires `host` and `user`; `port` defaults to 22. Authentication uses
/// the key at `key_file` when given, otherwise the SSH agent.
fn build_config(args: &NativeArgs) -> Result<SshConfig, NativeError> {
    let host = arg_str(args, "host")?;
    let port = arg_num::<u16>(args, "port").unwrap_or(22);
    let user = arg_str(args, "user")?;

    let key_path = args
        .get("key_file")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);

    let auth = if let Some(key_path) = key_path {
        AuthMethod::Key {
            key_path,
            passphrase: None,
        }
    } else {
        AuthMethod::Agent
    };

    Ok(SshConfig {
        host: host.to_string(),
        port,
        user: user.to_string(),
        auth,
        host_key_policy: parse_host_key_policy(args)?,
        connect_timeout: Duration::from_secs(30),
        command_timeout: Some(Duration::from_mins(1)),
        known_hosts_path: None,
    })
}

/// Run `command` on the remote host described by `args`.
async fn execute_command(args: &NativeArgs) -> Result<String, NativeError> {
    let command = arg_str(args, "command")?;
    let config = build_config(args)?;

    let session = SshSession::connect(config)
        .await
        .map_err(|e| NativeError::execution_context("SSH connect failed", e))?;

    let result = session
        .execute(command)
        .await
        .map_err(|e| NativeError::execution_context("SSH execute failed", e))?;

    let _ = session.close().await;

    if result.exit_code == 0 {
        Ok(result.stdout)
    } else {
        Err(NativeError::Failed(format!(
            "SSH command exited {}: {}",
            result.exit_code, result.stderr
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> NativeArgs {
        NativeArgs::from([
            ("host".into(), serde_json::json!("db-primary")),
            ("user".into(), serde_json::json!("ops")),
            ("command".into(), serde_json::json!("uname -a")),
        ])
    }

    #[test]
    fn metadata_names_plugin_and_functions() {
        let executor = SshExecutor;
        assert_eq!(executor.name(), "tumult-ssh");
        assert_eq!(executor.functions(), &["execute"]);
    }

    #[tokio::test]
    async fn unknown_function_is_rejected() {
        let err = SshExecutor
            .execute("upload", &base_args())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
        assert!(err.to_string().contains("execute"));
    }

    #[test]
    fn host_key_policy_defaults_to_verify() {
        let config = build_config(&base_args()).unwrap();
        assert_eq!(
            config.host_key_policy,
            HostKeyPolicy::Verify,
            "absent host_key_policy must fall back to the secure default"
        );
    }

    #[test]
    fn host_key_policy_accept_any_is_explicit_opt_in() {
        let mut args = base_args();
        args.insert("host_key_policy".into(), serde_json::json!("accept-any"));
        let config = build_config(&args).unwrap();
        assert_eq!(config.host_key_policy, HostKeyPolicy::AcceptAny);
    }

    #[test]
    fn host_key_policy_parses_all_named_policies() {
        let mut args = base_args();
        args.insert("host_key_policy".into(), serde_json::json!("verify"));
        assert_eq!(
            build_config(&args).unwrap().host_key_policy,
            HostKeyPolicy::Verify
        );
        args.insert(
            "host_key_policy".into(),
            serde_json::json!("trust-on-first-use"),
        );
        assert_eq!(
            build_config(&args).unwrap().host_key_policy,
            HostKeyPolicy::TrustOnFirstUse
        );
    }

    #[test]
    fn host_key_policy_rejects_unknown_value() {
        let mut args = base_args();
        args.insert("host_key_policy".into(), serde_json::json!("yolo"));
        let err = build_config(&args).unwrap_err();
        assert!(
            matches!(err, NativeError::InvalidArgument { .. }),
            "expected InvalidArgument, got: {err:?}"
        );
        assert!(err.to_string().contains("yolo"));
    }

    #[test]
    fn host_key_policy_rejects_non_string_value() {
        let mut args = base_args();
        args.insert("host_key_policy".into(), serde_json::json!(true));
        assert!(build_config(&args).is_err());
    }

    #[test]
    fn config_defaults_port_and_agent_auth() {
        let config = build_config(&base_args()).unwrap();
        assert_eq!(config.host, "db-primary");
        assert_eq!(config.port, 22);
        assert_eq!(config.user, "ops");
        assert!(matches!(config.auth, AuthMethod::Agent));
    }

    #[test]
    fn key_file_selects_key_auth() {
        let mut args = base_args();
        args.insert("port".into(), serde_json::json!(2222));
        args.insert(
            "key_file".into(),
            serde_json::json!("/home/ops/.ssh/id_ed25519"),
        );
        let config = build_config(&args).unwrap();
        assert_eq!(config.port, 2222);
        match config.auth {
            AuthMethod::Key {
                key_path,
                passphrase,
            } => {
                assert_eq!(key_path, PathBuf::from("/home/ops/.ssh/id_ed25519"));
                assert!(passphrase.is_none());
            }
            AuthMethod::Agent => panic!("expected key auth"),
        }
    }

    #[test]
    fn missing_host_is_typed_error() {
        let mut args = base_args();
        args.remove("host");
        let err = build_config(&args).unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
    }
}
