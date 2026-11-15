//! OpenTelemetry instrumentation for the analytics pipeline.
//!
//! Provides spans, events, and gauges for every stage:
//! ```text
//! Experiment → TOON Journal → Arrow → DuckDB → Parquet
//! ```
//!
//! ## Spans
//! - `resilience.analytics.ingest` — journal → Arrow → `DuckDB`
//! - `resilience.analytics.query` — SQL execution
//! - `resilience.analytics.export` — Parquet/CSV/IPC write
//! - `resilience.analytics.import` — Parquet → `DuckDB` restore
//! - `resilience.analytics.purge` — retention policy execution
//!
//! ## Events (structured logs per `OTel` event semantic conventions)
//! - `journal.ingested`
//! - `resilience.analytics.journal.duplicate`
//! - `resilience.analytics.query.executed`
//! - `resilience.analytics.export.completed`
//! - `resilience.analytics.import.completed`
//! - `resilience.analytics.purge.completed`
//!
//! ## Gauges
//! - `resilience.store.experiments` — experiment count
//! - `resilience.store.activities` — activity count
//! - `resilience.store.size_bytes` — database file size

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER_NAME: &str = "tumult-analytics";

/// Start a span for journal ingestion. Returns a context guard — span ends on drop.
#[must_use]
pub fn begin_ingest(experiment_id: &str, experiment_title: &str) -> IngestGuard {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder("resilience.analytics.ingest")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new("resilience.experiment.id", experiment_id.to_string()),
            KeyValue::new("resilience.experiment.title", experiment_title.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    IngestGuard {
        _guard: cx.attach(),
    }
}

pub struct IngestGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Emit event: journal ingested successfully.
pub fn event_journal_ingested(experiment_id: &str, activity_count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "journal.ingested",
        vec![
            KeyValue::new("resilience.experiment.id", experiment_id.to_string()),
            // usize → i64: activity counts are always << i64::MAX.
            KeyValue::new(
                "resilience.activity.count",
                i64::try_from(activity_count).unwrap_or(i64::MAX),
            ),
        ],
    );
}

/// Emit event: journal skipped (duplicate).
pub fn event_journal_duplicate(experiment_id: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "resilience.analytics.journal.duplicate",
        vec![KeyValue::new(
            "resilience.experiment.id",
            experiment_id.to_string(),
        )],
    );
}

/// Start a span for SQL query execution. Returns a context guard.
#[must_use]
pub fn begin_query(sql: &str) -> QueryGuard {
    let tracer = global::tracer(TRACER_NAME);
    let sql_preview = if sql.len() > 256 {
        format!("{}...", &sql[..sql.floor_char_boundary(256)])
    } else {
        sql.to_string()
    };
    let span = tracer
        .span_builder("resilience.analytics.query")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![KeyValue::new("db.statement", sql_preview)])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    QueryGuard {
        _guard: cx.attach(),
    }
}

pub struct QueryGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Emit event: SQL query executed with results.
pub fn event_query_executed(row_count: usize, column_count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "resilience.analytics.query.executed",
        vec![
            KeyValue::new(
                "resilience.query.row_count",
                i64::try_from(row_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "resilience.query.column_count",
                i64::try_from(column_count).unwrap_or(i64::MAX),
            ),
        ],
    );
}

/// Start a span for data export. Returns a context guard.
#[must_use]
pub fn begin_export(format: &str, path: &str) -> ExportGuard {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder("resilience.analytics.export")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new("resilience.export.format", format.to_string()),
            KeyValue::new("resilience.export.path", path.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    ExportGuard {
        _guard: cx.attach(),
    }
}

pub struct ExportGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Emit event: data exported.
pub fn event_export_completed(format: &str, row_count: usize, bytes: u64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "resilience.analytics.export.completed",
        vec![
            KeyValue::new("resilience.export.format", format.to_string()),
            KeyValue::new(
                "resilience.export.row_count",
                i64::try_from(row_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "resilience.export.bytes",
                i64::try_from(bytes).unwrap_or(i64::MAX),
            ),
        ],
    );
}

/// Start a span for data import. Returns a context guard.
#[must_use]
pub fn begin_import(path: &str) -> ImportGuard {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder("resilience.analytics.import")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![KeyValue::new(
            "resilience.import.path",
            path.to_string(),
        )])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    ImportGuard {
        _guard: cx.attach(),
    }
}

pub struct ImportGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Emit event: data imported.
pub fn event_import_completed(experiment_count: usize, activity_count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "resilience.analytics.import.completed",
        vec![
            KeyValue::new(
                "resilience.import.experiment_count",
                i64::try_from(experiment_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "resilience.import.activity_count",
                i64::try_from(activity_count).unwrap_or(i64::MAX),
            ),
        ],
    );
}

/// Start a span for retention purge. Returns a context guard.
#[must_use]
pub fn begin_purge(older_than_days: u32) -> PurgeGuard {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder("resilience.analytics.purge")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![KeyValue::new(
            "resilience.purge.older_than_days",
            i64::from(older_than_days),
        )])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    PurgeGuard {
        _guard: cx.attach(),
    }
}

pub struct PurgeGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Emit event: purge completed.
pub fn event_purge_completed(purged_count: usize, remaining_count: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "resilience.analytics.purge.completed",
        vec![
            KeyValue::new(
                "resilience.purge.removed",
                i64::try_from(purged_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "resilience.purge.remaining",
                i64::try_from(remaining_count).unwrap_or(i64::MAX),
            ),
        ],
    );
}

// ── Gauges ──────────────────────────────────────────────────

/// Record store gauges (experiment count, activity count, file size).
pub fn record_store_gauges(
    experiment_count: usize,
    activity_count: usize,
    size_bytes: Option<u64>,
) {
    let meter = global::meter(TRACER_NAME);
    record_store_gauges_on(&meter, experiment_count, activity_count, size_bytes);
}

/// [`record_store_gauges`] on `meter` instead of the global one.
fn record_store_gauges_on(
    meter: &Meter,
    experiment_count: usize,
    activity_count: usize,
    size_bytes: Option<u64>,
) {
    let g = meter.u64_gauge("resilience.store.experiments").build();
    g.record(experiment_count as u64, &[]);

    let g = meter.u64_gauge("resilience.store.activities").build();
    g.record(activity_count as u64, &[]);

    if let Some(bytes) = size_bytes {
        let g = meter.u64_gauge("resilience.store.size_bytes").build();
        g.record(bytes, &[]);
    }
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::{capture_spans, MetricCapture};

    use super::*;

    /// What one guard left behind once it was dropped.
    #[derive(Debug, PartialEq)]
    struct Recorded {
        name: String,
        attributes: Vec<KeyValue>,
        events: Vec<(String, Vec<KeyValue>)>,
    }

    /// The single span `f` finished.
    fn recorded(f: impl FnOnce()) -> Recorded {
        let ((), spans) = capture_spans(f);
        assert_eq!(spans.len(), 1, "one span per guard: {spans:?}");
        let span = &spans[0];
        Recorded {
            name: span.name.to_string(),
            attributes: span.attributes.clone(),
            events: span
                .events
                .events
                .iter()
                .map(|event| (event.name.to_string(), event.attributes.clone()))
                .collect(),
        }
    }

    fn event(name: &str, attributes: Vec<KeyValue>) -> (String, Vec<KeyValue>) {
        (name.to_string(), attributes)
    }

    /// The `db.statement` recorded for `sql`.
    fn statement_recorded(sql: &str) -> String {
        let span = recorded(|| {
            let _guard = begin_query(sql);
        });
        assert_eq!(span.attributes.len(), 1, "{span:?}");
        assert_eq!(span.attributes[0].key.as_str(), "db.statement");
        span.attributes[0].value.as_str().into_owned()
    }

    #[test]
    fn ingest_span_names_the_experiment_and_counts_its_activities() {
        let span = recorded(|| {
            let _guard = begin_ingest("e-001", "test experiment");
            event_journal_ingested("e-001", 5);
        });

        assert_eq!(
            span,
            Recorded {
                name: "resilience.analytics.ingest".into(),
                attributes: vec![
                    KeyValue::new("resilience.experiment.id", "e-001"),
                    KeyValue::new("resilience.experiment.title", "test experiment"),
                ],
                events: vec![event(
                    "journal.ingested",
                    vec![
                        KeyValue::new("resilience.experiment.id", "e-001"),
                        KeyValue::new("resilience.activity.count", 5_i64),
                    ],
                )],
            }
        );
    }

    #[test]
    fn duplicate_journal_is_an_event_on_the_ingest_span() {
        let span = recorded(|| {
            let _guard = begin_ingest("e-002", "dup test");
            event_journal_duplicate("e-002");
        });

        assert_eq!(span.name, "resilience.analytics.ingest");
        assert_eq!(
            span.events,
            [event(
                "resilience.analytics.journal.duplicate",
                vec![KeyValue::new("resilience.experiment.id", "e-002")],
            )]
        );
    }

    #[test]
    fn query_span_records_the_statement_and_the_result_shape() {
        let span = recorded(|| {
            let _guard = begin_query("SELECT count(*) FROM experiments");
            event_query_executed(10, 3);
        });

        assert_eq!(
            span,
            Recorded {
                name: "resilience.analytics.query".into(),
                attributes: vec![KeyValue::new(
                    "db.statement",
                    "SELECT count(*) FROM experiments"
                )],
                events: vec![event(
                    "resilience.analytics.query.executed",
                    vec![
                        KeyValue::new("resilience.query.row_count", 10_i64),
                        KeyValue::new("resilience.query.column_count", 3_i64),
                    ],
                )],
            }
        );
    }

    #[test]
    fn query_statement_is_cut_to_256_bytes() {
        let exact = "x".repeat(256);
        let long = "y".repeat(257);

        assert_eq!(statement_recorded(&exact), exact);
        assert_eq!(statement_recorded(&long), format!("{}...", "y".repeat(256)));
    }

    #[test]
    fn query_statement_is_cut_on_a_character_boundary() {
        // Byte 256 falls inside the two-byte character that starts at 255.
        let sql = format!("{}{}", "x".repeat(255), "é".repeat(10));

        assert_eq!(statement_recorded(&sql), format!("{}...", "x".repeat(255)));
    }

    #[test]
    fn export_span_records_the_target_and_what_was_written() {
        let span = recorded(|| {
            let _guard = begin_export("parquet", "/tmp/out.parquet");
            event_export_completed("parquet", 100, 4096);
        });

        assert_eq!(
            span,
            Recorded {
                name: "resilience.analytics.export".into(),
                attributes: vec![
                    KeyValue::new("resilience.export.format", "parquet"),
                    KeyValue::new("resilience.export.path", "/tmp/out.parquet"),
                ],
                events: vec![event(
                    "resilience.analytics.export.completed",
                    vec![
                        KeyValue::new("resilience.export.format", "parquet"),
                        KeyValue::new("resilience.export.row_count", 100_i64),
                        KeyValue::new("resilience.export.bytes", 4096_i64),
                    ],
                )],
            }
        );
    }

    #[test]
    fn import_span_records_the_source_and_what_was_restored() {
        let span = recorded(|| {
            let _guard = begin_import("/tmp/backup");
            event_import_completed(50, 200);
        });

        assert_eq!(
            span,
            Recorded {
                name: "resilience.analytics.import".into(),
                attributes: vec![KeyValue::new("resilience.import.path", "/tmp/backup")],
                events: vec![event(
                    "resilience.analytics.import.completed",
                    vec![
                        KeyValue::new("resilience.import.experiment_count", 50_i64),
                        KeyValue::new("resilience.import.activity_count", 200_i64),
                    ],
                )],
            }
        );
    }

    #[test]
    fn purge_span_records_the_age_limit_and_the_outcome() {
        let span = recorded(|| {
            let _guard = begin_purge(90);
            event_purge_completed(10, 40);
        });

        assert_eq!(
            span,
            Recorded {
                name: "resilience.analytics.purge".into(),
                attributes: vec![KeyValue::new("resilience.purge.older_than_days", 90_i64)],
                events: vec![event(
                    "resilience.analytics.purge.completed",
                    vec![
                        KeyValue::new("resilience.purge.removed", 10_i64),
                        KeyValue::new("resilience.purge.remaining", 40_i64),
                    ],
                )],
            }
        );
    }

    #[test]
    fn store_gauges_report_the_latest_counts_and_a_known_size() {
        let capture = MetricCapture::new();
        let meter = capture.meter(TRACER_NAME);
        let value =
            |name: &str| -> Vec<f64> { capture.points(name).iter().map(|p| p.value).collect() };

        record_store_gauges_on(&meter, 10, 50, None);
        assert_eq!(value("resilience.store.experiments"), [10.0]);
        assert_eq!(value("resilience.store.activities"), [50.0]);
        assert_eq!(
            value("resilience.store.size_bytes"),
            [0.0_f64; 0],
            "an unknown size is not reported as zero"
        );

        record_store_gauges_on(&meter, 12, 61, Some(1024 * 1024));
        assert_eq!(value("resilience.store.experiments"), [12.0]);
        assert_eq!(value("resilience.store.activities"), [61.0]);
        assert_eq!(value("resilience.store.size_bytes"), [1_048_576.0]);
    }
}
