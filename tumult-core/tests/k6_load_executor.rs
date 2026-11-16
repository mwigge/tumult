//! Behavior tests for the k6 load executor: summary/text metric parsing and
//! the full start/stop lifecycle against a fake `k6` binary.

use std::collections::HashMap;
use std::path::PathBuf;

use tumult_core::runner::k6::{
    k6_metric_or_warn, k6_summary_count, k6_summary_metric, parse_k6_counter, parse_k6_metric,
    parse_k6_rate, read_k6_summary, K6LoadExecutor,
};
use tumult_core::runner::{LoadExecutor, LoadHandle};
use tumult_core::types::{LoadConfig, LoadResult, LoadTool};
use tumult_test_utils::EnvGuard;

fn config() -> LoadConfig {
    LoadConfig {
        tool: LoadTool::K6,
        script: PathBuf::from("script.js"),
        vus: Some(7),
        duration_s: Some(1.0),
        thresholds: HashMap::new(),
        profile: None,
    }
}

// -- Text-output parsing ----------------------------------------------------

const K6_TEXT: &str = r"
     iterations...........: 1025 51.006998/s
     iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms p(99)=201.09ms
     checks_total.......: 1000    50.0/s
     checks_failed......: 25     1.25/s
";

#[test]
fn parse_k6_metric_extracts_named_stats() {
    assert_eq!(
        parse_k6_metric(K6_TEXT, "iteration_duration", "med"),
        Some(63.81)
    );
    assert_eq!(
        parse_k6_metric(K6_TEXT, "iteration_duration", "p(95)"),
        Some(148.01)
    );
    assert_eq!(
        parse_k6_metric(K6_TEXT, "iteration_duration", "p(99)"),
        Some(201.09)
    );
}

#[test]
fn parse_k6_metric_returns_none_for_missing_metric_or_stat() {
    assert_eq!(parse_k6_metric(K6_TEXT, "http_req_duration", "med"), None);
    assert_eq!(
        parse_k6_metric(K6_TEXT, "iteration_duration", "p(50)"),
        None
    );
    assert_eq!(parse_k6_metric("", "iteration_duration", "med"), None);
}

#[test]
fn parse_k6_metric_returns_none_when_the_value_is_not_numeric() {
    let output = "iteration_duration...: med=n/a";
    assert_eq!(parse_k6_metric(output, "iteration_duration", "med"), None);
}

#[test]
fn parse_k6_counter_extracts_the_count_column() {
    assert_eq!(parse_k6_counter(K6_TEXT, "iterations"), Some(1025));
    assert_eq!(parse_k6_counter(K6_TEXT, "checks_total"), Some(1000));
    assert_eq!(parse_k6_counter(K6_TEXT, "checks_failed"), Some(25));
    assert_eq!(parse_k6_counter(K6_TEXT, "vus"), None);
    // A matching line without a colon has no count to extract.
    assert_eq!(
        parse_k6_counter("iterations no colon here", "iterations"),
        None
    );
}

#[test]
fn parse_k6_rate_extracts_the_per_second_value() {
    assert_eq!(parse_k6_rate(K6_TEXT, "iterations"), Some(51.006_998));
    // A counter line without a rate yields nothing.
    assert_eq!(parse_k6_rate("iterations...: 1025", "iterations"), None);
    assert_eq!(parse_k6_rate("", "iterations"), None);
}

// -- JSON summary parsing ----------------------------------------------------

fn summary_json() -> serde_json::Value {
    serde_json::json!({
        "metrics": {
            "iterations": {"count": 300, "rate": 29.82},
            "iteration_duration": {"med": 63.81, "p(95)": 148.01, "p(99)": 201.09},
            "checks_total": {"count": 300},
            "checks_failed": {"count": 6}
        }
    })
}

#[test]
fn summary_metric_and_count_look_up_nested_fields() {
    let summary = summary_json();
    assert_eq!(
        k6_summary_metric(Some(&summary), "iterations", "rate"),
        Some(29.82)
    );
    assert_eq!(
        k6_summary_metric(Some(&summary), "iteration_duration", "p(95)"),
        Some(148.01)
    );
    assert_eq!(k6_summary_count(Some(&summary), "checks_failed"), Some(6));
}

#[test]
fn summary_lookups_return_none_for_absent_or_mistyped_paths() {
    let summary = summary_json();
    assert_eq!(k6_summary_metric(None, "iterations", "rate"), None);
    assert_eq!(k6_summary_metric(Some(&summary), "vus_max", "rate"), None);
    assert_eq!(k6_summary_metric(Some(&summary), "iterations", "avg"), None);
    assert_eq!(k6_summary_count(None, "iterations"), None);
    // `rate` is a float, not an unsigned count.
    assert_eq!(k6_summary_count(Some(&summary), "iteration_duration"), None);
}

#[test]
fn metric_or_warn_defaults_missing_values_to_zero() {
    assert!((k6_metric_or_warn(Some(12.5), "iterations.rate") - 12.5).abs() < f64::EPSILON);
    assert!((k6_metric_or_warn(None, "iterations.rate")).abs() < f64::EPSILON);
}

#[test]
fn read_k6_summary_parses_a_valid_export_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("summary.json");
    std::fs::write(&path, serde_json::to_string(&summary_json()).expect("json")).expect("write");

    let summary = read_k6_summary(&path).expect("valid summary parses");
    assert_eq!(summary["metrics"]["iterations"]["count"], 300);
}

#[test]
fn read_k6_summary_returns_none_for_missing_or_invalid_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(read_k6_summary(&dir.path().join("absent.json")).is_none());

    let invalid = dir.path().join("invalid.json");
    std::fs::write(&invalid, "not json at all").expect("write");
    assert!(read_k6_summary(&invalid).is_none());
}

// -- start/stop lifecycle against a fake k6 binary ---------------------------

/// Write an executable fake-k6 script and point `TUMULT_K6_BINARY` at it.
/// The executor resolves the k6 binary through that variable, so the guard
/// keeps the tests that point it at a fake from running concurrently.
fn install_fake_k6(env: &mut EnvGuard, dir: &tempfile::TempDir, body: &str) {
    let path = dir.path().join("fake-k6");
    #[cfg(unix)]
    tumult_test_utils::write_executable(&path, body);
    #[cfg(not(unix))]
    std::fs::write(&path, body).expect("write fake k6");
    env.set("TUMULT_K6_BINARY", &path);
}

/// A fake k6 that writes a `--summary-export` JSON file and exits cleanly.
const FAKE_K6_WITH_SUMMARY: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  if [ "$1" = "--summary-export" ]; then
    shift
    printf '%s' '{"metrics":{"iterations":{"count":300,"rate":29.82},"iteration_duration":{"med":63.81,"p(95)":148.01,"p(99)":201.09},"checks_total":{"count":300},"checks_failed":{"count":6}}}' > "$1"
  fi
  shift
done
exit 0
"#;

/// A fake k6 that writes no summary file, forcing the text-parsing fallback.
const FAKE_K6_TEXT_ONLY: &str = r#"#!/bin/sh
echo "     iterations...........: 1025 51.006998/s"
echo "     iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms p(99)=201.09ms"
echo "     checks_total.......: 1000    50.0/s"
echo "     checks_failed......: 25     1.25/s"
exit 0
"#;

#[test]
fn stop_collects_metrics_from_the_json_summary_export() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_k6(&mut env, &dir, FAKE_K6_WITH_SUMMARY);

    let executor = K6LoadExecutor;
    let handle = executor.start(&config()).expect("fake k6 starts");
    let result: LoadResult = executor.stop(handle).expect("fake k6 stops");

    assert_eq!(result.tool, LoadTool::K6);
    assert_eq!(result.vus, 7);
    assert!((result.throughput_rps - 29.82).abs() < f64::EPSILON);
    assert!((result.latency_p50_ms - 63.81).abs() < f64::EPSILON);
    assert!((result.latency_p95_ms - 148.01).abs() < f64::EPSILON);
    assert!((result.latency_p99_ms - 201.09).abs() < f64::EPSILON);
    // 6 failed checks out of 300.
    assert!((result.error_rate - 0.02).abs() < f64::EPSILON);
    assert_eq!(result.total_requests, 300);
    assert!(result.thresholds_met);
    assert!(result.ended_at_ns >= result.started_at_ns);
    assert!(result.duration_s >= 0.0);
}

#[test]
fn stop_falls_back_to_text_parsing_without_a_summary_export() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_k6(&mut env, &dir, FAKE_K6_TEXT_ONLY);

    let executor = K6LoadExecutor;
    let handle = executor.start(&config()).expect("fake k6 starts");
    let result = executor.stop(handle).expect("fake k6 stops");

    assert!((result.throughput_rps - 51.006_998).abs() < f64::EPSILON);
    assert!((result.latency_p50_ms - 63.81).abs() < f64::EPSILON);
    assert!((result.latency_p95_ms - 148.01).abs() < f64::EPSILON);
    assert!((result.latency_p99_ms - 201.09).abs() < f64::EPSILON);
    assert!((result.error_rate - 0.025).abs() < f64::EPSILON);
    assert_eq!(result.total_requests, 1025);
    assert!(result.thresholds_met);
}

#[test]
fn stop_marks_thresholds_unmet_when_k6_exits_nonzero() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_k6(&mut env, &dir, "#!/bin/sh\nexit 1\n");

    let executor = K6LoadExecutor;
    let handle = executor.start(&config()).expect("fake k6 starts");
    let result = executor
        .stop(handle)
        .expect("exit code still yields a result");

    assert!(!result.thresholds_met);
    // No summary and no text metrics: everything defaults to zero.
    assert_eq!(result.total_requests, 0);
    assert!((result.error_rate - 0.0).abs() < f64::EPSILON);
}

#[test]
fn start_errors_when_the_k6_binary_cannot_be_spawned() {
    let mut env = EnvGuard::lock();
    env.set("TUMULT_K6_BINARY", "/nonexistent/tumult-k6-test-binary");

    let executor = K6LoadExecutor;
    let Err(error) = executor.start(&config()) else {
        panic!("a missing binary must fail to start");
    };

    assert!(
        error.contains("failed to start k6"),
        "unexpected error: {error}"
    );
}

#[test]
fn stop_rejects_a_handle_from_another_executor() {
    let executor = K6LoadExecutor;
    let error = executor
        .stop(LoadHandle {
            inner: Box::new(()),
        })
        .expect_err("a foreign handle must be rejected");

    assert_eq!(error, "invalid load handle");
}

#[test]
fn abort_kills_a_running_k6_at_once() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_k6(&mut env, &dir, "#!/bin/sh\nexec sleep 30\n");

    let executor = K6LoadExecutor;
    let handle = executor.start(&config()).expect("fake k6 starts");
    let started = std::time::Instant::now();
    let outcome = executor.abort(handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "abort blocked for {elapsed:?}"
    );
    let result = outcome.expect("an aborted k6 still leaves a marked result");
    assert!(result.execution.aborted);
    assert_eq!(result.total_requests, 0, "a killed k6 has no summary");
    assert!(!result.thresholds_met);
    assert!(
        result.execution.reasons.join(" ").contains("aborted"),
        "{:?}",
        result.execution.reasons
    );
}

/// Waits until the process whose pid is in `pid_file` has exited: it is a
/// zombie until the executor reaps it, or gone.
#[cfg(unix)]
fn wait_for_exit(pid_file: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let pid = std::fs::read_to_string(pid_file).unwrap_or_default();
        // Without its newline the pid is not written yet, or not in full.
        if let Some(pid) = pid.strip_suffix('\n') {
            let pid: u32 = pid
                .parse()
                .unwrap_or_else(|e| panic!("the pid file holds {pid:?}: {e}"));
            if !is_running(pid) {
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fake k6 did not exit in time"
        );
        // sleep: interval of the bounded poll above
        std::thread::sleep(std::time::Duration::from_millis(10));
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

#[cfg(unix)]
#[test]
fn abort_collects_a_k6_that_already_finished() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("fake-k6.pid");
    let body = FAKE_K6_WITH_SUMMARY.replacen(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\necho $$ > '{}'\n", pid_file.display()),
        1,
    );
    install_fake_k6(&mut env, &dir, &body);

    let executor = K6LoadExecutor;
    let handle = executor.start(&config()).expect("fake k6 starts");
    wait_for_exit(&pid_file);
    let result = executor.abort(handle).expect("a finished k6 is collected");

    assert!(!result.execution.aborted);
    assert_eq!(result.total_requests, 300);
}
