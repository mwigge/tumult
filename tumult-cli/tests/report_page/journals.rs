//! The journals the report tests build: a run that did nothing, and the
//! parts a test adds to it.

use tumult_core::journal::write_journal;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, DuringResult, ExperimentStatus, Journal,
    LoadExecution, LoadResult, LoadSample, LoadTool, NfrCriticality, NfrKind, NfrOperator,
    NfrState, NfrVerdict, PostResult, ProbeDuring, ProbePost, SpanId, TestKind, TraceId,
};

use super::report;
use crate::lab::Lab;

/// 2026-11-16 22:15:19 UTC.
pub const STARTED: i64 = 1_794_867_319_000_000_000;
pub const MILLISECOND: i64 = 1_000_000;

/// The journal of a run of a minute that did nothing.
pub fn journal() -> Journal {
    Journal {
        experiment_title: "A run that was written down".into(),
        experiment_id: "run-1".into(),
        status: ExperimentStatus::Completed,
        started_at_ns: STARTED,
        ended_at_ns: STARTED + 60_000 * MILLISECOND,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![],
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: None,
        halt: None,
        blast_radius: None,
        kind: TestKind::Chaos,
        objectives: None,
        probe_samples: vec![],
        target_version: None,
        target: None,
    }
}

/// A step that began `after_ms` into the run and left no trace and no
/// words.
pub fn step(name: &str, status: ActivityStatus, after_ms: i64, duration_ms: u64) -> ActivityResult {
    ActivityResult {
        name: name.into(),
        activity_type: ActivityType::Action,
        status,
        started_at_ns: STARTED + after_ms * MILLISECOND,
        duration_ms,
        output: None,
        error: None,
        trace_id: TraceId(String::new()),
        span_id: SpanId::empty(),
    }
}

/// The report the binary writes for `journal`, which it reads from a file.
pub fn report_for(journal: &Journal) -> String {
    let lab = Lab::with_state("ok");
    write_journal(journal, &lab.path("journal.toon")).expect("write the journal");
    report(&lab)
}

/// A verdict on a latency objective that observed nothing.
pub fn verdict(id: &str, operator: NfrOperator, threshold: f64, state: NfrState) -> NfrVerdict {
    NfrVerdict {
        nfr_id: id.into(),
        kind: NfrKind::Latency,
        metric: "load.latency_p99_ms".into(),
        operator,
        threshold,
        observed: None,
        state,
        margin: None,
        criticality: NfrCriticality::Major,
    }
}

/// A window of ten seconds that began `after_ms` into the run.
pub fn window(after_ms: i64, p50: f64, p95: f64, p99: f64) -> LoadSample {
    LoadSample {
        ts_ns: STARTED + after_ms * MILLISECOND,
        duration_s: 10.0,
        vus: 4,
        throughput_rps: 12.5,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        error_rate: 0.25,
        requests: 125,
        counts: None,
        request_timeout_ms: Some(1000.0),
    }
}

/// A load of a minute as its tool summed it up.
pub fn load(execution: LoadExecution, samples: Vec<LoadSample>) -> LoadResult {
    LoadResult {
        tool: LoadTool::K6,
        started_at_ns: STARTED + 2000 * MILLISECOND,
        ended_at_ns: STARTED + 62_000 * MILLISECOND,
        duration_s: 60.0,
        vus: 4,
        throughput_rps: 12.5,
        latency_p50_ms: 40.0,
        latency_p95_ms: 900.26,
        latency_p99_ms: 1000.0,
        error_rate: 0.4,
        total_requests: 750,
        thresholds_met: false,
        samples,
        execution,
    }
}

/// What two probes measured while a fault of 12.5 s was active.
pub fn during() -> DuringResult {
    let fault_started = STARTED + 2000 * MILLISECOND;
    DuringResult {
        started_at_ns: fault_started,
        ended_at_ns: fault_started + 12_500 * MILLISECOND,
        fault_active_s: 12.5,
        sample_interval_s: 0.5,
        probes: vec![
            ProbeDuring {
                name: "api".into(),
                samples: 25,
                mean: 12.34,
                max: 80.0,
                min: 2.0,
                error_rate: 0.2,
                breached_at_ns: Some(fault_started + 1500 * MILLISECOND),
                breach_count: 5,
            },
            ProbeDuring {
                name: "db".into(),
                samples: 24,
                mean: 3.0,
                max: 4.0,
                min: 1.0,
                error_rate: 0.0,
                breached_at_ns: None,
                breach_count: 0,
            },
        ],
        degradation_onset_s: None,
        degradation_peak_s: None,
        degradation_magnitude: None,
        graceful_degradation: None,
    }
}

/// What two probes measured in the 30 s after the fault.
pub fn post() -> PostResult {
    PostResult {
        started_at_ns: STARTED + 15_000 * MILLISECOND,
        ended_at_ns: STARTED + 45_000 * MILLISECOND,
        duration_s: 30.0,
        samples: 30,
        probes: vec![
            ProbePost {
                name: "api".into(),
                mean: 10.0,
                p95: 20.5,
                error_rate: 0.1,
                returned_to_baseline: true,
                recovery_time_s: 4.2,
            },
            ProbePost {
                name: "db".into(),
                mean: 3.0,
                p95: 4.0,
                error_rate: 1.0,
                returned_to_baseline: false,
                recovery_time_s: 30.0,
            },
        ],
        recovery_time_s: 30.0,
        full_recovery: false,
        residual_degradation: None,
        data_integrity_verified: None,
        data_loss_detected: None,
        mttr_s: None,
    }
}
