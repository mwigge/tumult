//! Tests for the `ProviderExecutor`.

use super::super::*;
use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType};

// ── ProviderExecutor tests ────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_runs_echo() {
    let activity = Activity {
        name: "echo-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "echo".into(),
            arguments: vec!["hello world".into()],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("hello world"));
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_captures_failure() {
    let activity = Activity {
        name: "false-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "false".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_nonexistent_returns_error() {
    let activity = Activity {
        name: "bad-cmd".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "/nonexistent/binary".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: None,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    assert!(outcome.error.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_provider_rejects_unknown_plugin() {
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "unknown-plugin".into(),
            function: "test-fn".into(),
            arguments: std::collections::HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    let error = outcome.error.as_ref().unwrap();
    assert!(error.contains("unknown native plugin"));
    // The typed error lists the registered plugins for discoverability.
    assert!(error.contains("tumult-kubernetes"));
    assert!(error.contains("tumult-net"));
    assert!(error.contains("tumult-ssh"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_provider_rejects_unknown_function() {
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "tumult-ssh".into(),
            function: "command-exeucte".into(), // typo must not silently run anything
            arguments: std::collections::HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor::new();
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    let error = outcome.error.as_ref().unwrap();
    assert!(error.contains("unknown tumult-ssh function"));
    assert!(error.contains("execute"), "should list available functions");
}

// ── Pipe draining / timeout-kill tests ──────────────────────

fn process_activity(path: &str, arguments: Vec<String>, timeout_s: Option<f64>) -> Activity {
    Activity {
        name: "drain-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: path.into(),
            arguments,
            env: std::collections::HashMap::new(),
            timeout_s,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

/// A child emitting far more than the ~64 KiB OS pipe buffer must not be
/// killed as a false timeout: the executor drains the pipes while waiting.
#[tokio::test(flavor = "multi_thread")]
async fn async_executor_drains_large_output_without_false_timeout() {
    // ~1.4 MB of stdout — many times the pipe buffer.
    let activity = process_activity("seq", vec!["1".into(), "200000".into()], Some(10.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(
        outcome.success,
        "large output must not time out: {outcome:?}"
    );
    let output = outcome.output.expect("output must be captured");
    assert!(
        output.ends_with("200000"),
        "output should be drained to completion"
    );
}

/// Same as above, but through the synchronous fallback path (no runtime).
#[test]
fn sync_executor_drains_large_output_without_false_timeout() {
    let activity = process_activity("seq", vec!["1".into(), "200000".into()], Some(10.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(
        outcome.success,
        "large output must not time out: {outcome:?}"
    );
    let output = outcome.output.expect("output must be captured");
    assert!(output.ends_with("200000"));
}

/// Output beyond the 8 MiB capture cap is truncated with a note rather than
/// captured in full, but the child still runs to completion.
#[test]
fn sync_executor_truncates_output_beyond_cap() {
    // ~12 MB of stdout — beyond the 8 MiB capture cap.
    let activity = process_activity("seq", vec!["1".into(), "1500000".into()], Some(30.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(outcome.success);
    let output = outcome.output.expect("output must be captured");
    assert!(
        output.ends_with("[output truncated at 8 MiB]"),
        "truncated output must carry the truncation note"
    );
}

/// On timeout the executor kills the child's whole process group, so a
/// grandchild spawned by a wrapper script cannot outlive the timeout.
#[cfg(unix)]
#[test]
fn sync_executor_timeout_kills_process_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("grandchild.pid");
    // The wrapper starts a grandchild that would outlive it by far, and is
    // killed after 2s, long after it wrote the pid. If only the direct child
    // were killed, the grandchild would still be running afterwards.
    let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
    let activity = process_activity("sh", vec!["-c".into(), script], Some(2.0));

    let outcome = ProviderExecutor::new().execute(&activity);

    assert!(!outcome.success, "timed-out process must fail");
    let error = outcome.error.expect("timeout must be reported");
    assert!(error.contains("timed out"), "unexpected error: {error}");

    let pid = std::fs::read_to_string(&pid_file).expect("the wrapper started its grandchild");
    let pid: u32 = pid
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("the pid file holds {pid:?}: {e}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while is_running(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "grandchild {pid} survived the timeout: the process group was not killed"
        );
        // sleep: interval of the bounded poll for the grandchild's end
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Whether process `pid` exists and has not exited (a zombie has).
///
/// Read from `/proc`, so that "gone" is something the kernel said and not
/// a tool that failed to run.
#[cfg(target_os = "linux")]
fn is_running(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // "pid (comm) state ...": comm may itself hold spaces and brackets.
        Ok(stat) => {
            let (_, after_comm) = stat
                .rsplit_once(')')
                .unwrap_or_else(|| panic!("unexpected /proc/{pid}/stat: {stat}"));
            let state = after_comm
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("no state in /proc/{pid}/stat: {stat}"));
            state != "Z" && state != "X"
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        // ESRCH: the process went away between open and read.
        Err(e) if e.raw_os_error() == Some(3) => false,
        Err(e) => panic!("cannot read /proc/{pid}/stat: {e}"),
    }
}

/// Whether process `pid` exists and has not exited (a zombie has).
///
/// `ps` reports a process that is gone the way it reports its own failure,
/// so it first has to find this process.
#[cfg(all(unix, not(target_os = "linux")))]
fn is_running(pid: u32) -> bool {
    let state_of = |pid: u32| {
        let ps = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        String::from_utf8_lossy(&ps.stdout).trim().to_string()
    };
    assert!(
        !state_of(std::process::id()).is_empty(),
        "ps does not report this process, so it cannot tell that another is gone"
    );
    let state = state_of(pid);
    !state.is_empty() && !state.starts_with('Z')
}
