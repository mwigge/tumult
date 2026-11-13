//! Behavior tests for the oha load adapter: the start/stop lifecycle
//! against a fake `oha` binary, including windowed sampling for a soak
//! profile, early stop, the kill-after-grace path and the no-evidence
//! outcome.

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
/// the old `-j` is rejected, as the real binary does). It prints a JSON
/// summary whose 200 count is the concurrency it was asked for, so windows
/// are distinguishable, plus one 503 and the deadline aborts a real run
/// ends with.
const FAKE_OHA_JSON: &str = r#"#!/bin/sh
conc=0
format=text
while [ $# -gt 0 ]; do
  if [ "$1" = "-c" ]; then shift; conc="$1"; fi
  if [ "$1" = "--output-format" ]; then shift; format="$1"; fi
  if [ "$1" = "-j" ]; then echo "error: unexpected argument '-j' found" >&2; exit 2; fi
  if [ "$1" = "--version" ]; then exit 0; fi
  shift
done
if [ "$format" != "json" ]; then echo "text output" ; exit 0; fi
echo x >> "$(dirname "$0")/calls"
printf '%s' "{\"summary\":{\"successRate\":1.0,\"total\":1.0,\"slowest\":0.2,\"fastest\":0.01,\"average\":0.05,\"requestsPerSec\":50.0,\"totalData\":1,\"sizePerRequest\":1,\"sizePerSec\":1},\"responseTimeHistogram\":{},\"latencyPercentiles\":{\"p50\":0.02,\"p95\":0.08,\"p99\":0.1},\"rps\":{},\"details\":{},\"statusCodeDistribution\":{\"200\":$conc,\"503\":1},\"errorDistribution\":{\"aborted due to deadline\":$conc}}"
exit 0
"#;

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
}

#[test]
fn soak_profile_streams_one_sample_per_interval() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let soak = LoadProfile::Soak {
        sample_interval_s: 0.1,
    };
    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(Some(soak), 0.3)).expect("starts");
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
    let slow = FAKE_OHA_JSON.replacen("conc=0\n", "conc=0\nsleep 0.3\n", 1);
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
    assert!(result.thresholds_met);
    assert!(elapsed < Duration::from_secs(5), "stop took {elapsed:?}");
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

    let executor = OhaLoadExecutor::with_window_grace(Duration::from_millis(200));
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
}

#[test]
fn dropping_a_handle_without_stop_kills_the_window() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("started");
    // Records that it ran, then sleeps far longer than the test allows.
    let body = format!(
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\ntouch {}\nsleep 30\nexit 0\n",
        marker.display()
    );
    install_fake_oha(&dir, &body);

    let executor = OhaLoadExecutor::default();
    let handle = executor.start(&config(None, 5.0)).expect("starts");
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(marker.exists(), "the fake never started");
    let started = Instant::now();
    drop(handle);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "drop must kill the in-flight oha, not wait for it"
    );
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
