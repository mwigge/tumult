//! The recovery clock: from the first failing sample of a steady-state
//! probe after the method started to the first passing sample after the
//! last failure. The samples here are written by hand, so each test names
//! the second each one was taken.

use super::*;
use crate::runner::phases::build_post_result;

/// The epoch second the method of these runs started.
const METHOD_STARTED_NS: i64 = 1_800_000_000_000_000_000;

fn at(second: f64) -> i64 {
    // Whole tenths of a second: the cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    let tenths = (second * 10.0).round() as i64;
    METHOD_STARTED_NS + tenths * 100_000_000
}

fn sample(probe: &str, phase: SamplePhase, second: f64, ok: bool) -> ProbeSample {
    ProbeSample {
        probe: probe.into(),
        phase,
        ts_ns: at(second),
        duration_ms: 5,
        ok,
        reading: None,
    }
}

fn during(probe: &str, second: f64, ok: bool) -> ProbeSample {
    sample(probe, SamplePhase::During, second, ok)
}

fn post(probe: &str, second: f64, ok: bool) -> ProbeSample {
    sample(probe, SamplePhase::Post, second, ok)
}

/// The post result of a run whose samples are `samples` and whose post
/// phase started at `post_started` seconds, as the runner builds it.
fn post_result(post_started: f64, samples: &[ProbeSample]) -> PostResult {
    let mut grouped: IndexMap<String, Vec<ActivityResult>> = IndexMap::new();
    for s in samples.iter().filter(|s| s.phase == SamplePhase::Post) {
        grouped
            .entry(s.probe.clone())
            .or_default()
            .push(ActivityResult {
                name: s.probe.clone(),
                activity_type: ActivityType::Probe,
                status: if s.ok {
                    ActivityStatus::Succeeded
                } else {
                    ActivityStatus::Failed
                },
                started_at_ns: s.ts_ns,
                duration_ms: s.duration_ms,
                output: None,
                error: None,
                trace_id: TraceId::default(),
                span_id: SpanId::default(),
            });
    }
    let grouped: Vec<(String, Vec<ActivityResult>)> = grouped.into_iter().collect();
    let ended = samples.iter().map(|s| s.ts_ns).max().unwrap_or(0) + 5_000_000;
    build_post_result(at(post_started), ended, &grouped, samples).expect("a post phase")
}

fn close(actual: Option<f64>, expected: f64) -> bool {
    actual.is_some_and(|actual| (actual - expected).abs() < 1e-6)
}

#[test]
fn an_outage_that_ends_inside_the_method_is_timed_from_its_first_failing_sample() {
    let samples = [
        during("up", 0.0, true),
        during("up", 1.0, false),
        during("up", 2.0, false),
        during("up", 3.0, false),
        during("up", 3.9, true),
        post("up", 4.0, true),
    ];

    let result = post_result(4.0, &samples);

    assert!(result.full_recovery);
    assert_eq!(result.impact_observed, Some(true));
    assert_eq!(result.outage_started_at_ns, Some(at(1.0)));
    assert_eq!(result.recovered_at_ns, Some(at(3.9)));
    assert!(close(result.mttr_s, 2.9), "{result:?}");
    // The time from the end of the method keeps its meaning.
    assert!(close(Some(result.recovery_time_s), 0.0), "{result:?}");
}

#[test]
fn an_outage_that_outlives_the_method_is_timed_from_its_first_failing_sample() {
    let samples = [
        during("up", 0.0, true),
        during("up", 1.0, false),
        post("up", 1.5, false),
        post("up", 2.5, false),
        post("up", 3.5, true),
    ];

    let result = post_result(1.5, &samples);

    assert_eq!(result.impact_observed, Some(true));
    assert_eq!(result.outage_started_at_ns, Some(at(1.0)));
    assert_eq!(result.recovered_at_ns, Some(at(3.5)));
    assert!(close(result.mttr_s, 2.5), "{result:?}");
    assert!(close(Some(result.recovery_time_s), 2.0), "{result:?}");
}

#[test]
fn an_outage_first_seen_after_the_method_is_timed_from_that_sample() {
    let samples = [
        during("up", 0.0, true),
        post("up", 0.2, false),
        post("up", 1.2, false),
        post("up", 2.2, true),
    ];

    let result = post_result(0.2, &samples);

    assert_eq!(result.outage_started_at_ns, Some(at(0.2)));
    assert!(close(result.mttr_s, 2.0), "{result:?}");
    assert!(close(Some(result.recovery_time_s), 2.0), "{result:?}");
}

#[test]
fn the_clock_stops_when_the_last_probe_that_failed_is_back() {
    let samples = [
        during("api", 1.0, false),
        during("api", 2.0, true),
        during("db", 1.0, true),
        during("db", 2.0, true),
        post("api", 3.0, true),
        post("api", 4.0, true),
        post("api", 5.0, true),
        post("db", 3.0, false),
        post("db", 4.0, false),
        post("db", 5.0, true),
    ];

    let result = post_result(3.0, &samples);

    assert_eq!(result.outage_started_at_ns, Some(at(1.0)));
    assert_eq!(result.recovered_at_ns, Some(at(5.0)));
    assert!(close(result.mttr_s, 4.0), "{result:?}");
}

#[test]
fn a_probe_that_fails_again_after_a_pass_keeps_the_clock_running() {
    let samples = [
        during("up", 1.0, false),
        during("up", 2.0, true),
        during("up", 3.0, false),
        during("up", 3.5, true),
        post("up", 4.0, true),
    ];

    let result = post_result(4.0, &samples);

    assert_eq!(result.outage_started_at_ns, Some(at(1.0)));
    assert_eq!(result.recovered_at_ns, Some(at(3.5)));
    assert!(close(result.mttr_s, 2.5), "{result:?}");
}

#[test]
fn a_run_whose_samples_never_failed_saw_no_outage_and_measures_no_recovery() {
    let samples = [
        during("up", 0.0, true),
        during("up", 1.0, true),
        post("up", 1.5, true),
    ];

    let result = post_result(1.5, &samples);

    assert!(result.full_recovery);
    assert_eq!(result.impact_observed, Some(false));
    assert_eq!(result.outage_started_at_ns, None);
    assert_eq!(result.recovered_at_ns, None);
    assert_eq!(result.mttr_s, None);
}

#[test]
fn a_run_that_did_not_recover_saw_an_outage_and_no_recovery() {
    let samples = [
        during("up", 1.0, false),
        post("up", 2.0, false),
        post("up", 3.0, false),
    ];

    let result = post_result(2.0, &samples);

    assert!(!result.full_recovery);
    assert_eq!(result.impact_observed, Some(true));
    assert_eq!(result.outage_started_at_ns, Some(at(1.0)));
    assert_eq!(result.recovered_at_ns, None);
    assert_eq!(result.mttr_s, None);
    assert!(close(Some(result.recovery_time_s), 1.0), "{result:?}");
}
