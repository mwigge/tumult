//! Hermetic integration tests driving the adapters against fake `#!/bin/sh`
//! binaries — no real `claude` / `codex`, no network.
#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tumult_agent_cli::{
    run_prompt, AdapterRegistry, AgentCliAdapter, AgentCliError, ClaudeCodeAdapter, CodexAdapter,
    PromptRequest, RawOutput,
};

/// Realistic `claude -p --output-format json` result envelope (documented
/// shape of Claude Code's print-mode JSON output).
const CLAUDE_JSON_ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2734,"duration_api_ms":2450,"num_turns":1,"result":"Fake claude answer.","session_id":"5b1e9a2f-0000-4000-8000-000000000000","total_cost_usd":0.0421,"usage":{"input_tokens":12,"output_tokens":9}}"#;

const CLAUDE_JSON_ERROR_ENVELOPE: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":812,"num_turns":1,"result":"Credit balance is too low","session_id":"5b1e9a2f-0000-4000-8000-000000000001"}"#;

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

    fn unset(key: &'static str) -> Self {
        let prev = env::var_os(key);
        env::remove_var(key);
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
fn claude_detect_reports_missing_binary() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", dir.path().join("does-not-exist"));
    let _path = EnvGuard::set("PATH", dir.path()); // empty dir: PATH lookup fails

    let probe = ClaudeCodeAdapter::new().detect();
    assert!(!probe.installed);
    assert!(probe.bin_path.is_none());
    assert!(probe.version.is_none());
    assert!(
        probe.detail.contains("npm i -g @anthropic-ai/claude-code"),
        "detail should carry the install hint: {}",
        probe.detail
    );
}

#[test]
fn claude_detect_parses_version() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi"#,
    );
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);
    let _key = EnvGuard::unset("ANTHROPIC_API_KEY");

    let probe = ClaudeCodeAdapter::new().detect();
    assert!(probe.installed);
    assert_eq!(probe.version.as_deref(), Some("2.0.13"));
    assert_eq!(probe.bin_path.as_deref(), Some(bin.as_path()));
    assert_eq!(
        probe.logged_in, None,
        "auth must stay unclear without a cheap signal"
    );
}

#[test]
fn claude_detect_reports_api_key_auth() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi"#,
    );
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);
    let _key = EnvGuard::set("ANTHROPIC_API_KEY", "sk-ant-test");

    let probe = ClaudeCodeAdapter::new().detect();
    assert_eq!(probe.logged_in, Some(true));
    assert!(
        probe.detail.contains("ANTHROPIC_API_KEY"),
        "{}",
        probe.detail
    );
}

#[test]
fn claude_run_prompt_round_trip() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture = dir.path().join("envelope.json");
    fs::write(&fixture, CLAUDE_JSON_ENVELOPE).expect("write fixture");
    let prompt_file = dir.path().join("prompt.txt");
    let color_file = dir.path().join("no_color.txt");
    let body = format!(
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi
cat > "{prompt}"
printf '%s' "$NO_COLOR" > "{color}"
cat "{fixture}""#,
        prompt = prompt_file.display(),
        color = color_file.display(),
        fixture = fixture.display(),
    );
    let bin = fake_bin(dir.path(), "claude", &body);
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);

    let answer = run_prompt(
        &ClaudeCodeAdapter::new(),
        &request("What is chaos engineering?", dir.path()),
    )
    .expect("round trip succeeds");

    assert_eq!(answer, "Fake claude answer.");
    let piped = fs::read_to_string(&prompt_file).expect("prompt captured");
    assert_eq!(
        piped, "What is chaos engineering?",
        "prompt must be piped via stdin"
    );
    let no_color = fs::read_to_string(&color_file).expect("NO_COLOR captured");
    assert_eq!(no_color, "1", "runner must always set NO_COLOR=1");
}

#[test]
fn claude_build_invocation_model_flag() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(dir.path(), "claude", "exit 0");
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);
    let adapter = ClaudeCodeAdapter::new();

    let mut req = request("hi", dir.path());
    let invocation = adapter.build_invocation(&req).expect("build");
    assert_eq!(
        invocation.argv[1..],
        ["-p", "--output-format", "json"],
        "no --model flag without an override"
    );
    assert_eq!(invocation.stdin.as_deref(), Some("hi"));
    assert_eq!(invocation.cwd, dir.path());

    req.model = Some("claude-sonnet-4-5".to_string());
    let invocation = adapter.build_invocation(&req).expect("build with model");
    assert_eq!(
        invocation.argv[1..],
        [
            "-p",
            "--output-format",
            "json",
            "--model",
            "claude-sonnet-4-5"
        ]
    );
}

#[test]
fn claude_parse_output_rejects_non_json() {
    let raw = RawOutput {
        stdout: "plain text, definitely not JSON".to_string(),
        stderr: String::new(),
        exit_code: Some(0),
    };
    let err = ClaudeCodeAdapter::new()
        .parse_output(&raw)
        .expect_err("strict JSON");
    match &err {
        AgentCliError::OutputParse { name, detail } => {
            assert_eq!(name, "claude-code");
            assert!(
                detail.contains("plain text"),
                "detail must include a stdout snippet: {detail}"
            );
        }
        other => panic!("expected OutputParse, got: {other:?}"),
    }
}

#[test]
fn claude_parse_output_surfaces_error_envelope() {
    let raw = RawOutput {
        stdout: CLAUDE_JSON_ERROR_ENVELOPE.to_string(),
        stderr: String::new(),
        exit_code: Some(0),
    };
    let err = ClaudeCodeAdapter::new()
        .parse_output(&raw)
        .expect_err("is_error envelope");
    assert!(
        matches!(&err, AgentCliError::InvocationFailed { .. }),
        "expected InvocationFailed, got: {err:?}"
    );
    assert!(
        err.to_string().contains("Credit balance is too low"),
        "{err}"
    );
}

#[test]
fn claude_nonzero_exit_maps_to_invocation_failed() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi
echo "API error: rate limited" >&2
exit 2"#,
    );
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);

    let err = run_prompt(&ClaudeCodeAdapter::new(), &request("hi", dir.path()))
        .expect_err("non-zero exit");
    match &err {
        AgentCliError::InvocationFailed { name, explain } => {
            assert_eq!(name, "claude-code");
            assert!(explain.contains("exit code 2"), "{explain}");
            assert!(explain.contains("rate limited"), "{explain}");
        }
        other => panic!("expected InvocationFailed, got: {other:?}"),
    }
}

#[test]
fn runner_kills_child_on_timeout() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi
sleep 5"#,
    );
    let _bin = EnvGuard::set("CLAUDE_CODE_BIN", &bin);

    let mut req = request("hi", dir.path());
    req.timeout = Duration::from_millis(300);
    let start = Instant::now();
    let err = run_prompt(&ClaudeCodeAdapter::new(), &req).expect_err("must time out");
    let elapsed = start.elapsed();

    assert!(
        matches!(&err, AgentCliError::Timeout { name, .. } if name == "claude-code"),
        "expected Timeout, got: {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "child not killed promptly: {elapsed:?}"
    );
}

#[test]
fn codex_detect_and_round_trip() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let args_file = dir.path().join("args.txt");
    let prompt_file = dir.path().join("prompt.txt");
    let body = format!(
        r#"if [ "$1" = "--version" ]; then echo "codex-cli 0.46.0"; exit 0; fi
printf '%s\n' "$@" > "{args}"
cat > "{prompt}"
printf 'The final answer.\n'"#,
        args = args_file.display(),
        prompt = prompt_file.display(),
    );
    let bin = fake_bin(dir.path(), "codex", &body);
    let _bin = EnvGuard::set("CODEX_BIN", &bin);
    let _key = EnvGuard::unset("OPENAI_API_KEY");

    let adapter = CodexAdapter::new();
    let probe = adapter.detect();
    assert!(probe.installed);
    assert_eq!(probe.version.as_deref(), Some("0.46.0"));
    assert_eq!(probe.logged_in, None);

    let answer =
        run_prompt(&adapter, &request("Explain blast radius.", dir.path())).expect("round trip");
    assert_eq!(answer, "The final answer.");

    let args: Vec<String> = fs::read_to_string(&args_file)
        .expect("args captured")
        .lines()
        .map(String::from)
        .collect();
    for expected in [
        "exec",
        "--ephemeral",
        "read-only",
        "--skip-git-repo-check",
        "-",
    ] {
        assert!(
            args.iter().any(|a| a == expected),
            "argv missing {expected:?}: {args:?}"
        );
    }
    let piped = fs::read_to_string(&prompt_file).expect("prompt captured");
    assert_eq!(piped, "Explain blast radius.");
}

#[test]
fn codex_build_invocation_model_flag() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(dir.path(), "codex", "exit 0");
    let _bin = EnvGuard::set("CODEX_BIN", &bin);
    let adapter = CodexAdapter::new();

    let mut req = request("hi", dir.path());
    let invocation = adapter.build_invocation(&req).expect("build");
    assert!(!invocation.argv.contains(&"-m".to_string()));
    assert_eq!(invocation.argv.last().map(String::as_str), Some("-"));

    req.model = Some("gpt-5-codex".to_string());
    let invocation = adapter.build_invocation(&req).expect("build with model");
    let m_pos = invocation
        .argv
        .iter()
        .position(|a| a == "-m")
        .expect("-m flag present");
    assert_eq!(
        invocation.argv.get(m_pos + 1).map(String::as_str),
        Some("gpt-5-codex")
    );
    assert_eq!(invocation.argv.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_parse_output_rejects_empty_stdout() {
    let raw = RawOutput {
        stdout: "  \n".to_string(),
        stderr: String::new(),
        exit_code: Some(0),
    };
    let err = CodexAdapter::new()
        .parse_output(&raw)
        .expect_err("empty stdout");
    assert!(
        matches!(&err, AgentCliError::OutputParse { name, .. } if name == "codex"),
        "expected OutputParse, got: {err:?}"
    );
}

#[test]
fn registry_lookup_and_unknown_name() {
    let registry = AdapterRegistry::builtin();
    assert_eq!(registry.names(), ["claude-code", "codex"]);
    assert_eq!(
        registry.get("codex").expect("codex registered").name(),
        "codex"
    );

    let err = registry
        .get("cursor")
        .map(AgentCliAdapter::name)
        .expect_err("unknown adapter");
    match &err {
        AgentCliError::UnknownAdapter { name, available } => {
            assert_eq!(name, "cursor");
            assert_eq!(available, "claude-code, codex");
        }
        other => panic!("expected UnknownAdapter, got: {other:?}"),
    }
}

#[test]
fn registry_detect_all_probes_every_adapter() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let claude = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi"#,
    );
    let codex = fake_bin(
        dir.path(),
        "codex",
        r#"if [ "$1" = "--version" ]; then echo "codex-cli 0.46.0"; exit 0; fi"#,
    );
    let _claude = EnvGuard::set("CLAUDE_CODE_BIN", &claude);
    let _codex = EnvGuard::set("CODEX_BIN", &codex);

    let probes = AdapterRegistry::builtin().detect_all();
    assert_eq!(probes.len(), 2);
    assert!(probes.iter().all(|(_, probe)| probe.installed));
    let versions: Vec<_> = probes
        .iter()
        .map(|(name, p)| (*name, p.version.clone()))
        .collect();
    assert_eq!(
        versions,
        [
            ("claude-code", Some("2.0.13".to_string())),
            ("codex", Some("0.46.0".to_string())),
        ]
    );
}
