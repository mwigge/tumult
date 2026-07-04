//! Probe-sampling cadence tests: during-phase interleaving with the method,
//! post-phase recovery loop, timeout handling, and sampler panic tolerance.

use super::*;
use crate::controls::ControlRegistry;
use crate::runner::phases::{build_post_result, collect_post_samples};
use std::sync::atomic::AtomicI64;
use std::sync::Mutex;
use std::time::Duration;

// -- Tests: during-phase samples interleave with method execution

/// Method actions sleep and record their execution window; probes record
/// their execution timestamps. Lets the test assert that during-phase
/// samples were taken *while* the method was running.
struct InterleavingExecutor {
    method_started: AtomicI64,
    method_ended: AtomicI64,
    probe_times: Mutex<Vec<i64>>,
}

impl InterleavingExecutor {
    fn new() -> Self {
        Self {
            method_started: AtomicI64::new(0),
            method_ended: AtomicI64::new(0),
            probe_times: Mutex::new(Vec::new()),
        }
    }
}

impl ActivityExecutor for InterleavingExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match activity.activity_type {
            ActivityType::Action => {
                self.method_started
                    .store(epoch_nanos_now(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(300));
                self.method_ended.store(epoch_nanos_now(), Ordering::SeqCst);
                ActivityOutcome {
                    success: true,
                    output: None,
                    error: None,
                    duration_ms: 300,
                }
            }
            ActivityType::Probe => {
                self.probe_times.lock().unwrap().push(epoch_nanos_now());
                ActivityOutcome {
                    success: true,
                    output: Some("200".into()),
                    error: None,
                    duration_ms: 1,
                }
            }
        }
    }
}

#[test]
fn during_samples_interleave_with_method_execution() {
    let exp = experiment_with_hypothesis();
    let executor_impl = Arc::new(InterleavingExecutor::new());
    let executor: Arc<dyn ActivityExecutor> = executor_impl.clone();
    let controls = Arc::new(ControlRegistry::new());

    let sampling = SamplingConfig {
        interval: Duration::from_millis(20),
        max_during_samples: 100,
        recovery_timeout: Duration::from_millis(200),
    };

    let journal =
        run_experiment_with_sampling(&exp, &executor, &controls, &default_config(), &sampling)
            .unwrap();

    let method_start = executor_impl.method_started.load(Ordering::SeqCst);
    let method_end = executor_impl.method_ended.load(Ordering::SeqCst);
    assert!(method_start > 0, "method action should have run");
    assert!(method_end > method_start);

    let probe_times = executor_impl.probe_times.lock().unwrap();
    let during_count = probe_times
        .iter()
        .filter(|&&t| t > method_start && t < method_end)
        .count();
    assert!(
        during_count >= 2,
        "expected at least 2 probe samples taken while the 300ms method ran \
         (20ms interval), got {during_count}"
    );
    let after_count = probe_times.iter().filter(|&&t| t >= method_end).count();
    assert!(
        after_count >= 1,
        "post-phase / hypothesis-after probes should run after the method"
    );

    let during = journal
        .during_result
        .expect("during_result should be present");
    assert!(
        during.probes[0].samples >= 2,
        "samples should be spread over the method duration, got {}",
        during.probes[0].samples
    );
    assert!(
        (during.sample_interval_s - 0.02).abs() < 1e-9,
        "journal must record the actual sampling interval, got {}",
        during.sample_interval_s
    );

    let post = journal.post_result.expect("post_result should be present");
    assert!(post.full_recovery, "healthy probes recover immediately");
}

// -- Tests: post-phase recovery loop (unit level, via phases internals)

/// Probe output is "500" for the first `failures_before_recovery` calls,
/// then "200" (which passes the `test_probe` tolerance of exactly 200).
struct FlakyProbeExecutor {
    calls: AtomicUsize,
    failures_before_recovery: usize,
}

impl ActivityExecutor for FlakyProbeExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let output = if n < self.failures_before_recovery {
            "500"
        } else {
            "200"
        };
        ActivityOutcome {
            success: true,
            output: Some(output.into()),
            error: None,
            duration_ms: 1,
        }
    }
}

#[test]
fn post_phase_samples_until_probe_recovers() {
    let hypothesis = Hypothesis {
        title: "recovers after two failures".into(),
        probes: vec![test_probe("health-check")],
    };
    let executor = FlakyProbeExecutor {
        calls: AtomicUsize::new(0),
        failures_before_recovery: 2,
    };

    let started = std::time::Instant::now();
    let started_at_ns = epoch_nanos_now();
    let samples = collect_post_samples(
        &hypothesis,
        &executor,
        Duration::from_millis(10),
        Duration::from_secs(5),
        None,
    );
    let ended_at_ns = epoch_nanos_now();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "loop must stop on recovery, not run to the timeout"
    );
    assert_eq!(samples.len(), 1, "one probe");
    let (_, probe_samples) = &samples[0];
    assert_eq!(
        probe_samples.len(),
        3,
        "two failing rounds plus the recovering round"
    );

    let post = build_post_result(started_at_ns, ended_at_ns, &samples).unwrap();
    assert!(post.full_recovery);
    assert!(post.probes[0].returned_to_baseline);
    assert!(
        post.recovery_time_s > 0.0,
        "recovery after two failed rounds takes at least one interval, got {}",
        post.recovery_time_s
    );
    assert!(post.recovery_time_s <= post.duration_s);
    assert_eq!(post.mttr_s, Some(post.recovery_time_s));
}

#[test]
fn post_phase_stops_at_timeout_when_probe_never_recovers() {
    let hypothesis = Hypothesis {
        title: "never recovers".into(),
        probes: vec![test_probe("health-check")],
    };
    let executor = MockExecutor::with_output("500");

    let started = std::time::Instant::now();
    let started_at_ns = epoch_nanos_now();
    let samples = collect_post_samples(
        &hypothesis,
        &executor,
        Duration::from_millis(10),
        Duration::from_millis(60),
        None,
    );
    let ended_at_ns = epoch_nanos_now();

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "loop must stop near the 60ms timeout"
    );
    let (_, probe_samples) = &samples[0];
    assert!(
        probe_samples.len() >= 2,
        "should sample repeatedly before timing out, got {}",
        probe_samples.len()
    );

    let post = build_post_result(started_at_ns, ended_at_ns, &samples).unwrap();
    assert!(!post.full_recovery);
    assert!(!post.probes[0].returned_to_baseline);
    assert_eq!(
        post.mttr_s, None,
        "MTTR is unknown when recovery never happened"
    );
}

// -- Tests: sampler-thread panic tolerance

/// Panics on probe execution from any thread except the one it was created
/// on: the hypothesis-before/after and post-phase probes (main thread)
/// succeed, while the during-phase sampler thread panics.
struct PanicOffMainProbeExecutor {
    main_thread: std::thread::ThreadId,
}

impl ActivityExecutor for PanicOffMainProbeExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        assert!(
            !(activity.activity_type == ActivityType::Probe
                && std::thread::current().id() != self.main_thread),
            "sampler thread probe panic (intentional test panic)"
        );
        ActivityOutcome {
            success: true,
            output: Some("200".into()),
            error: None,
            duration_ms: 1,
        }
    }
}

#[test]
fn sampler_thread_panic_does_not_fail_the_run() {
    let exp = experiment_with_hypothesis();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(PanicOffMainProbeExecutor {
        main_thread: std::thread::current().id(),
    });
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &default_config(),
        &fast_sampling(),
    )
    .expect("runner must not propagate a sampler-thread panic");

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(
        journal.during_result.is_none(),
        "the sampler panicked before collecting any samples"
    );
    assert!(
        journal.post_result.is_some(),
        "post phase runs on the main thread and should still be collected"
    );
}

// -- Tests: no probes → no sampling loops

#[test]
fn empty_probe_list_skips_sampling_phases() {
    let mut exp = minimal_experiment();
    exp.steady_state_hypothesis = Some(Hypothesis {
        title: "no probes".into(),
        probes: vec![],
    });
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());

    let started = std::time::Instant::now();
    // Default sampling config: proves the 30s recovery timeout is skipped
    // entirely when there is nothing to sample.
    let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

    assert!(journal.during_result.is_none());
    assert!(journal.post_result.is_none());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "no-probe experiments must not wait on sampling loops"
    );
}
