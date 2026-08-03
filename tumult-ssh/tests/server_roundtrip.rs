//! Execution-path tests against an in-process russh test server.
//!
//! The server binds an ephemeral loopback port per test — no Docker, no
//! external sshd, no shared state between tests. Command behavior is
//! scripted by exact command string, so stdout/stderr/exit-status mapping
//! is asserted end to end through the real SSH protocol.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key;
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId};
use tokio::net::TcpListener;

use tumult_plugin::native::{NativeArgs, NativeError, NativeExecutor};
use tumult_ssh::{HostKeyPolicy, SshConfig, SshError, SshExecutor, SshPool, SshSession};

// Throwaway ed25519 keys generated for this test suite only.
const CLIENT_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCJRBvO3nZiCl90+1UcTLAAr/VXW9cq8mMJCQdND9MIXwAAAJhGBYBqRgWA
agAAAAtzc2gtZWQyNTUxOQAAACCJRBvO3nZiCl90+1UcTLAAr/VXW9cq8mMJCQdND9MIXw
AAAEAYABayWUtvHW8urprwIunsrzHwVY+vZnEf0sca5SDt6IlEG87edmIKX3T7VRxMsACv
9Vdb1yryYwkJB00P0whfAAAAEnR1bXVsdC10ZXN0LWNsaWVudAECAw==
-----END OPENSSH PRIVATE KEY-----
";

const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBfVCuqvZencLa0RiSSBUK8Fte3ZeWNvNA665Tf+cXUqgAAAJj0e4Ic9HuC
HAAAAAtzc2gtZWQyNTUxOQAAACBfVCuqvZencLa0RiSSBUK8Fte3ZeWNvNA665Tf+cXUqg
AAAEAxvXRBUnx7Jcin5D2271AJtMlLOev8LVHrd47wJIIPR19UK6q9l6dwtrRGJJIFQrwW
17dl5Y280DrrlN/5xdSqAAAAEHR1bXVsdC10ZXN0LWhvc3QBAgMEBQ==
-----END OPENSSH PRIVATE KEY-----
";

/// Server-side handler with scripted command behavior.
///
/// `uploads` records the bytes the client streams after a `cat > '<path>'`
/// exec, keyed by the remote path — the remote-side effect upload tests
/// assert against. `in_flight` buffers per-channel bytes until EOF.
#[derive(Clone)]
struct TestHandler {
    reject_auth: bool,
    uploads: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    in_flight: HashMap<ChannelId, (String, Vec<u8>)>,
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.reject_auth {
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        } else {
            Ok(Auth::Accept)
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        session.channel_success(channel)?;
        match command.as_str() {
            "true" => {
                session.exit_status_request(channel, 0)?;
            }
            "greet" => {
                session.data(channel, &b"hello from tumult\n"[..])?;
                session.exit_status_request(channel, 0)?;
            }
            "fail" => {
                session.extended_data(channel, 1, &b"boom\n"[..])?;
                session.exit_status_request(channel, 3)?;
            }
            "no-status" => {
                // Close without ever reporting an exit status.
                session.data(channel, &b"partial output\n"[..])?;
            }
            "hang" => {
                // Keep the channel open forever: no output, no status, no close.
                return Ok(());
            }
            "die" => {
                // Tear the whole connection down mid-command.
                return Err(russh::Error::Disconnect);
            }
            "sigkill" => {
                session.extended_data(channel, 1, &b"about to die\n"[..])?;
                session.exit_signal_request(channel, russh::Sig::KILL, false, "", "")?;
            }
            "sigterm-quiet" => {
                // Signal death with no preceding stderr output.
                session.exit_signal_request(channel, russh::Sig::TERM, false, "", "")?;
            }
            "sigsegv-core" => {
                session.exit_signal_request(channel, russh::Sig::SEGV, true, "", "")?;
            }
            "signal-and-status" => {
                // Both a signal and an explicit status: the status must win.
                session.exit_signal_request(channel, russh::Sig::INT, false, "", "")?;
                session.exit_status_request(channel, 42)?;
            }
            cmd if cmd.starts_with("cat > '") => {
                // Upload path: buffer the streamed bytes until the client
                // sends EOF, then report the exit status from `channel_eof`.
                let path = cmd
                    .strip_prefix("cat > '")
                    .and_then(|rest| rest.split('\'').next())
                    .expect("quoted remote path")
                    .to_string();
                self.in_flight.insert(channel, (path, Vec::new()));
                return Ok(());
            }
            _ => {
                session.extended_data(channel, 1, &b"command not found\n"[..])?;
                session.exit_status_request(channel, 127)?;
            }
        }
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some((_, buf)) = self.in_flight.get_mut(&channel) {
            buf.extend_from_slice(data);
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some((path, buf)) = self.in_flight.remove(&channel) {
            if path.starts_with("/missing/") {
                let msg = format!("cat: {path}: No such file or directory\n");
                session.extended_data(channel, 1, msg.into_bytes())?;
                session.exit_status_request(channel, 1)?;
            } else {
                self.uploads.lock().expect("uploads lock").insert(path, buf);
                session.exit_status_request(channel, 0)?;
            }
            session.close(channel)?;
        }
        Ok(())
    }
}

/// Spawn a test SSH server on an ephemeral loopback port.
///
/// Returns the bound port and a counter of accepted TCP connections.
async fn spawn_server(reject_auth: bool) -> (u16, Arc<AtomicUsize>) {
    let (port, connections, _) = spawn_server_with_uploads(reject_auth).await;
    (port, connections)
}

/// Like [`spawn_server`], but also returns the map of uploaded remote files
/// the server records for `cat > '<path>'` commands.
async fn spawn_server_with_uploads(
    reject_auth: bool,
) -> (u16, Arc<AtomicUsize>, Arc<Mutex<HashMap<String, Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let connections = Arc::new(AtomicUsize::new(0));
    let uploads = Arc::new(Mutex::new(HashMap::new()));

    let host_key = russh::keys::decode_secret_key(HOST_KEY, None).expect("parse host key");
    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        ..Default::default()
    });

    let counter = Arc::clone(&connections);
    let uploaded = Arc::clone(&uploads);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let config = Arc::clone(&config);
            let handler = TestHandler {
                reject_auth,
                uploads: Arc::clone(&uploaded),
                in_flight: HashMap::new(),
            };
            tokio::spawn(async move {
                if let Ok(session) = server::run_stream(config, stream, handler).await {
                    let _ = session.await;
                }
            });
        }
    });

    (port, connections, uploads)
}

/// Write the embedded client key into `dir` with 0600 permissions.
fn write_client_key(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("id_ed25519");
    std::fs::write(&path, CLIENT_KEY).expect("write key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod key");
    }
    path
}

/// `SshSession` is not `Debug`, so `Result::expect_err` cannot be used on
/// connect results; unwrap the error by hand.
async fn connect_err(config: SshConfig) -> SshError {
    match SshSession::connect(config).await {
        Ok(_) => panic!("expected connection to fail"),
        Err(err) => err,
    }
}

fn config_for(port: u16, key_path: PathBuf) -> SshConfig {
    SshConfig::with_key("127.0.0.1", "chaos", key_path)
        .port(port)
        .connect_timeout(Duration::from_secs(2))
        .command_timeout(Duration::from_secs(2))
        .host_key_policy(HostKeyPolicy::AcceptAny)
}

// ── Execute round trip ────────────────────────────────────────

#[tokio::test]
async fn execute_captures_stdout_and_zero_exit() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("greet").await.expect("execute");

    assert_eq!(result.exit_code, 0);
    assert!(result.success());
    assert_eq!(result.stdout, "hello from tumult");
    assert_eq!(result.stderr, "");
    session.close().await.expect("close");
}

#[tokio::test]
async fn execute_maps_nonzero_exit_and_stderr() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("fail").await.expect("execute");

    assert_eq!(result.exit_code, 3);
    assert!(!result.success());
    assert_eq!(result.stderr, "boom");
    assert_eq!(result.stdout, "");
}

#[tokio::test]
async fn execute_defaults_to_exit_1_when_server_reports_no_status() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("no-status").await.expect("execute");

    assert_eq!(
        result.exit_code, 1,
        "missing exit status must map to failure, not success"
    );
    assert_eq!(result.stdout, "partial output");
}

#[tokio::test]
async fn execute_times_out_when_command_never_finishes() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let config =
        config_for(port, write_client_key(&dir)).command_timeout(Duration::from_millis(300));
    let session = SshSession::connect(config).await.expect("connect");

    let err = session.execute("hang").await.expect_err("must time out");

    match err {
        SshError::Timeout { seconds } => {
            assert!(
                (seconds - 0.3).abs() < 1e-9,
                "the error must report the configured deadline, got {seconds}"
            );
        }
        other => panic!("expected Timeout, got: {other:?}"),
    }
}

// ── Signal termination and exit-status precedence ─────────────

#[tokio::test]
async fn execute_maps_exit_signal_to_137_and_appends_note_to_stderr() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("sigkill").await.expect("execute");

    assert_eq!(
        result.exit_code, 137,
        "a signal death without exit status must map to 128 + 9"
    );
    assert!(!result.success());
    assert_eq!(result.stderr, "about to die\nkilled by signal: KILL");
}

#[tokio::test]
async fn execute_signal_note_alone_when_stderr_is_empty() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("sigterm-quiet").await.expect("execute");

    assert_eq!(result.exit_code, 137);
    assert_eq!(result.stderr, "killed by signal: TERM");
    assert_eq!(result.stdout, "");
}

#[tokio::test]
async fn execute_marks_core_dumped_signal_deaths() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("sigsegv-core").await.expect("execute");

    assert_eq!(result.exit_code, 137);
    assert_eq!(result.stderr, "killed by signal: SEGV (core dumped)");
}

#[tokio::test]
async fn execute_prefers_explicit_exit_status_over_signal() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let result = session.execute("signal-and-status").await.expect("execute");

    assert_eq!(
        result.exit_code, 42,
        "an explicit exit status takes precedence over the signal fallback"
    );
    assert_eq!(result.stderr, "killed by signal: INT");
}

// ── Connection and auth failure paths ─────────────────────────

#[tokio::test]
async fn auth_rejection_maps_to_authentication_failed() {
    let (port, _) = spawn_server(true).await;
    let dir = tempfile::TempDir::new().unwrap();

    let err = connect_err(config_for(port, write_client_key(&dir))).await;

    match err {
        SshError::AuthenticationFailed { host, user, reason } => {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(user, "chaos");
            assert!(reason.contains("rejected"), "unexpected reason: {reason}");
        }
        other => panic!("expected AuthenticationFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn connect_times_out_against_silent_listener() {
    // A TCP listener that accepts but never speaks SSH.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let silent = tokio::spawn(async move {
        let _socket = listener.accept().await;
        std::future::pending::<()>().await;
    });

    let dir = tempfile::TempDir::new().unwrap();
    let config =
        config_for(port, write_client_key(&dir)).connect_timeout(Duration::from_millis(300));

    let err = connect_err(config).await;

    assert!(
        matches!(err, SshError::Timeout { .. }),
        "expected Timeout, got: {err:?}"
    );
    silent.abort();
}

#[tokio::test]
async fn connect_to_closed_port_maps_to_connection_failed() {
    // Bind then immediately drop to find a port that refuses connections.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let dir = tempfile::TempDir::new().unwrap();
    let err = connect_err(config_for(port, write_client_key(&dir))).await;

    match err {
        SshError::ConnectionFailed { host, port: p, .. } => {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(p, port);
        }
        other => panic!("expected ConnectionFailed, got: {other:?}"),
    }
}

// ── Host key policies against a real server key ───────────────

#[tokio::test]
async fn trust_on_first_use_records_key_then_verify_accepts_it() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let key_path = write_client_key(&dir);
    let known_hosts = dir.path().join("known_hosts");

    // First connection: TOFU records the server key.
    let config = config_for(port, key_path.clone())
        .host_key_policy(HostKeyPolicy::TrustOnFirstUse)
        .known_hosts_path(known_hosts.clone());
    let session = SshSession::connect(config).await.expect("TOFU connect");
    session.close().await.expect("close");

    let contents = std::fs::read_to_string(&known_hosts).expect("known_hosts written");
    assert!(
        contents.contains(&format!("[127.0.0.1]:{port}")),
        "non-standard port must use bracket notation: {contents}"
    );

    // Second connection: strict Verify now accepts the recorded key.
    let config = config_for(port, key_path)
        .host_key_policy(HostKeyPolicy::Verify)
        .known_hosts_path(known_hosts);
    let session = SshSession::connect(config).await.expect("Verify connect");
    let result = session.execute("greet").await.expect("execute");
    assert_eq!(result.stdout, "hello from tumult");
}

#[tokio::test]
async fn verify_policy_rejects_unknown_server_key() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    std::fs::write(&known_hosts, "").unwrap();

    let config = config_for(port, write_client_key(&dir))
        .host_key_policy(HostKeyPolicy::Verify)
        .known_hosts_path(known_hosts);

    let err = connect_err(config).await;

    // `SshSession::connect` passes the handler's typed host-key rejection
    // through untouched, as documented.
    assert!(
        matches!(err, SshError::HostKeyNotFound { .. }),
        "expected HostKeyNotFound, got: {err:?}"
    );
}

// ── Session pool behavior ─────────────────────────────────────

#[tokio::test]
async fn pool_reuses_live_session_across_executes() {
    let (port, connections) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let config = config_for(port, write_client_key(&dir));
    let pool = SshPool::new();

    let first = pool.execute(&config, "greet").await.expect("first execute");
    let second = pool.execute(&config, "fail").await.expect("second execute");

    assert_eq!(first.stdout, "hello from tumult");
    assert_eq!(second.exit_code, 3);
    assert_eq!(pool.len(), 1, "one cached session per endpoint");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "second execute must reuse the pooled connection"
    );
}

#[tokio::test]
async fn pool_reconnects_when_cached_session_is_dead() {
    let (port, connections) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let config = config_for(port, write_client_key(&dir));
    let pool = SshPool::new();

    // Populate the pool, then have the server kill the connection mid-command.
    pool.execute(&config, "greet").await.expect("first execute");
    let _ = pool.execute(&config, "die").await;
    assert_eq!(connections.load(Ordering::SeqCst), 1);

    // The stale-probe must detect the dead session and reconnect.
    let result = pool
        .execute(&config, "greet")
        .await
        .expect("execute after reconnect");

    assert_eq!(result.stdout, "hello from tumult");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        2,
        "a fresh connection must be established after the server dropped the session"
    );
    assert_eq!(pool.len(), 1);
}

#[tokio::test]
async fn pool_connect_failure_leaves_pool_empty() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let dir = tempfile::TempDir::new().unwrap();
    let config = config_for(port, write_client_key(&dir));
    let pool = SshPool::new();

    let err = pool
        .execute(&config, "greet")
        .await
        .expect_err("connect must fail");

    assert!(matches!(err, SshError::ConnectionFailed { .. }));
    assert!(pool.is_empty(), "failed connections must not be cached");
}

// ── Native executor dispatch ──────────────────────────────────

fn executor_args(port: u16, key_path: &std::path::Path, command: &str) -> NativeArgs {
    NativeArgs::from([
        ("host".into(), serde_json::json!("127.0.0.1")),
        ("port".into(), serde_json::json!(port)),
        ("user".into(), serde_json::json!("chaos")),
        (
            "key_file".into(),
            serde_json::json!(key_path.display().to_string()),
        ),
        ("host_key_policy".into(), serde_json::json!("accept-any")),
        ("command".into(), serde_json::json!(command)),
    ])
}

#[tokio::test]
async fn native_execute_returns_stdout_on_success() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let key_path = write_client_key(&dir);

    let output = SshExecutor
        .execute("execute", &executor_args(port, &key_path, "greet"))
        .await
        .expect("native execute");

    assert_eq!(output, "hello from tumult");
}

#[tokio::test]
async fn native_execute_maps_nonzero_exit_to_failed_error() {
    let (port, _) = spawn_server(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let key_path = write_client_key(&dir);

    let err = SshExecutor
        .execute("execute", &executor_args(port, &key_path, "fail"))
        .await
        .expect_err("non-zero exit must be an error");

    assert!(
        matches!(err, NativeError::Failed(_)),
        "expected Failed, got: {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains('3'),
        "exit code must be reported: {message}"
    );
    assert!(
        message.contains("boom"),
        "stderr must be reported: {message}"
    );
}

// ── File upload ───────────────────────────────────────────────

#[tokio::test]
async fn upload_fails_when_local_file_is_missing() {
    let (port, _, _) = spawn_server_with_uploads(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let missing = dir.path().join("no-such-script.sh");
    let err = session
        .upload_file(&missing, "/remote/x.sh")
        .await
        .expect_err("missing local file must fail");

    match err {
        SshError::UploadFailed(reason) => {
            assert!(
                reason.contains("read local file"),
                "expected the local-read context, got: {reason}"
            );
        }
        other => panic!("expected UploadFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn upload_streams_bytes_to_the_remote_path() {
    let (port, _, uploads) = spawn_server_with_uploads(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let local = dir.path().join("payload.sh");
    std::fs::write(&local, b"#!/bin/sh\necho chaos\n").unwrap();

    session
        .upload_file(&local, "/remote/payload.sh")
        .await
        .expect("upload");

    let uploads = uploads.lock().expect("uploads lock");
    assert_eq!(
        uploads.get("/remote/payload.sh").map(Vec::as_slice),
        Some(b"#!/bin/sh\necho chaos\n".as_slice()),
        "the server must record the exact streamed bytes"
    );
}

#[tokio::test]
async fn upload_fails_when_remote_write_exits_nonzero() {
    let (port, _, uploads) = spawn_server_with_uploads(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let local = dir.path().join("payload.sh");
    std::fs::write(&local, b"echo hi\n").unwrap();

    let err = session
        .upload_file(&local, "/missing/dir/payload.sh")
        .await
        .expect_err("nonexistent remote directory must fail");

    match err {
        SshError::UploadFailed(reason) => {
            assert!(
                reason.contains("non-zero status"),
                "expected the remote exit-status context, got: {reason}"
            );
        }
        other => panic!("expected UploadFailed, got: {other:?}"),
    }
    assert!(
        uploads.lock().expect("uploads lock").is_empty(),
        "a failed write must not be recorded as an upload"
    );
}

#[tokio::test]
async fn upload_rejects_remote_path_with_control_characters() {
    let (port, _, _) = spawn_server_with_uploads(false).await;
    let dir = tempfile::TempDir::new().unwrap();
    let session = SshSession::connect(config_for(port, write_client_key(&dir)))
        .await
        .expect("connect");

    let local = dir.path().join("payload.sh");
    std::fs::write(&local, b"echo hi\n").unwrap();

    let err = session
        .upload_file(&local, "/tmp/evil\nrm -rf /")
        .await
        .expect_err("control characters in the remote path must fail");

    assert!(
        matches!(err, SshError::InvalidPath { .. }),
        "expected InvalidPath, got: {err:?}"
    );
}
