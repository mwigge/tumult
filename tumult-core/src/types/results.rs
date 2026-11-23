//! Per-phase measurement result types (baseline, during, post, load, analysis).

use serde::{Deserialize, Serialize};

use super::enums::{
    BaselineMethod, BaselineSource, LoadTool, NfrCriticality, NfrKind, NfrOperator, NfrState,
    SamplePhase, Trend,
};

// ── Baseline Result (Phase 1 output) ───────────────────────────

/// Aggregated baseline statistics for a single probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeBaseline {
    /// Name of the probe as declared in the experiment.
    pub name: String,
    pub mean: f64,
    pub stddev: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    /// Fraction of samples that errored (0.0-1.0).
    pub error_rate: f64,
    /// Number of samples taken.
    pub samples: u32,
}

/// Phase 1 output: steady-state baseline captured before fault injection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineResult {
    /// Epoch-nanosecond timestamp at which capture started.
    pub started_at_ns: i64,
    /// Epoch-nanosecond timestamp at which capture ended.
    pub ended_at_ns: i64,
    /// Capture window in seconds.
    pub duration_s: f64,
    /// Warmup period in seconds at the start of the window.
    pub warmup_s: f64,
    /// Total number of samples taken across probes.
    pub samples: u32,
    /// Interval in seconds between probe samples.
    pub interval_s: f64,
    pub method: BaselineMethod,
    /// Standard-deviation multiplier, when the method uses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma: Option<f64>,
    pub source: BaselineSource,
    /// Whether an anomaly was detected in the baseline window.
    pub anomaly_detected: bool,
    pub probes: Vec<ProbeBaseline>,
    /// Derived lower tolerance bound applied to during/post samples.
    pub tolerance_lower: f64,
    /// Derived upper tolerance bound applied to during/post samples.
    pub tolerance_upper: f64,
}

// ── During Result (Phase 2 output) ─────────────────────────────

/// Aggregated during-phase statistics for a single probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeDuring {
    /// Name of the probe as declared in the experiment.
    pub name: String,
    /// Number of samples taken.
    pub samples: u32,
    pub mean: f64,
    pub max: f64,
    pub min: f64,
    /// Fraction of samples that errored (0.0-1.0).
    pub error_rate: f64,
    /// Epoch-nanosecond timestamp of the first tolerance breach, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breached_at_ns: Option<i64>,
    /// Number of samples that breached tolerance.
    pub breach_count: u32,
}

/// Phase 2 output: probe samples collected while the fault was active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuringResult {
    /// Epoch-nanosecond timestamp at which the phase started.
    pub started_at_ns: i64,
    /// Epoch-nanosecond timestamp at which the phase ended.
    pub ended_at_ns: i64,
    /// How long the fault was active, in seconds.
    pub fault_active_s: f64,
    /// Interval in seconds between probe samples.
    pub sample_interval_s: f64,
    pub probes: Vec<ProbeDuring>,
}

// ── Post Result (Phase 3 output) ───────────────────────────────

/// Aggregated post-phase (recovery) statistics for a single probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbePost {
    /// Name of the probe as declared in the experiment.
    pub name: String,
    pub mean: f64,
    pub p95: f64,
    /// Fraction of samples that errored (0.0-1.0).
    pub error_rate: f64,
    /// Whether the probe returned within baseline tolerance.
    pub returned_to_baseline: bool,
    /// Seconds from method end until the probe returned within baseline tolerance.
    /// When it did not return, the seconds until its last failing sample.
    pub recovery_time_s: f64,
}

/// Phase 3 output: recovery measurement after the method completed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostResult {
    /// Epoch-nanosecond timestamp at which the phase started.
    pub started_at_ns: i64,
    /// Epoch-nanosecond timestamp at which the phase ended.
    pub ended_at_ns: i64,
    /// Phase duration in seconds.
    pub duration_s: f64,
    /// Number of samples taken.
    pub samples: u32,
    pub probes: Vec<ProbePost>,
    /// Seconds from method end until full recovery was observed.
    /// Without full recovery, the largest time of a probe: a lower bound.
    /// It does not see an outage that ended inside the method: the
    /// recovery time objective reads `mttr_s`.
    pub recovery_time_s: f64,
    /// Whether every probe returned to baseline.
    pub full_recovery: bool,
    /// Time to recovery in seconds: from the outage seen
    /// (`outage_started_at_ns`) to the recovery seen (`recovered_at_ns`).
    /// The recovery time objective and the `recovery.mttr_s` metric read
    /// it.
    ///
    /// `None` when the post phase ended without full recovery, when no
    /// sample saw an outage (a run that saw none is no evidence of
    /// recovery), when no samples were taken, or when an action of the
    /// method failed: such a run did not apply its fault as written, so it
    /// does not measure recovery from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mttr_s: Option<f64>,
    /// Whether a sample of a steady-state probe failed after the method
    /// started, in the during or the post phase: the run saw an outage.
    /// `None` in a journal written before 3.0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact_observed: Option<bool>,
    /// Epoch-nanosecond start of the first failing sample of a
    /// steady-state probe after the method started: the outage seen.
    /// `None` when the run saw no outage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outage_started_at_ns: Option<i64>,
    /// Epoch-nanosecond start of the first passing sample after the last
    /// failure, the latest over the probes that failed: the recovery seen.
    /// `None` when the run saw no outage, and when the post phase ended
    /// without full recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at_ns: Option<i64>,
}

// ── Load Result ────────────────────────────────────────────────

/// Metrics collected from the load tool that ran during the experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadResult {
    pub tool: LoadTool,
    /// Epoch-nanosecond timestamp at which the load test started.
    pub started_at_ns: i64,
    /// Epoch-nanosecond timestamp at which the load test ended.
    pub ended_at_ns: i64,
    /// Load test duration in seconds.
    pub duration_s: f64,
    /// Virtual users used by the load tool.
    pub vus: u32,
    /// Throughput in requests per second.
    pub throughput_rps: f64,
    /// Median response latency in milliseconds.
    pub latency_p50_ms: f64,
    /// 95th-percentile response latency in milliseconds.
    pub latency_p95_ms: f64,
    /// 99th-percentile response latency in milliseconds.
    pub latency_p99_ms: f64,
    /// Fraction of requests that failed (0.0-1.0).
    pub error_rate: f64,
    pub total_requests: u64,
    /// Whether all configured thresholds held.
    pub thresholds_met: bool,
    /// Time series of per-window summaries, when the adapter streams them
    /// (one per sample interval for a soak profile, one per stage
    /// otherwise). Empty for tools that only report an end-of-run summary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<LoadSample>,
    /// How the plan was carried out: windows planned and completed, an
    /// abort, and why a run fell short. Left out of the journal when there
    /// is nothing to say (a tool that only reports an end-of-run summary
    /// and ran to its end).
    #[serde(default, skip_serializing_if = "LoadExecution::is_unremarkable")]
    pub execution: LoadExecution,
}

/// How a load plan was carried out, as opposed to what it measured.
///
/// `planned_windows` and `completed_windows` are set by adapters that run
/// the plan one window at a time (oha). A plan with fewer completed than
/// planned windows was truncated: the load never ran its full shape, so
/// its thresholds count as unmet whatever the windows that did run
/// measured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadExecution {
    /// Windows the plan called for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_windows: Option<u32>,
    /// Windows that ran to their end and reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_windows: Option<u32>,
    /// Windows in which no request was answered (a blackout): the
    /// completed ones, and those counted in `lost_to_unanswered`.
    /// Recorded, never a deviation by itself.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unanswered_windows: u32,
    /// Windows that never ran because the unanswered window before them
    /// was still waiting for its requests to time out when their slot
    /// came. Part of the blackout, not a truncation of the plan.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lost_to_unanswered: u32,
    /// The load was ended by a halt or an interrupt, not by the plan or
    /// the method running out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub aborted: bool,
    /// Why the plan fell short, in plain words; empty when it did not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// What became of every request of the run, the windows that left no
    /// sample included. `None` when the tool does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<RequestCounts>,
    /// Sampled windows whose p99 fell among the requests that timed out
    /// ([`LoadSample::censored`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub censored_windows: u32,
}

/// What became of the requests of a window or a run. A request is one of
/// `ok`, `timed_out`, `transport`, `http_4xx`, `http_5xx`, `http_other`
/// or `unclassified`; `sent` is their sum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCounts {
    pub sent: u64,
    /// Answered with a 2xx or 3xx status.
    pub ok: u64,
    /// Never answered within the request timeout.
    pub timed_out: u64,
    /// Failed in transport for another reason (refused, reset, …).
    pub transport: u64,
    pub http_4xx: u64,
    pub http_5xx: u64,
    /// Answered with a status outside 200–599.
    pub http_other: u64,
    /// Failed, the tool does not say how.
    pub unclassified: u64,
}

impl RequestCounts {
    /// Requests that did not end in a 2xx or 3xx answer.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.sent.saturating_sub(self.ok)
    }

    /// Requests that got any HTTP answer.
    #[must_use]
    pub fn answered(&self) -> u64 {
        self.ok + self.http_4xx + self.http_5xx + self.http_other
    }

    /// `failed / sent`; `None` when nothing was sent.
    #[must_use]
    pub fn failure_share(&self) -> Option<f64> {
        if self.sent == 0 {
            return None;
        }
        // Request counts are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        Some(self.failed() as f64 / self.sent as f64)
    }

    /// Add the counts of `other`.
    pub fn add(&mut self, other: &Self) {
        self.sent += other.sent;
        self.ok += other.ok;
        self.timed_out += other.timed_out;
        self.transport += other.transport;
        self.http_4xx += other.http_4xx;
        self.http_5xx += other.http_5xx;
        self.http_other += other.http_other;
        self.unclassified += other.unclassified;
    }
}

// serde's `skip_serializing_if` passes a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(flag: &bool) -> bool {
    !*flag
}

impl LoadExecution {
    /// Nothing worth a line in the journal.
    #[must_use]
    pub fn is_unremarkable(&self) -> bool {
        *self == Self::default()
    }

    /// Fewer windows are accounted for — completed, or lost to a blackout
    /// — than the plan called for. `false` for a tool that does not count
    /// windows.
    #[must_use]
    pub fn truncated(&self) -> bool {
        match (self.planned_windows, self.completed_windows) {
            (Some(planned), Some(completed)) => {
                completed.saturating_add(self.lost_to_unanswered) < planned
            }
            _ => false,
        }
    }
}

/// One windowed load summary: what the tool measured between `ts_ns` and
/// `ts_ns + duration_s`. The lake stores each field as a `probe_samples`
/// series named `load.<field>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadSample {
    /// Epoch-nanosecond start of the window.
    pub ts_ns: i64,
    /// Window length in seconds.
    pub duration_s: f64,
    /// Concurrency (virtual users) during the window.
    pub vus: u32,
    pub throughput_rps: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
    /// Fraction of requests that failed (0.0-1.0).
    pub error_rate: f64,
    pub requests: u64,
    /// What became of the window's requests, when the tool says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<RequestCounts>,
    /// How long a request was waited for before it counted as timed out.
    /// The latency percentiles count such a request as taking at least
    /// this long, so one that reads this value or more is a lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<f64>,
}

impl LoadSample {
    /// Whether `latency_ms`, one of this window's percentiles, fell among
    /// the requests that timed out: the true figure is that or worse.
    #[must_use]
    pub fn censors(&self, latency_ms: f64) -> bool {
        self.request_timeout_ms
            .is_some_and(|timeout_ms| latency_ms >= timeout_ms)
    }

    /// Whether any of the window's percentiles is censored (the p99 is
    /// the first to be).
    #[must_use]
    pub fn censored(&self) -> bool {
        self.censors(self.latency_p99_ms)
    }
}

// ── Raw probe samples and objective verdicts ──────────────────

/// One raw probe observation, kept so the lake can analyse the time series
/// rather than only the per-phase aggregates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeSample {
    /// Probe name as declared in the hypothesis.
    pub probe: String,
    pub phase: SamplePhase,
    /// Epoch-nanosecond timestamp at which the sample started.
    pub ts_ns: i64,
    /// Probe execution time in milliseconds (the measured value).
    pub duration_ms: u64,
    /// Whether the sample was within tolerance.
    pub ok: bool,
}

/// Evaluation of one [`NfrObjective`](crate::types::NfrObjective) against
/// the run's measurements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NfrVerdict {
    pub nfr_id: String,
    pub kind: NfrKind,
    pub metric: String,
    pub operator: NfrOperator,
    pub threshold: f64,
    /// The measured value; `None` when the metric was not produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<f64>,
    pub state: NfrState,
    /// Signed headroom: positive when met with margin, negative when
    /// breached. For `lte` this is `threshold - observed`; for `gte` it is
    /// `observed - threshold`; `None` when inconclusive, and when the run
    /// did not recover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    pub criticality: NfrCriticality,
    /// The recovery time objective only: whether the run recovered while
    /// the post phase watched. `Some(false)`: it did not, so `observed` is
    /// the time it watched, a lower bound. `None` for any other objective,
    /// for a watch that does not count, and in an older journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered: Option<bool>,
    /// Why the verdict is what it is when its numbers do not say. The
    /// recovery time objective of a run that saw no outage is
    /// inconclusive with the reason [`NfrVerdict::NO_OUTAGE_OBSERVED`].
    /// `None` for any other verdict and in an older journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl NfrVerdict {
    /// The reason of the recovery time objective of a run whose samples
    /// saw no outage: the run is no evidence of recovery.
    pub const NO_OUTAGE_OBSERVED: &'static str = "no outage observed";
}

/// Objective outcome of a run: the declared recovery targets, what was
/// measured, and every objective verdict.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ObjectiveOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rto_target_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpo_target_s: Option<f64>,
    /// Measured recovery time (post-phase MTTR): from the outage seen to
    /// the recovery seen. `None` for a run that did not recover (its `rto`
    /// verdict holds the lower bound) and for one that saw no outage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rto_measured_s: Option<f64>,
    /// Measured data-loss window, when an `rpo` objective resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpo_measured_s: Option<f64>,
    #[serde(default)]
    pub verdicts: Vec<NfrVerdict>,
}

impl ObjectiveOutcome {
    /// Whether any blocking objective was breached.
    #[must_use]
    pub fn has_blocking_breach(&self) -> bool {
        self.verdicts
            .iter()
            .any(|v| v.state == NfrState::Breached && v.criticality == NfrCriticality::Blocking)
    }
}

// ── Analysis Result (Phase 4 output) ───────────────────────────

/// Phase 4 output: estimate-vs-actual comparison and resilience scoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// How close the Phase 0 estimate was to the observed outcome (0.0-1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_accuracy: Option<f64>,
    /// Estimated vs. measured recovery time delta in seconds. Reserved:
    /// not populated in this version (always `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_recovery_delta_s: Option<f64>,
    /// Direction of change versus previous runs. Reserved: requires
    /// historical journals; not populated in this version (always `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<Trend>,
    /// 1.0 when the run completed, 0.0 when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resilience_score: Option<f64>,
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;

    #[test]
    fn probe_baseline_round_trips() {
        let pb = ProbeBaseline {
            name: "api-latency".into(),
            mean: 45.2,
            stddev: 8.3,
            p50: 43.1,
            p95: 58.7,
            p99: 72.4,
            min: 12.0,
            max: 98.3,
            error_rate: 0.001,
            samples: 60,
        };
        let decoded: ProbeBaseline = toon_round_trip(&pb);
        assert_eq!(decoded, pb);
    }

    #[test]
    fn baseline_result_round_trips() {
        let result = BaselineResult {
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_120_000_000_000,
            duration_s: 120.0,
            warmup_s: 15.0,
            samples: 60,
            interval_s: 2.0,
            method: BaselineMethod::MeanStddev,
            sigma: Some(2.0),
            source: BaselineSource::Live,
            anomaly_detected: false,
            probes: vec![ProbeBaseline {
                name: "api-latency".into(),
                mean: 45.2,
                stddev: 8.3,
                p50: 43.1,
                p95: 58.7,
                p99: 72.4,
                min: 12.0,
                max: 98.3,
                error_rate: 0.001,
                samples: 60,
            }],
            tolerance_lower: 28.6,
            tolerance_upper: 61.8,
        };
        let decoded: BaselineResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn probe_during_round_trips() {
        let pd = ProbeDuring {
            name: "api-latency".into(),
            samples: 30,
            mean: 342.8,
            max: 1204.3,
            min: 45.0,
            error_rate: 0.12,
            breached_at_ns: Some(1_774_980_136_000_000_000),
            breach_count: 18,
        };
        let decoded: ProbeDuring = toon_round_trip(&pd);
        assert_eq!(decoded, pd);
    }

    #[test]
    fn during_result_round_trips() {
        let result = DuringResult {
            started_at_ns: 1_774_980_135_000_000_000,
            ended_at_ns: 1_774_980_165_000_000_000,
            fault_active_s: 30.0,
            sample_interval_s: 1.0,
            probes: vec![ProbeDuring {
                name: "api-latency".into(),
                samples: 30,
                mean: 342.8,
                max: 1204.3,
                min: 45.0,
                error_rate: 0.12,
                breached_at_ns: Some(1_774_980_136_000_000_000),
                breach_count: 18,
            }],
        };
        let decoded: DuringResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn probe_post_round_trips() {
        let pp = ProbePost {
            name: "api-latency".into(),
            mean: 46.1,
            p95: 59.2,
            error_rate: 0.002,
            returned_to_baseline: true,
            recovery_time_s: 12.4,
        };
        let decoded: ProbePost = toon_round_trip(&pp);
        assert_eq!(decoded, pp);
    }

    #[test]
    fn post_result_round_trips() {
        let result = PostResult {
            started_at_ns: 1_774_980_165_000_000_000,
            ended_at_ns: 1_774_980_285_000_000_000,
            duration_s: 120.0,
            samples: 60,
            probes: vec![ProbePost {
                name: "api-latency".into(),
                mean: 46.1,
                p95: 59.2,
                error_rate: 0.002,
                returned_to_baseline: true,
                recovery_time_s: 12.4,
            }],
            recovery_time_s: 12.4,
            full_recovery: true,
            mttr_s: None,
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        };
        let decoded: PostResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn load_result_round_trips() {
        let result = LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_s: 300.0,
            vus: 50,
            throughput_rps: 494.1,
            latency_p50_ms: 42.3,
            latency_p95_ms: 187.4,
            latency_p99_ms: 342.1,
            error_rate: 0.008,
            total_requests: 148_230,
            thresholds_met: true,
            samples: vec![],
            execution: LoadExecution::default(),
        };
        let decoded: LoadResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn load_execution_round_trips_and_is_left_out_when_unremarkable() {
        let mut result = LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 1,
            ended_at_ns: 2,
            duration_s: 1.0,
            vus: 5,
            throughput_rps: 10.0,
            latency_p50_ms: 1.0,
            latency_p95_ms: 2.0,
            latency_p99_ms: 3.0,
            error_rate: 0.0,
            total_requests: 10,
            thresholds_met: false,
            samples: vec![],
            execution: LoadExecution::default(),
        };
        let plain = serde_json::to_string(&result).expect("json");
        assert!(!plain.contains("execution"), "{plain}");
        let old: LoadResult = serde_json::from_str(&plain).expect("an older journal still reads");
        assert_eq!(old.execution, LoadExecution::default());

        result.execution = LoadExecution {
            planned_windows: Some(60),
            completed_windows: Some(35),
            unanswered_windows: 2,
            lost_to_unanswered: 1,
            aborted: true,
            reasons: vec!["25 of 60 windows were skipped".into()],
            requests: Some(RequestCounts {
                sent: 900,
                ok: 700,
                timed_out: 200,
                ..RequestCounts::default()
            }),
            censored_windows: 3,
        };
        assert!(result.execution.truncated());
        let decoded: LoadResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn request_counts_add_up_and_give_a_failure_share() {
        let mut counts = RequestCounts {
            sent: 100,
            ok: 60,
            timed_out: 30,
            http_5xx: 10,
            ..RequestCounts::default()
        };
        assert_eq!(counts.failed(), 40);
        assert_eq!(counts.answered(), 70);
        assert_eq!(counts.failure_share(), Some(0.4));
        assert_eq!(RequestCounts::default().failure_share(), None);
        counts.add(&RequestCounts {
            sent: 100,
            ok: 90,
            transport: 4,
            http_4xx: 5,
            unclassified: 1,
            ..RequestCounts::default()
        });
        assert_eq!(
            counts,
            RequestCounts {
                sent: 200,
                ok: 150,
                timed_out: 30,
                transport: 4,
                http_4xx: 5,
                http_5xx: 10,
                http_other: 0,
                unclassified: 1,
            }
        );
    }

    #[test]
    fn a_percentile_at_the_request_timeout_is_censored() {
        let sample = |p99: f64, timeout: Option<f64>| LoadSample {
            ts_ns: 1,
            duration_s: 10.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 25.0,
            latency_p99_ms: p99,
            error_rate: 0.0,
            requests: 400,
            counts: None,
            request_timeout_ms: timeout,
        };
        assert!(sample(2000.0, Some(2000.0)).censored());
        assert!(!sample(1999.0, Some(2000.0)).censored());
        assert!(!sample(2000.0, None).censored(), "no timeout, no claim");
        let tail = sample(2000.0, Some(2000.0));
        assert!(!tail.censors(tail.latency_p95_ms));
        // A journal written before the counts existed still reads.
        let old = r#"{"ts_ns":1,"duration_s":10.0,"vus":4,"throughput_rps":40.0,
            "latency_p50_ms":20.0,"latency_p95_ms":25.0,"latency_p99_ms":30.0,
            "error_rate":0.0,"requests":400}"#;
        let read: LoadSample = serde_json::from_str(old).expect("an older sample reads");
        assert_eq!((read.counts, read.request_timeout_ms), (None, None));
    }

    #[test]
    fn a_tool_that_counts_no_windows_is_never_truncated() {
        assert!(!LoadExecution::default().truncated());
        let full = LoadExecution {
            planned_windows: Some(3),
            completed_windows: Some(3),
            ..LoadExecution::default()
        };
        assert!(!full.truncated());
    }

    #[test]
    fn analysis_result_round_trips() {
        let result = AnalysisResult {
            estimate_accuracy: Some(0.83),
            estimate_recovery_delta_s: Some(-2.6),
            trend: Some(Trend::Improving),
            resilience_score: Some(0.92),
        };
        let decoded: AnalysisResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn an_rto_verdict_says_the_run_did_not_recover_and_an_older_one_still_reads() {
        let older = NfrVerdict {
            nfr_id: "rto".into(),
            kind: NfrKind::Rto,
            metric: "recovery.mttr_s".into(),
            operator: NfrOperator::Lte,
            threshold: 2.0,
            observed: None,
            state: NfrState::Inconclusive,
            margin: None,
            criticality: NfrCriticality::Blocking,
            recovered: None,
            reason: None,
        };
        let written = toon_format::encode_default(&older).expect("encode");
        assert!(!written.contains("recovered"), "{written}");
        let read: NfrVerdict = toon_format::decode_default(&written).expect("decode");
        assert_eq!(read, older);

        let unrecovered = NfrVerdict {
            observed: Some(29.1),
            state: NfrState::Breached,
            recovered: Some(false),
            ..older
        };
        let decoded: NfrVerdict = toon_round_trip(&unrecovered);
        assert_eq!(decoded, unrecovered);
    }

    #[test]
    fn a_post_result_keeps_the_outage_it_saw_and_an_older_one_still_reads() {
        let older = PostResult {
            started_at_ns: 1_774_980_165_000_000_000,
            ended_at_ns: 1_774_980_170_000_000_000,
            duration_s: 5.0,
            samples: 5,
            probes: vec![],
            recovery_time_s: 4.0,
            full_recovery: true,
            mttr_s: Some(4.0),
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        };
        let written = toon_format::encode_default(&older).expect("encode");
        for key in ["impact_observed", "outage_started_at_ns", "recovered_at_ns"] {
            assert!(!written.contains(key), "{written}");
        }
        let read: PostResult = toon_format::decode_default(&written).expect("decode");
        assert_eq!(read, older);

        let timed = PostResult {
            mttr_s: Some(7.5),
            impact_observed: Some(true),
            outage_started_at_ns: Some(1_774_980_161_500_000_000),
            recovered_at_ns: Some(1_774_980_169_000_000_000),
            ..older.clone()
        };
        assert_eq!(toon_round_trip(&timed), timed);

        let unaffected = PostResult {
            mttr_s: None,
            impact_observed: Some(false),
            ..older
        };
        let written = toon_format::encode_default(&unaffected).expect("encode");
        assert!(written.contains("impact_observed: false"), "{written}");
        assert_eq!(toon_round_trip(&unaffected), unaffected);
    }

    #[test]
    fn an_rto_verdict_says_why_it_is_inconclusive_and_an_older_one_still_reads() {
        let older = NfrVerdict {
            nfr_id: "rto".into(),
            kind: NfrKind::Rto,
            metric: "recovery.mttr_s".into(),
            operator: NfrOperator::Lte,
            threshold: 2.0,
            observed: None,
            state: NfrState::Inconclusive,
            margin: None,
            criticality: NfrCriticality::Blocking,
            recovered: None,
            reason: None,
        };
        let written = toon_format::encode_default(&older).expect("encode");
        assert!(!written.contains("reason"), "{written}");
        let read: NfrVerdict = toon_format::decode_default(&written).expect("decode");
        assert_eq!(read, older);

        let unaffected = NfrVerdict {
            reason: Some(NfrVerdict::NO_OUTAGE_OBSERVED.into()),
            ..older
        };
        assert_eq!(toon_round_trip(&unaffected), unaffected);
    }
}
