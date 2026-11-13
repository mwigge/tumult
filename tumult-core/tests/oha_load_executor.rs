//! Behavior tests for the oha load adapter: the start/stop lifecycle
//! against a fake `oha` binary, including windowed sampling for a soak
//! profile, the plan clock, planned against completed windows, the
//! normal stop, the abort with its partial evidence, the kill-after-grace
//! path and the no-evidence outcomes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tumult_core::runner::oha::OhaLoadExecutor;
use tumult_core::runner::{load_executor_for, LoadExecutor};
use tumult_core::types::{LoadConfig, LoadProfile, LoadTool};

// The executor resolves the binary through `TUMULT_OHA_BINARY`; tests that
// point it at a fake binary must not run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn config(profile: Option<LoadProfile>, duration_s: f64) -> LoadConfig {
    LoadConfig {
        tool: LoadTool::Oha,
        script: PathBuf::from("http://127.0.0.1:1/health"),
        vus: Some(4),
        duration_s: Some(duration_s),
        thresholds: HashMap::new(),
        profile,
    }
}

fn soak(duration_s: f64) -> LoadConfig {
    config(
        Some(LoadProfile::Soak {
            sample_interval_s: 1.0,
        }),
        duration_s,
    )
}

/// The lines a fake wrote to `name` next to itself, one per window.
fn recorded(dir: &tempfile::TempDir, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.path().join(name))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Write an executable fake-oha script and point `TUMULT_OHA_BINARY` at it.
fn install_fake_oha(dir: &tempfile::TempDir, body: &str) {
    let path = dir.path().join("fake-oha");
    std::fs::write(&path, body).expect("write fake oha");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake oha");
    }
    std::env::set_var("TUMULT_OHA_BINARY", &path);
}

/// Block until the fake oha has been invoked `n` times (each invocation
/// appends a line to `calls` next to the script), so a test can let the
/// planned windows start before it asks the executor to stop.
fn wait_for_calls(dir: &tempfile::TempDir, n: usize) {
    let calls = dir.path().join("calls");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let seen = std::fs::read_to_string(&calls).map_or(0, |text| text.lines().count());
        if seen >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fake oha ran {seen} of {n} windows"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A fake oha with the command line of oha 1.x (`--output-format json`;
/// the old `-j` is rejected, as the real binary does) that insists on the
/// flags the adapter must pass: `-w` so in-flight requests are measured
/// and `-t` so a hung request becomes a timeout. It prints a JSON summary
/// whose 200 count is the concurrency it was asked for, so windows are
/// distinguishable, plus one 503.
const FAKE_OHA_JSON: &str = r#"#!/bin/sh
conc=0
format=text
wait=no
timeout=
length=
while [ $# -gt 0 ]; do
  if [ "$1" = "-c" ]; then shift; conc="$1"; fi
  if [ "$1" = "-z" ]; then shift; length="$1"; fi
  if [ "$1" = "-t" ]; then shift; timeout="$1"; fi
  if [ "$1" = "-w" ]; then wait=yes; fi
  if [ "$1" = "--output-format" ]; then shift; format="$1"; fi
  if [ "$1" = "-j" ]; then echo "error: unexpected argument '-j' found" >&2; exit 2; fi
  if [ "$1" = "--version" ]; then exit 0; fi
  shift
done
if [ "$format" != "json" ]; then echo "text output" ; exit 0; fi
if [ "$wait" != "yes" ] || [ -z "$timeout" ]; then echo "missing -w or -t" >&2; exit 2; fi
echo x >> "$(dirname "$0")/calls"
echo "$length" >> "$(dirname "$0")/lengths"
echo "$timeout" >> "$(dirname "$0")/timeouts"
#BEFORE_SUMMARY
printf '%s' "{\"summary\":{\"successRate\":1.0,\"total\":0.1,\"slowest\":0.2,\"fastest\":0.01,\"average\":0.05,\"requestsPerSec\":50.0,\"totalData\":1,\"sizePerRequest\":1,\"sizePerSec\":1},\"responseTimeHistogram\":{},\"latencyPercentiles\":{\"p50\":0.02,\"p95\":0.08,\"p99\":0.1},\"rps\":{},\"details\":{},\"statusCodeDistribution\":{\"200\":$conc,\"503\":1},\"errorDistribution\":{}}"
exit 0
"#;

/// A fake oha that prints `summary` verbatim for every window.
fn fake_printing(summary: &str) -> String {
    format!(
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\ncat <<'JSON'\n{summary}\nJSON\nexit 0\n"
    )
}

/// A fake oha that records its pid and then sleeps far longer than any
/// test allows.
fn fake_hanging(dir: &tempfile::TempDir) -> String {
    format!(
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\necho $$ > {}\nexec sleep 30\n",
        dir.path().join("pid").display()
    )
}

/// Whether the process the hanging fake recorded is still alive. `false`
/// when the fake never got as far as recording itself.
#[cfg(target_os = "linux")]
fn hanging_fake_is_alive(dir: &tempfile::TempDir) -> bool {
    // Give a fake that was just spawned the time to record itself.
    std::thread::sleep(Duration::from_millis(300));
    let Ok(pid) = std::fs::read_to_string(dir.path().join("pid")) else {
        return false;
    };
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.trim()));
    // A reaped process has no entry; a zombie is not running either.
    stat.is_ok_and(|s| !s.contains(") Z "))
}

#[test]
fn flat_config_is_one_window_and_one_sample() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("fake oha starts");
    let result = executor.stop(handle).expect("fake oha stops");

    assert_eq!(result.tool, LoadTool::Oha);
    assert_eq!(result.vus, 4);
    assert_eq!(result.samples.len(), 1);
    // 4 answers of 200 plus one 503.
    assert_eq!(result.total_requests, 5);
    assert!((result.throughput_rps - 50.0).abs() < f64::EPSILON);
    assert!((result.latency_p50_ms - 20.0).abs() < 1e-9);
    assert!((result.latency_p95_ms - 80.0).abs() < 1e-9);
    assert!((result.latency_p99_ms - 100.0).abs() < 1e-9);
    // The 503 counts as an error even though oha's successRate says 1.0.
    assert!((result.error_rate - 0.2).abs() < 1e-9);
    assert!(result.thresholds_met);
    assert!(result.ended_at_ns >= result.started_at_ns);
    // The sample is as long as the window really ran (`summary.total`),
    // not as long as it was planned.
    assert!((result.samples[0].duration_s - 0.1).abs() < 1e-9);
    assert_eq!(result.execution.planned_windows, Some(1));
    assert_eq!(result.execution.completed_windows, Some(1));
    assert!(!result.execution.aborted);
    assert!(result.execution.reasons.is_empty());
    // A one-second window gives up on an answer before its slot is over.
    assert_eq!(recorded(&dir, "timeouts"), vec!["750ms"]);
}

#[test]
fn the_request_timeout_follows_the_window_up_to_thirty_seconds() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    std::env::remove_var("TUMULT_OHA_REQUEST_TIMEOUT_S");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let executor = OhaLoadExecutor::default();
    for duration_s in [10.0, 600.0] {
        let handle = executor.start(&config(None, duration_s)).expect("starts");
        executor.stop(handle).expect("stops");
    }
    assert_eq!(recorded(&dir, "timeouts"), vec!["10s", "30s"]);

    // An explicit timeout is used as given.
    let executor = OhaLoadExecutor::default().with_request_timeout(Duration::from_secs(45));
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    executor.stop(handle).expect("stops");
    assert_eq!(
        recorded(&dir, "timeouts").last().map(String::as_str),
        Some("45s")
    );
}

/// What oha 1.16 prints against a target that never answers (`-w -t`),
/// after outliving its one-second slot by half a second while its last
/// requests time out.
fn fake_blacked_out() -> String {
    "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\nsleep 1.5\ncat <<'JSON'\n\
     {\"summary\":{\"successRate\":0.0,\"total\":1.5,\"slowest\":null,\"requestsPerSec\":2.6},\
     \"latencyPercentiles\":{\"p50\":null,\"p95\":null,\"p99\":null},\
     \"statusCodeDistribution\":{},\"errorDistribution\":{\"timeout\":4}}\nJSON\nexit 0\n"
        .to_string()
}

/// Run a four-window soak of one-second windows to the end of its plan.
/// The stop comes three seconds after the plan's last slot has closed, so
/// a loaded machine cannot turn the wait into windows left unstarted.
fn run_soak_to_its_end(config: &LoadConfig) -> tumult_core::types::LoadResult {
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(config).expect("starts");
    std::thread::sleep(Duration::from_secs(7));
    executor.stop(handle).expect("stops")
}

#[test]
fn a_blacked_out_soak_of_one_second_windows_is_recorded_not_truncated() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    std::env::remove_var("TUMULT_OHA_REQUEST_TIMEOUT_S");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, &fake_blacked_out());

    // Every started window ends half a second into the next one's slot,
    // so the plan clock shortens or drops the windows behind it. Which
    // of the two depends on scheduling; neither is a truncation.
    let result = run_soak_to_its_end(&soak(4.0));

    assert_eq!(result.execution.planned_windows, Some(4));
    assert!(
        !result.execution.truncated(),
        "{:?}",
        result.execution.reasons
    );
    assert_eq!(result.execution.unanswered_windows, 4);
    assert!(result.samples.is_empty());
    assert!((result.error_rate - 1.0).abs() < 1e-9);
    assert!(
        result.thresholds_met,
        "no threshold declared: {:?}",
        result.execution.reasons
    );
}

#[test]
fn a_blacked_out_soak_misses_a_declared_threshold() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    std::env::remove_var("TUMULT_OHA_REQUEST_TIMEOUT_S");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, &fake_blacked_out());

    let mut bounded = soak(4.0);
    bounded.thresholds.insert("error_rate".into(), 0.5);
    let result = run_soak_to_its_end(&bounded);

    assert!(!result.thresholds_met);
    assert!(
        !result.execution.truncated(),
        "the miss is the threshold's, not a truncation: {:?}",
        result.execution.reasons
    );
    assert_eq!(result.execution.unanswered_windows, 4);
}

#[test]
fn soak_profile_streams_one_sample_per_interval() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let soak = LoadProfile::Soak {
        sample_interval_s: 1.0,
    };
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(Some(soak), 3.0)).expect("starts");
    // Stop skips windows that have not started, so let all three begin.
    wait_for_calls(&dir, 3);
    let result = executor.stop(handle).expect("stops");

    assert_eq!(result.samples.len(), 3, "{:?}", result.samples);
    assert!(result.samples.iter().all(|s| s.vus == 4 && s.requests == 5));
    assert!(result
        .samples
        .windows(2)
        .all(|pair| pair[0].ts_ns <= pair[1].ts_ns));
    assert_eq!(result.total_requests, 15);
}

#[test]
fn ramp_profile_steps_the_concurrency_per_window() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let ramp = LoadProfile::Ramp {
        start_vus: 1,
        ramp_up_s: 0.2,
    };
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(Some(ramp), 0.3)).expect("starts");
    wait_for_calls(&dir, 2);
    let result = executor.stop(handle).expect("stops");

    // One window per stage: the ramp at its midpoint level (1 -> 4 at
    // t = 0.5 is 2.5, rounded to 3), then the hold at 4.
    let vus: Vec<u32> = result.samples.iter().map(|s| s.vus).collect();
    assert_eq!(vus, vec![3, 4]);
}

#[test]
fn stop_finishes_the_current_window_and_skips_the_rest() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // Each window takes ~300 ms regardless of `-z`.
    let slow = FAKE_OHA_JSON.replacen("#BEFORE_SUMMARY", "sleep 0.3", 1);
    install_fake_oha(&dir, &slow);

    let soak = LoadProfile::Soak {
        sample_interval_s: 1.0,
    };
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(Some(soak), 60.0)).expect("starts");
    let started = Instant::now();
    let result = executor.stop(handle).expect("stops");
    let elapsed = started.elapsed();

    // Exactly the in-flight window completed; the other 59 never started,
    // and stop did not sit out the plan's remaining 60 s.
    assert_eq!(result.samples.len(), 1, "{:?}", result.samples);
    assert!(elapsed < Duration::from_secs(5), "stop took {elapsed:?}");
    // One window of sixty is not the plan: recorded, and never a pass.
    assert_eq!(result.execution.planned_windows, Some(60));
    assert_eq!(result.execution.completed_windows, Some(1));
    assert!(!result.execution.aborted);
    assert!(!result.thresholds_met, "a truncated plan must not pass");
    let reasons = result.execution.reasons.join("; ");
    assert!(
        reasons.contains("the method ended before 59 of 60 windows started"),
        "{reasons}"
    );
}

#[test]
fn a_window_that_outlives_its_grace_is_killed_and_reported() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // Ignores `-z` entirely and would run for 30 s.
    install_fake_oha(
        &dir,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\nsleep 30\nexit 0\n",
    );

    let executor = OhaLoadExecutor::with_window_grace(Duration::from_millis(200))
        .with_request_timeout(Duration::from_millis(100));
    let handle = executor.start(&config(None, 0.1)).expect("starts");
    let started = Instant::now();
    let result = executor
        .stop(handle)
        .expect("a killed window is still a result");
    let elapsed = started.elapsed();

    assert!(result.samples.is_empty());
    assert!(!result.thresholds_met, "no completed window must not pass");
    assert_eq!(result.total_requests, 0);
    assert!(
        elapsed < Duration::from_secs(5),
        "the kill was late: {elapsed:?}"
    );
}

#[test]
fn stop_lets_a_window_longer_than_the_grace_finish() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // The window takes 0.8 s; grace and request timeout are far shorter.
    let slow = FAKE_OHA_JSON.replacen("#BEFORE_SUMMARY", "sleep 0.8", 1);
    install_fake_oha(&dir, &slow);

    let executor = OhaLoadExecutor::with_window_grace(Duration::from_millis(50))
        .with_request_timeout(Duration::from_millis(50));
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let result = executor.stop(handle).expect("stops");

    assert_eq!(
        result.samples.len(),
        1,
        "a healthy in-flight window must not be killed by stop"
    );
    assert!(result.thresholds_met);
}

#[test]
fn failed_window_is_reported_not_hidden() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(
        &dir,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\necho broken >&2\nexit 2\n",
    );

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let result = executor.stop(handle).expect("stops");

    assert!(result.samples.is_empty());
    assert!(!result.thresholds_met);
    assert_eq!(result.total_requests, 0);
    assert_eq!(result.execution.completed_windows, Some(0));
    let reasons = result.execution.reasons.join("; ");
    assert!(reasons.contains("1 of 1 windows failed"), "{reasons}");
    assert!(reasons.contains("broken"), "{reasons}");
}

#[test]
fn dropping_a_handle_right_after_start_does_not_wait_for_a_window() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, &fake_hanging(&dir));

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 600.0)).expect("starts");
    // No waiting for the window to be up: the drop races the first spawn.
    let started = Instant::now();
    drop(handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "drop waited {elapsed:?} for a window it should have killed"
    );
    #[cfg(target_os = "linux")]
    assert!(!hanging_fake_is_alive(&dir), "drop left an oha running");
}

#[test]
fn abort_kills_the_in_flight_window_at_once() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, &fake_hanging(&dir));

    // A flat ten-minute load: one window that a halt must not sit out.
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 600.0)).expect("starts");
    let started = Instant::now();
    let outcome = executor.abort(handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "abort blocked for {elapsed:?}"
    );
    // The cut window printed nothing, so there is no evidence — but the
    // abort itself is a result, marked as such, never an error.
    let result = outcome.expect("an aborted flat load is still a result");
    assert!(result.execution.aborted);
    assert_eq!(result.execution.planned_windows, Some(1));
    assert_eq!(result.execution.completed_windows, Some(0));
    assert_eq!(result.total_requests, 0);
    assert!(!result.thresholds_met);
    #[cfg(target_os = "linux")]
    assert!(!hanging_fake_is_alive(&dir), "abort left an oha running");
}

/// A fake oha that, like the real one, prints what it had measured when
/// it is interrupted, and otherwise runs far longer than any test allows.
const FAKE_OHA_PARTIAL_ON_SIGINT: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && exit 0
trap 'printf "%s" "{\"summary\":{\"total\":3.2},\"latencyPercentiles\":{\"p50\":0.02,\"p95\":0.08,\"p99\":0.1},\"statusCodeDistribution\":{\"200\":64},\"errorDistribution\":{}}"; exit 0' INT
echo x >> "$(dirname "$0")/calls"
sleep 30 &
wait $!
"#;

#[test]
fn abort_of_a_flat_load_keeps_the_partial_evidence_and_marks_it() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_PARTIAL_ON_SIGINT);

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 600.0)).expect("starts");
    wait_for_calls(&dir, 1);
    // Let the shell reach its `wait` with the trap installed.
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let result = executor.abort(handle).expect("an aborted load is a result");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(3),
        "abort blocked for {elapsed:?}"
    );
    assert!(result.execution.aborted, "the abort must be marked");
    assert_eq!(result.execution.planned_windows, Some(1));
    assert_eq!(
        result.execution.completed_windows,
        Some(0),
        "a cut window is evidence, not a completed window"
    );
    assert_eq!(result.total_requests, 64);
    assert_eq!(result.samples.len(), 1);
    assert!((result.samples[0].duration_s - 3.2).abs() < 1e-9);
    assert!(!result.thresholds_met, "an aborted load never passes");
}

#[test]
fn abort_keeps_the_windows_that_completed() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // The first two windows answer at once; the third hangs.
    let third_hangs = FAKE_OHA_JSON.replacen(
        "#BEFORE_SUMMARY",
        "[ \"$(wc -l < \"$(dirname \"$0\")/calls\")\" -gt 2 ] && sleep 30",
        1,
    );
    install_fake_oha(&dir, &third_hangs);

    let soak = LoadProfile::Soak {
        sample_interval_s: 1.0,
    };
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(Some(soak), 60.0)).expect("starts");
    wait_for_calls(&dir, 3);
    let started = Instant::now();
    let result = executor.abort(handle).expect("completed windows are kept");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "abort blocked for {elapsed:?}"
    );
    assert_eq!(result.samples.len(), 2, "{:?}", result.samples);
    assert_eq!(result.total_requests, 10);
    assert!(result.execution.aborted);
    assert_eq!(result.execution.planned_windows, Some(60));
    assert_eq!(result.execution.completed_windows, Some(2));
    assert!(!result.thresholds_met);
}

#[test]
fn a_window_that_sent_nothing_never_meets_thresholds() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // What oha 1.16 prints when no request was answered and none failed.
    install_fake_oha(
        &dir,
        &fake_printing(
            r#"{"summary":{"successRate":null,"total":0.5,"requestsPerSec":0.0},"latencyPercentiles":{"p50":null,"p95":null,"p99":null},"statusCodeDistribution":{},"errorDistribution":{}}"#,
        ),
    );

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let result = executor.stop(handle).expect("stops");

    assert_eq!(result.total_requests, 0);
    assert!(result.samples.is_empty());
    assert!(!result.thresholds_met, "zero requests must not pass");
}

#[test]
fn a_window_where_every_request_timed_out_is_all_errors() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // What oha 1.16 prints against a target that never answers (`-w -t`).
    install_fake_oha(
        &dir,
        &fake_printing(
            r#"{"summary":{"successRate":0.0,"total":1.0,"slowest":null,"requestsPerSec":3.9},"latencyPercentiles":{"p50":null,"p95":null,"p99":null},"statusCodeDistribution":{},"errorDistribution":{"timeout":4}}"#,
        ),
    );

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let result = executor.stop(handle).expect("stops");

    assert_eq!(result.total_requests, 4);
    assert!((result.error_rate - 1.0).abs() < 1e-9);
    assert!(result.samples.is_empty(), "no answer, no latency sample");
    assert_eq!(result.execution.unanswered_windows, 1);
    // No threshold was declared: the blackout is recorded, and it is for
    // the hypothesis and the objectives to judge, as with k6.
    assert!(result.thresholds_met);

    // Against a declared bound the same window is a miss.
    let mut bounded = config(None, 1.0);
    bounded.thresholds.insert("error_rate".into(), 0.05);
    let handle = executor.start(&bounded).expect("starts");
    let result = executor.stop(handle).expect("stops");
    assert!(!result.thresholds_met);
}

#[test]
fn a_straggler_holding_stdout_does_not_hang_a_finished_window() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // A child of the wrapper outlives it with the stdout pipe still open.
    let straggler = FAKE_OHA_JSON.replacen("#BEFORE_SUMMARY", "sleep 30 &", 1);
    install_fake_oha(&dir, &straggler);

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let started = Instant::now();
    let result = executor.stop(handle).expect("stops");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "the drain hung for {elapsed:?}"
    );
    assert_eq!(result.samples.len(), 1, "the summary was still read");
}

#[test]
fn a_straggler_outside_the_group_fails_the_window_instead_of_hanging() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // Escapes the process group, so the group kill cannot close the pipe.
    let escaped = FAKE_OHA_JSON.replacen("#BEFORE_SUMMARY", "setsid sleep 30 &", 1);
    install_fake_oha(&dir, &escaped);

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    let started = Instant::now();
    let result = executor.stop(handle).expect("stops");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the drain hung for {elapsed:?}"
    );
    assert!(result.samples.is_empty());
    assert!(!result.thresholds_met);
}

#[test]
fn start_errors_when_the_oha_binary_cannot_be_spawned() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    std::env::set_var("TUMULT_OHA_BINARY", "/nonexistent/tumult-oha-test-binary");

    let executor = OhaLoadExecutor::default();
    let Err(error) = executor.start(&config(None, 1.0)) else {
        panic!("a missing binary must fail to start");
    };
    assert!(error.contains("failed to start oha"), "{error}");
}

#[test]
fn adapter_selector_covers_k6_and_oha_but_not_jmeter() {
    assert!(load_executor_for(&LoadTool::K6).is_ok());
    assert!(load_executor_for(&LoadTool::Oha).is_ok());
    let Err(err) = load_executor_for(&LoadTool::Jmeter) else {
        panic!("jmeter must have no adapter");
    };
    assert!(err.contains("jmeter"), "{err}");
}
