//! Hermetic edge-case tests: probe failures, resolution fallbacks, error
//! envelopes, and runner error propagation — no real CLIs, no network.
#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tumult_agent_cli::{
    resolver, run, run_prompt, AdapterRegistry, AgentCliAdapter, AgentCliError, ClaudeCodeAdapter,
    CliInvocation, CliProbe, CodexAdapter, PromptRequest, RawOutput,
};

/// Serializes tests that mutate process-global env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Restores an env var to its previous state on drop.
struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = env::var_os(key);
        env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => env::set_var(self.key, prev),
            None => env::remove_var(self.key),
        }
    }
}

/// Write an executable `#!/bin/sh` script into `dir`.
fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake binary");
    let mut perms = fs::metadata(&path).expect("stat fake binary").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake binary");
    path
}

fn request(prompt: &str, workspace: &Path) -> PromptRequest {
    let mut req = PromptRequest::new(prompt, workspace);
    req.timeout = Duration::from_secs(4);
    req
}

#[test]
fn claude_parse_output_rejects_envelope_without_result() {
    let raw = RawOutput {
        stdout: r#"{"type":"result","subtype":"success","is_error":false}"#.to_string(),
        stderr: String::new(),
        exit_code: Some(0),
    };
    let err = ClaudeCodeAdapter::new()
        .parse_output(&raw)
        .expect_err("envelope without a result string");
    match &err {
        AgentCliError::OutputParse { name, detail } => {
            assert_eq!(name, "claude-code");
            assert!(
                detail.contains("no string `result` field"),
                "detail must explain the missing field: {detail}"
            );
            assert!(
                detail.contains("success"),
                "detail must report the envelope subtype: {detail}"
            );
        }
        other => panic!("expected OutputParse, got: {other:?}"),
    }
}

#[test]
fn claude_parse_output_error_envelope_without_result_text() {
    let raw = RawOutput {
        stdout: r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#
            .to_string(),
        stderr: String::new(),
        exit_code: Some(0),
    };
    let err = ClaudeCodeAdapter::new()
        .parse_output(&raw)
        .expect_err("is_error envelope");
    match &err {
        AgentCliError::InvocationFailed { name, explain } => {
            assert_eq!(name, "claude-code");
            assert!(
                explain.contains("(no result text in error envelope)"),
                "missing result text must be called out: {explain}"
            );
        }
        other => panic!("expected InvocationFailed, got: {other:?}"),
    }
}

#[test]
fn build_invocation_requires_resolvable_binary() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let _claude = EnvGuard::set("CLAUDE_CODE_BIN", dir.path().join("no-claude"));
    let _codex = EnvGuard::set("CODEX_BIN", dir.path().join("no-codex"));
    let _path = EnvGuard::set("PATH", dir.path()); // empty dir: PATH lookup fails

    let err = ClaudeCodeAdapter::new()
        .build_invocation(&request("hi", dir.path()))
        .expect_err("claude binary missing");
    match &err {
        AgentCliError::BinaryNotFound { name, install_hint } => {
            assert_eq!(name, "claude-code");
            assert!(
                install_hint.contains("CLAUDE_CODE_BIN"),
                "hint must mention the env override: {install_hint}"
            );
        }
        other => panic!("expected BinaryNotFound, got: {other:?}"),
    }

    let err = CodexAdapter::new()
        .build_invocation(&request("hi", dir.path()))
        .expect_err("codex binary missing");
    match &err {
        AgentCliError::BinaryNotFound { name, install_hint } => {
            assert_eq!(name, "codex");
            assert!(
                install_hint.contains("npm i -g @openai/codex"),
                "hint must carry the install command: {install_hint}"
            );
        }
        other => panic!("expected BinaryNotFound, got: {other:?}"),
    }
}

#[test]
fn whitespace_only_model_is_dropped() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let claude = fake_bin(dir.path(), "claude", "exit 0");
    let codex = fake_bin(dir.path(), "codex", "exit 0");
    let _claude = EnvGuard::set("CLAUDE_CODE_BIN", &claude);
    let _codex = EnvGuard::set("CODEX_BIN", &codex);

    let mut req = request("hi", dir.path());
    req.model = Some("   ".to_string());

    let invocation = ClaudeCodeAdapter::new()
        .build_invocation(&req)
        .expect("build");
    assert_eq!(
        invocation.argv[1..],
        ["-p", "--output-format", "json"],
        "a blank model override must not add a --model flag"
    );

    let invocation = CodexAdapter::new().build_invocation(&req).expect("build");
    assert!(
        !invocation.argv.iter().any(|a| a == "-m"),
        "a blank model override must not add a -m flag: {:?}",
        invocation.argv
    );
    assert_eq!(invocation.argv.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_detect_reports_missing_binary() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let _bin = EnvGuard::set("CODEX_BIN", dir.path().join("does-not-exist"));
    let _path = EnvGuard::set("PATH", dir.path());

    let probe = CodexAdapter::new().detect();
    assert!(!probe.installed);
    assert!(probe.bin_path.is_none());
    assert!(
        probe.detail.contains("npm i -g @openai/codex"),
        "detail should carry the install hint: {}",
        probe.detail
    );
}

#[test]
fn codex_detect_reports_api_key_auth() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "codex",
        r#"if [ "$1" = "--version" ]; then echo "codex-cli 0.46.0"; exit 0; fi"#,
    );
    let _bin = EnvGuard::set("CODEX_BIN", &bin);
    let _key = EnvGuard::set("OPENAI_API_KEY", "sk-test");

    let probe = CodexAdapter::new().detect();
    assert!(probe.installed);
    assert_eq!(probe.logged_in, Some(true));
    assert!(probe.detail.contains("OPENAI_API_KEY"), "{}", probe.detail);
}

#[test]
fn detect_reports_broken_version_probe() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let body = r#"echo "not a real cli" >&2
exit 1"#;
    let claude = fake_bin(dir.path(), "claude", body);
    let codex = fake_bin(dir.path(), "codex", body);
    let _claude = EnvGuard::set("CLAUDE_CODE_BIN", &claude);
    let _codex = EnvGuard::set("CODEX_BIN", &codex);

    let claude_probe = ClaudeCodeAdapter::new().detect();
    let codex_probe = CodexAdapter::new().detect();
    for (adapter, probe) in [("claude-code", claude_probe), ("codex", codex_probe)] {
        assert!(
            !probe.installed,
            "{adapter} probe must report not installed"
        );
        assert!(
            probe.bin_path.is_none(),
            "{adapter} bin_path must be cleared"
        );
        assert!(
            probe.detail.contains("exit code 1") && probe.detail.contains("not a real cli"),
            "{adapter} detail must describe the probe failure: {}",
            probe.detail
        );
    }
}

#[test]
fn run_prompt_reports_missing_binary() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", dir.path().join("does-not-exist"));
    let _path = EnvGuard::set("PATH", dir.path());

    let adapter = ClaudeCodeAdapter::new();
    let err = run_prompt(&adapter, &request("hi", dir.path())).expect_err("no binary");
    match &err {
        AgentCliError::BinaryNotFound { name, install_hint } => {
            assert_eq!(name, "claude-code");
            assert_eq!(install_hint, adapter.install_hint());
        }
        other => panic!("expected BinaryNotFound, got: {other:?}"),
    }
}

/// Stub adapter whose probe definitively reports a logged-out CLI.
struct LoggedOutStub;

impl AgentCliAdapter for LoggedOutStub {
    fn name(&self) -> &'static str {
        "stub"
    }
    fn binary_env_key(&self) -> &'static str {
        "STUB_BIN"
    }
    fn install_hint(&self) -> &'static str {
        "install the stub"
    }
    fn auth_hint(&self) -> &'static str {
        "Run: stub login"
    }
    fn detect(&self) -> CliProbe {
        CliProbe {
            installed: true,
            version: Some("1.0.0".to_string()),
            logged_in: Some(false),
            bin_path: None,
            detail: "stub reports logged out".to_string(),
        }
    }
    fn build_invocation(&self, _req: &PromptRequest) -> Result<CliInvocation, AgentCliError> {
        unreachable!("run_prompt must fail on the auth check before building")
    }
    fn parse_output(&self, _raw: &RawOutput) -> Result<String, AgentCliError> {
        unreachable!("run_prompt must fail on the auth check before parsing")
    }
    fn explain_failure(&self, _raw: &RawOutput) -> String {
        "stub failure".to_string()
    }
}

#[test]
fn run_prompt_reports_logged_out_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = run_prompt(&LoggedOutStub, &request("hi", dir.path()))
        .expect_err("logged-out probe must fail fast");
    match &err {
        AgentCliError::NotAuthenticated { name, auth_hint } => {
            assert_eq!(name, "stub");
            assert_eq!(auth_hint, "Run: stub login");
        }
        other => panic!("expected NotAuthenticated, got: {other:?}"),
    }
}

#[test]
fn run_rejects_empty_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invocation = CliInvocation {
        argv: Vec::new(),
        stdin: None,
        cwd: dir.path().to_path_buf(),
        env: Vec::new(),
        timeout: Duration::from_secs(1),
    };
    let err = run("stub", &invocation).expect_err("empty argv");
    match &err {
        AgentCliError::InvocationFailed { name, explain } => {
            assert_eq!(name, "stub");
            assert!(explain.contains("invocation argv is empty"), "{explain}");
        }
        other => panic!("expected InvocationFailed, got: {other:?}"),
    }
}

#[test]
fn run_reports_spawn_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invocation = CliInvocation {
        argv: vec![dir
            .path()
            .join("definitely-not-a-binary")
            .display()
            .to_string()],
        stdin: None,
        cwd: dir.path().to_path_buf(),
        env: Vec::new(),
        timeout: Duration::from_secs(1),
    };
    let err = run("stub", &invocation).expect_err("spawn must fail");
    match &err {
        AgentCliError::InvocationFailed { name, explain } => {
            assert_eq!(name, "stub");
            assert!(explain.contains("failed to spawn"), "{explain}");
        }
        other => panic!("expected InvocationFailed, got: {other:?}"),
    }
}

#[test]
fn run_applies_env_overlay_and_closes_stdin_when_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "env-probe",
        r#"printf '%s|%s' "$NO_COLOR" "$EXTRA_VAR""#,
    );
    let invocation = CliInvocation {
        argv: vec![bin.display().to_string()],
        stdin: None,
        cwd: dir.path().to_path_buf(),
        env: vec![
            ("NO_COLOR".to_string(), "0".to_string()),
            ("EXTRA_VAR".to_string(), "present".to_string()),
        ],
        timeout: Duration::from_secs(4),
    };
    let raw = run("stub", &invocation).expect("run succeeds");
    assert_eq!(raw.exit_code, Some(0));
    assert_eq!(
        raw.stdout, "0|present",
        "invocation env must override the NO_COLOR default"
    );
}

#[test]
fn codex_explain_failure_describes_exit() {
    let raw = RawOutput {
        stdout: String::new(),
        stderr: "boom".to_string(),
        exit_code: Some(3),
    };
    let text = CodexAdapter::new().explain_failure(&raw);
    assert!(text.contains("codex exec"), "{text}");
    assert!(text.contains("exit code 3"), "{text}");
    assert!(text.contains("boom"), "{text}");
}

#[test]
fn resolver_ignores_non_executable_override() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    // A plain (non-executable) file as the override must be ignored.
    let stale = dir.path().join("stale-override");
    fs::write(&stale, "not a script").expect("write stale override");
    let real = fake_bin(dir.path(), "mycmd", "exit 0");
    let _override = EnvGuard::set("TEST_OVERRIDE_BIN", &stale);
    let _path = EnvGuard::set("PATH", dir.path());

    let resolved = resolver::resolve_binary("TEST_OVERRIDE_BIN", "mycmd")
        .expect("PATH lookup finds the real binary");
    assert_eq!(
        resolved, real,
        "non-executable override must fall back to PATH"
    );
}

#[test]
fn resolver_ignores_empty_override() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let real = fake_bin(dir.path(), "mycmd", "exit 0");
    let _override = EnvGuard::set("TEST_OVERRIDE_BIN", "");
    let _path = EnvGuard::set("PATH", dir.path());

    let resolved = resolver::resolve_binary("TEST_OVERRIDE_BIN", "mycmd")
        .expect("PATH lookup finds the real binary");
    assert_eq!(resolved, real, "empty override must fall back to PATH");
}

#[test]
fn is_executable_rejects_directories_and_missing_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        !resolver::is_executable(dir.path()),
        "a directory is not an executable file"
    );
    assert!(
        !resolver::is_executable(&dir.path().join("missing")),
        "a nonexistent path is not executable"
    );
}

#[test]
fn registry_default_matches_builtin_and_debug_lists_names() {
    let registry = AdapterRegistry::default();
    assert_eq!(registry.names(), ["claude-code", "codex"]);

    let debug = format!("{registry:?}");
    assert!(debug.contains("claude-code"), "{debug}");
    assert!(debug.contains("codex"), "{debug}");
}
