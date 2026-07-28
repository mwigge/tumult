//! Pure generation of synthetic chaos experiments as OTLP export requests,
//! matching tumult's span vocabulary and `resilience.*` metadata standard.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::number_data_point;
use opentelemetry_proto::tonic::metrics::v1::{
    metric, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
    Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};

/// Nanoseconds per second / day.
const NS_PER_S: u64 = 1_000_000_000;
const NS_PER_DAY: u64 = 86_400 * NS_PER_S;
/// Experiments are spread over the past this many days.
const SPREAD_DAYS: u64 = 14;

/// Demo targets: (resilience.target.system, resilience.target.technology).
const TARGETS: &[(&str, &str)] = &[
    ("database", "postgresql"),
    ("cache", "redis"),
    ("message-broker", "kafka"),
    ("api", "nginx"),
    ("container", "docker"),
];

/// Demo faults: (resilience.fault.type, subtype, plugin).
const FAULTS: &[(&str, &str, &str)] = &[
    ("network", "latency-injection", "tumult-net"),
    ("termination", "process-kill", "tumult-ssh"),
    ("resource-stress", "cpu-stress", "tumult-ssh"),
];

const ENVS: &[&str] = &["staging", "production"];

/// Deterministic xorshift64* RNG (no external rand dep; reproducible demos).
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // Avoid the degenerate all-zero state.
        Self(seed | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }

    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    fn bytes<const N: usize>(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(N);
        while out.len() < N {
            out.extend_from_slice(&self.next().to_le_bytes());
        }
        out.truncate(N);
        out
    }
}

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        ..KeyValue::default()
    }
}

fn ks(key: &str, value: impl Into<String>) -> KeyValue {
    kv(key, Value::StringValue(value.into()))
}

fn kf(key: &str, value: f64) -> KeyValue {
    kv(key, Value::DoubleValue(value))
}

fn kb(key: &str, value: bool) -> KeyValue {
    kv(key, Value::BoolValue(value))
}

fn tumult_resource() -> Resource {
    Resource {
        attributes: vec![
            ks("service.name", "tumult"),
            ks("service.version", "2.18.0"),
        ],
        ..Resource::default()
    }
}

fn uuid_v4(rng: &mut XorShift) -> String {
    let mut b = rng.bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

/// Everything needed to build one span (keeps `make_span` arity sane).
struct SpanSpec {
    trace_id: Vec<u8>,
    span_id: Vec<u8>,
    parent_span_id: Vec<u8>,
    name: String,
    start_ns: u64,
    duration_ns: u64,
    status_code: StatusCode,
    attributes: Vec<KeyValue>,
}

fn make_span(spec: SpanSpec) -> Span {
    Span {
        trace_id: spec.trace_id,
        span_id: spec.span_id,
        parent_span_id: spec.parent_span_id,
        name: spec.name,
        kind: SpanKind::Internal as i32,
        start_time_unix_nano: spec.start_ns,
        end_time_unix_nano: spec.start_ns + spec.duration_ns,
        attributes: spec.attributes,
        status: Some(Status {
            code: spec.status_code as i32,
            message: String::new(),
        }),
        ..Span::default()
    }
}

/// One batch of synthetic telemetry: traces + metrics + logs for `n`
/// experiments ending near `now_ns`.
pub struct DemoData {
    pub traces: ExportTraceServiceRequest,
    pub metrics: ExportMetricsServiceRequest,
    pub logs: ExportLogsServiceRequest,
    pub experiments: usize,
}

/// Generate `n` synthetic experiments spread over the past 14 days (when
/// `spread` is true; `--loop` batches use `spread = false` so they land at
/// `now`). Deterministic for a given `rng` state.
pub fn generate(rng: &mut XorShift, n: usize, now_ns: u64, spread: bool) -> DemoData {
    let mut spans = Vec::new();
    let mut sum_points = Vec::new();
    let mut deviation_points = Vec::new();
    let mut histogram_points = Vec::new();
    let mut log_records = Vec::new();

    for idx in 0..n {
        let (target_system, target_technology) =
            TARGETS[rng.range(0, TARGETS.len() as u64) as usize];
        let (fault_type, fault_subtype, plugin) =
            FAULTS[rng.range(0, FAULTS.len() as u64) as usize];
        let environment = ENVS[rng.range(0, ENVS.len() as u64) as usize];
        let hypothesis_met = rng.chance(0.75);
        let outcome_status = if hypothesis_met {
            "completed"
        } else {
            "deviated"
        };
        let recovery_time_s = if hypothesis_met {
            5.0 + rng.f64() * 55.0
        } else {
            60.0 + rng.f64() * 240.0
        };
        let with_rollback = rng.chance(0.20);

        let experiment_id = uuid_v4(rng);
        let experiment_name = format!("{target_technology}-{fault_subtype}-{:03}", idx + 1);

        // Timeline: uniform over the past SPREAD_DAYS days + jitter, or "now".
        let offset_ns = if spread {
            (now_ns.saturating_sub(SPREAD_DAYS * NS_PER_DAY))
                + (SPREAD_DAYS * NS_PER_DAY) * (idx as u64) / (n as u64)
                + rng.range(0, NS_PER_DAY / 2)
        } else {
            now_ns.saturating_sub(rng.range(60, 3600) * NS_PER_S)
        };
        let total_duration_ns = rng.range(60, 300) * NS_PER_S;
        let action_duration_ns = rng.range(1, 30) * NS_PER_S;
        let before_duration_ns = rng.range(5, 10) * NS_PER_S;
        let probe_duration_ns = rng.range(1, 5) * NS_PER_S;
        let after_duration_ns = rng.range(5, 10) * NS_PER_S;

        let action_failed = !hypothesis_met && rng.chance(0.5);
        let action_status = if action_failed {
            StatusCode::Error
        } else {
            StatusCode::Ok
        };
        let probe_status = if hypothesis_met {
            StatusCode::Ok
        } else {
            StatusCode::Error
        };

        let trace_id = rng.bytes::<16>();
        let root_id = rng.bytes::<8>();
        let before_id = rng.bytes::<8>();
        let action_id = rng.bytes::<8>();
        let probe_id = rng.bytes::<8>();
        let after_id = rng.bytes::<8>();

        let target_attrs = || {
            vec![
                ks("resilience.target.system", target_system),
                ks("resilience.target.technology", target_technology),
                ks("resilience.target.environment", environment),
            ]
        };
        let fault_attrs = || {
            vec![
                ks("resilience.fault.type", fault_type),
                ks("resilience.fault.subtype", fault_subtype),
                ks("resilience.fault.severity", "major"),
                ks("resilience.fault.blast_radius", "single-instance"),
                ks("resilience.fault.plugin", plugin),
            ]
        };

        // Root: resilience.experiment
        let mut root_attrs = vec![
            ks("resilience.experiment.id", experiment_id.clone()),
            ks("resilience.experiment.name", experiment_name.clone()),
            ks("resilience.outcome.status", outcome_status),
            kb("resilience.outcome.hypothesis_met", hypothesis_met),
            kf("resilience.outcome.recovery_time_s", recovery_time_s),
        ];
        root_attrs.extend(target_attrs());
        root_attrs.extend(fault_attrs());
        spans.push(make_span(SpanSpec {
            trace_id: trace_id.clone(),
            span_id: root_id.clone(),
            parent_span_id: vec![],
            name: "resilience.experiment".into(),
            start_ns: offset_ns,
            duration_ns: total_duration_ns,
            status_code: StatusCode::Ok,
            attributes: root_attrs,
        }));

        // resilience.hypothesis.before
        spans.push(make_span(SpanSpec {
            trace_id: trace_id.clone(),
            span_id: before_id,
            parent_span_id: root_id.clone(),
            name: "resilience.hypothesis.before".into(),
            start_ns: offset_ns + NS_PER_S,
            duration_ns: before_duration_ns,
            status_code: StatusCode::Ok,
            attributes: target_attrs(),
        }));

        // resilience.action
        let action_start = offset_ns + before_duration_ns + 2 * NS_PER_S;
        let mut action_attrs = target_attrs();
        action_attrs.extend(fault_attrs());
        action_attrs.push(ks("resilience.fault.action", fault_subtype));
        spans.push(make_span(SpanSpec {
            trace_id: trace_id.clone(),
            span_id: action_id.clone(),
            parent_span_id: root_id.clone(),
            name: "resilience.action".into(),
            start_ns: action_start,
            duration_ns: action_duration_ns,
            status_code: action_status,
            attributes: action_attrs,
        }));

        // resilience.probe
        let probe_start = action_start + action_duration_ns + NS_PER_S;
        spans.push(make_span(SpanSpec {
            trace_id: trace_id.clone(),
            span_id: probe_id.clone(),
            parent_span_id: root_id.clone(),
            name: "resilience.probe".into(),
            start_ns: probe_start,
            duration_ns: probe_duration_ns,
            status_code: probe_status,
            attributes: target_attrs(),
        }));

        // resilience.hypothesis.after
        let after_start = probe_start + probe_duration_ns + NS_PER_S;
        spans.push(make_span(SpanSpec {
            trace_id: trace_id.clone(),
            span_id: after_id,
            parent_span_id: root_id.clone(),
            name: "resilience.hypothesis.after".into(),
            start_ns: after_start,
            duration_ns: after_duration_ns,
            status_code: StatusCode::Ok,
            attributes: target_attrs(),
        }));

        // resilience.rollback (on ~20% of experiments)
        if with_rollback {
            let rollback_start = after_start + after_duration_ns + NS_PER_S;
            spans.push(make_span(SpanSpec {
                trace_id: trace_id.clone(),
                span_id: rng.bytes::<8>(),
                parent_span_id: root_id.clone(),
                name: "resilience.rollback".into(),
                start_ns: rollback_start,
                duration_ns: rng.range(1, 10) * NS_PER_S,
                status_code: StatusCode::Ok,
                attributes: target_attrs(),
            }));
        }

        // Metrics. The counter dims mirror real tumult: experiments.total is
        // tagged resilience.outcome.status = success|failure (a run only
        // counts as success when the hypothesis held), deviations.total is
        // tagged with the experiment name only.
        let metric_outcome = if hypothesis_met { "success" } else { "failure" };
        let end_ns = offset_ns + total_duration_ns;
        sum_points.push(NumberDataPoint {
            attributes: vec![
                ks("resilience.experiment.name", experiment_name.clone()),
                ks("resilience.outcome.status", metric_outcome),
                ks("resilience.target.system", target_system),
            ],
            time_unix_nano: end_ns,
            value: Some(number_data_point::Value::AsInt(1)),
            ..NumberDataPoint::default()
        });
        if !hypothesis_met {
            deviation_points.push(NumberDataPoint {
                attributes: vec![ks("resilience.experiment.name", experiment_name.clone())],
                time_unix_nano: end_ns,
                value: Some(number_data_point::Value::AsInt(1)),
                ..NumberDataPoint::default()
            });
        }
        let activity_count = if with_rollback { 5 } else { 4 };
        histogram_points.push(HistogramDataPoint {
            attributes: vec![
                ks("resilience.target.system", target_system),
                ks("resilience.fault.type", fault_type),
            ],
            start_time_unix_nano: offset_ns,
            time_unix_nano: end_ns,
            count: activity_count,
            sum: Some(total_duration_ns as f64 / NS_PER_S as f64),
            bucket_counts: vec![1, 2, 1, activity_count - 4, 0],
            explicit_bounds: vec![30.0, 60.0, 120.0, 300.0],
            min: Some(probe_duration_ns as f64 / NS_PER_S as f64),
            max: Some(total_duration_ns as f64 / NS_PER_S as f64),
            ..HistogramDataPoint::default()
        });

        // Correlated logs.
        let mut log = |severity: &str, body: String, span_id: &[u8]| {
            log_records.push(LogRecord {
                time_unix_nano: offset_ns + (log_records.len() as u64 % 50) * NS_PER_S,
                severity_text: severity.into(),
                body: Some(AnyValue {
                    value: Some(Value::StringValue(body)),
                }),
                trace_id: trace_id.clone(),
                span_id: span_id.to_vec(),
                attributes: vec![ks("resilience.experiment.id", experiment_id.clone())],
                ..LogRecord::default()
            });
        };
        log("INFO", "steady state verified".into(), &root_id);
        log(
            "INFO",
            format!("fault injected: {fault_type}/{fault_subtype}"),
            &action_id,
        );
        if !hypothesis_met {
            log(
                "ERROR",
                "steady state violated: probe breached baseline threshold".into(),
                &probe_id,
            );
        }
        if with_rollback {
            log("WARN", "rollback executed".into(), &root_id);
        }
        log(
            "INFO",
            format!("experiment {outcome_status}: hypothesis_met={hypothesis_met}"),
            &root_id,
        );
    }

    let resource = || Some(tumult_resource());

    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: resource(),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: resource(),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![
                    Metric {
                        name: "tumult.experiments.total".into(),
                        unit: "{experiment}".into(),
                        data: Some(metric::Data::Sum(Sum {
                            data_points: sum_points,
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "tumult.hypothesis.deviations.total".into(),
                        unit: "{deviation}".into(),
                        data: Some(metric::Data::Sum(Sum {
                            data_points: deviation_points,
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "tumult.experiment.duration".into(),
                        unit: "s".into(),
                        data: Some(metric::Data::Histogram(Histogram {
                            data_points: histogram_points,
                            aggregation_temporality: 2,
                        })),
                        ..Metric::default()
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: resource(),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    DemoData {
        traces,
        metrics,
        logs,
        experiments: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn generates_expected_span_tree_shape() {
        let mut rng = XorShift::new(42);
        let data = generate(&mut rng, 10, 1_800_000_000_000_000_000, true);

        // Group spans by trace.
        let spans: Vec<&Span> = data.traces.resource_spans[0].scope_spans[0]
            .spans
            .iter()
            .collect();
        let mut by_trace: HashMap<&[u8], Vec<&Span>> = HashMap::new();
        for span in &spans {
            by_trace.entry(&span.trace_id).or_default().push(span);
        }
        assert_eq!(by_trace.len(), 10, "one trace per experiment");

        for spans in by_trace.values() {
            // 5 or 6 spans: root + 4 children (+ rollback on ~20%).
            assert!((5..=6).contains(&spans.len()));
            let roots: Vec<_> = spans
                .iter()
                .filter(|s| s.parent_span_id.is_empty())
                .collect();
            assert_eq!(roots.len(), 1);
            assert_eq!(roots[0].name, "resilience.experiment");
            for name in [
                "resilience.hypothesis.before",
                "resilience.action",
                "resilience.probe",
                "resilience.hypothesis.after",
            ] {
                assert!(spans.iter().any(|s| s.name == name), "missing {name}");
            }
            // The root carries the promoted resilience.* attributes.
            let keys: Vec<&str> = roots[0]
                .attributes
                .iter()
                .map(|kv| kv.key.as_str())
                .collect();
            for key in [
                "resilience.experiment.id",
                "resilience.experiment.name",
                "resilience.target.system",
                "resilience.fault.type",
                "resilience.outcome.status",
                "resilience.outcome.hypothesis_met",
                "resilience.outcome.recovery_time_s",
            ] {
                assert!(keys.contains(&key), "root missing {key}");
            }
        }
    }

    #[test]
    fn generates_metrics_and_logs() {
        let mut rng = XorShift::new(7);
        let data = generate(&mut rng, 40, 1_800_000_000_000_000_000, true);
        let metrics = &data.metrics.resource_metrics[0].scope_metrics[0].metrics;

        let sums = match metrics[0].data.as_ref().unwrap() {
            metric::Data::Sum(s) => &s.data_points,
            _ => panic!("expected sum"),
        };
        assert_eq!(
            sums.len(),
            40,
            "one tumult.experiments.total point per experiment"
        );

        let deviations = match metrics[1].data.as_ref().unwrap() {
            metric::Data::Sum(s) => &s.data_points,
            _ => panic!("expected sum"),
        };
        assert!(!deviations.is_empty(), "seed 7 should produce deviations");
        assert!(deviations.len() < 40);

        let histograms = match metrics[2].data.as_ref().unwrap() {
            metric::Data::Histogram(h) => &h.data_points,
            _ => panic!("expected histogram"),
        };
        assert_eq!(histograms.len(), 40);

        let records = &data.logs.resource_logs[0].scope_logs[0].log_records;
        assert!(
            records.len() >= 40 * 3,
            "at least 3 log records per experiment"
        );
        assert!(records.iter().all(|r| !r.trace_id.is_empty()));
    }

    #[test]
    fn deterministic_per_seed() {
        let mut a = XorShift::new(42);
        let mut b = XorShift::new(42);
        let da = generate(&mut a, 5, 1_800_000_000_000_000_000, true);
        let db = generate(&mut b, 5, 1_800_000_000_000_000_000, true);
        let names = |d: &DemoData| {
            d.traces.resource_spans[0].scope_spans[0]
                .spans
                .iter()
                .map(|s| (s.name.clone(), s.start_time_unix_nano))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&da), names(&db));
    }

    #[test]
    fn outcome_split_is_roughly_75_25() {
        let mut rng = XorShift::new(7);
        let data = generate(&mut rng, 40, 1_800_000_000_000_000_000, true);
        let spans = &data.traces.resource_spans[0].scope_spans[0].spans;
        let met = spans
            .iter()
            .filter(|s| s.name == "resilience.experiment")
            .filter(|s| {
                s.attributes.iter().any(|kv| {
                    kv.key == "resilience.outcome.hypothesis_met"
                        && matches!(
                            kv.value.as_ref().and_then(|v| v.value.as_ref()),
                            Some(Value::BoolValue(true))
                        )
                })
            })
            .count();
        assert!(
            (24..=36).contains(&met),
            "met={met} should be near 75% of 40"
        );
    }
}
