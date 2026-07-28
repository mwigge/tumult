//! Plain row structs matching the schema v1 tables.
//!
//! Attribute maps travel as `Vec<(String, String)>` pairs and are bound into
//! the `MAP(VARCHAR, VARCHAR)` columns as JSON objects (see `schema.rs`).

/// One row of the `spans` table.
///
/// The materialized `resilience.*` columns are promoted by the OTLP
/// translation layer (`kronika-otel`); everything else lands in the maps.
#[derive(Debug, Clone, Default)]
pub struct SpanRow {
    pub ts_ns: i64,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub span_kind: String,
    pub duration_ns: i64,
    pub status_code: String,
    pub status_message: String,
    pub service_name: String,
    pub service_version: Option<String>,
    pub experiment_id: Option<String>,
    pub experiment_name: Option<String>,
    pub outcome_status: Option<String>,
    pub fault_type: Option<String>,
    pub fault_subtype: Option<String>,
    pub fault_severity: Option<String>,
    pub blast_radius: Option<String>,
    pub target_system: Option<String>,
    pub target_technology: Option<String>,
    pub target_environment: Option<String>,
    pub plugin_name: Option<String>,
    pub hypothesis_met: Option<bool>,
    pub recovery_time_s: Option<f64>,
    pub span_attrs: Vec<(String, String)>,
    pub resource_attrs: Vec<(String, String)>,
    /// Span events, pre-serialised as a JSON array string (`"[]"` when empty).
    pub events: String,
}

/// One row of the `logs` table.
#[derive(Debug, Clone, Default)]
pub struct LogRow {
    pub ts_ns: i64,
    pub severity_text: String,
    pub body: String,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub service_name: String,
    pub log_attrs: Vec<(String, String)>,
    pub resource_attrs: Vec<(String, String)>,
}

/// One row of the `metric_sums` table.
#[derive(Debug, Clone, Default)]
pub struct MetricSumRow {
    pub ts_ns: i64,
    pub metric_name: String,
    pub value: f64,
    pub experiment_name: Option<String>,
    pub outcome_status: Option<String>,
    pub plugin_name: Option<String>,
    pub attrs: Vec<(String, String)>,
    pub resource_attrs: Vec<(String, String)>,
}

/// One row of the `metric_gauges` table (same shape as `metric_sums`).
#[derive(Debug, Clone, Default)]
pub struct MetricGaugeRow {
    pub ts_ns: i64,
    pub metric_name: String,
    pub value: f64,
    pub experiment_name: Option<String>,
    pub outcome_status: Option<String>,
    pub plugin_name: Option<String>,
    pub attrs: Vec<(String, String)>,
    pub resource_attrs: Vec<(String, String)>,
}

/// One row of the `metric_histograms` table.
#[derive(Debug, Clone, Default)]
pub struct MetricHistogramRow {
    pub ts_ns: i64,
    pub metric_name: String,
    pub count: u64,
    pub sum: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub bucket_counts: Vec<i64>,
    pub explicit_bounds: Vec<f64>,
    pub attrs: Vec<(String, String)>,
    pub resource_attrs: Vec<(String, String)>,
}

/// One row of the `import_batches` table, recording a manual CSV/JSON import.
#[derive(Debug, Clone)]
pub struct ImportBatch {
    pub id: String,
    pub source: String,
    pub imported_at_ns: i64,
    pub rows: i32,
    pub label: Option<String>,
}

/// One row of the `experiment_runs` rollup view.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentRun {
    pub experiment_id: Option<String>,
    pub experiment_name: Option<String>,
    pub started_at_ns: Option<i64>,
    pub ended_at_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub outcome_status: Option<String>,
    pub hypothesis_met: Option<bool>,
}
