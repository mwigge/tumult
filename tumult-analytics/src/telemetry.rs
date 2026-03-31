//! OpenTelemetry instrumentation for the analytics pipeline.
//!
//! Provides spans, events, and gauges for every stage:
//! ```text
//! Experiment → TOON Journal → Arrow → DuckDB → Parquet
//! ```
//!
//! ## Spans
//! - `resilience.analytics.ingest` — journal → Arrow → DuckDB
//! - `resilience.analytics.query` — SQL execution
//! - `resilience.analytics.export` — Parquet/CSV/IPC write
//! - `resilience.analytics.import` — Parquet → DuckDB restore
//! - `resilience.analytics.purge` — retention policy execution
//!
//! ## Events (structured logs per OTel event semantic conventions)
//! - `resilience.analytics.journal.ingested`
//! - `resilience.analytics.journal.duplicate`
//! - `resilience.analytics.query.executed`
//! - `resilience.analytics.export.completed`
//! - `resilience.analytics.import.completed`
//! - `resilience.analytics.purge.completed`
//!
//! ## Gauges
//! - `tumult.store.experiments` — experiment count
//! - `tumult.store.activities` — activity count
//! - `tumult.store.size_bytes` — database file size

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER_NAME: &str = "tumult-analytics";

/// Start a span for journal ingestion. Returns a context guard — span ends on drop.
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
        "resilience.analytics.journal.ingested",
        vec![
            KeyValue::new("resilience.experiment.id", experiment_id.to_string()),
            KeyValue::new("resilience.activity.count", activity_count as i64),
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
pub fn begin_query(sql: &str) -> QueryGuard {
    let tracer = global::tracer(TRACER_NAME);
    let sql_preview = if sql.len() > 256 {
        format!("{}...", &sql[..256])
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
            KeyValue::new("resilience.query.row_count", row_count as i64),
            KeyValue::new("resilience.query.column_count", column_count as i64),
        ],
    );
}

/// Start a span for data export. Returns a context guard.
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
            KeyValue::new("resilience.export.row_count", row_count as i64),
            KeyValue::new("resilience.export.bytes", bytes as i64),
        ],
    );
}

/// Start a span for data import. Returns a context guard.
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
                experiment_count as i64,
            ),
            KeyValue::new("resilience.import.activity_count", activity_count as i64),
        ],
    );
}

/// Start a span for retention purge. Returns a context guard.
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
            KeyValue::new("resilience.purge.removed", purged_count as i64),
            KeyValue::new("resilience.purge.remaining", remaining_count as i64),
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

    let g = meter.u64_gauge("tumult.store.experiments").build();
    g.record(experiment_count as u64, &[]);

    let g = meter.u64_gauge("tumult.store.activities").build();
    g.record(activity_count as u64, &[]);

    if let Some(bytes) = size_bytes {
        let g = meter.u64_gauge("tumult.store.size_bytes").build();
        g.record(bytes, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_guard_creates_and_drops_span() {
        let _guard = begin_ingest("e-001", "test experiment");
        event_journal_ingested("e-001", 5);
        // guard drops here, ending the span
    }

    #[test]
    fn duplicate_event_does_not_panic() {
        let _guard = begin_ingest("e-002", "dup test");
        event_journal_duplicate("e-002");
    }

    #[test]
    fn query_guard_creates_and_drops_span() {
        let _guard = begin_query("SELECT count(*) FROM experiments");
        event_query_executed(10, 3);
    }

    #[test]
    fn query_truncates_long_sql() {
        let long_sql = "SELECT ".to_string() + &"x".repeat(500);
        let _guard = begin_query(&long_sql);
        event_query_executed(0, 0);
    }

    #[test]
    fn export_guard_creates_and_drops_span() {
        let _guard = begin_export("parquet", "/tmp/out.parquet");
        event_export_completed("parquet", 100, 4096);
    }

    #[test]
    fn import_guard_creates_and_drops_span() {
        let _guard = begin_import("/tmp/backup");
        event_import_completed(50, 200);
    }

    #[test]
    fn purge_guard_creates_and_drops_span() {
        let _guard = begin_purge(90);
        event_purge_completed(10, 40);
    }

    #[test]
    fn record_gauges_does_not_panic() {
        record_store_gauges(10, 50, Some(1024 * 1024));
        record_store_gauges(0, 0, None);
    }
}
