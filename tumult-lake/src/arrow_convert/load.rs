//! `LoadResult` → Arrow `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_core::types::{LoadResult, LoadSample};

use super::objectives::journal_probe_samples_schema;
use crate::error::AnalyticsError;

/// The `probe_samples` series a load sample is split into: `(probe_name,
/// value accessor)`. The names mirror the `load.*` objective metrics so an
/// `nfr` entry and a stored series read the same.
type SeriesAccessor = fn(&LoadSample) -> f64;
const LOAD_SERIES: [(&str, SeriesAccessor); 5] = [
    ("load.throughput_rps", |s| s.throughput_rps),
    ("load.latency_p50_ms", |s| s.latency_p50_ms),
    ("load.latency_p95_ms", |s| s.latency_p95_ms),
    ("load.latency_p99_ms", |s| s.latency_p99_ms),
    ("load.error_rate", |s| s.error_rate),
];

/// Schema for the `load_results` `DuckDB` table.
#[must_use]
pub fn load_results_schema() -> Schema {
    Schema::new(vec![
        Field::new("experiment_id", DataType::Utf8, false),
        Field::new("tool", DataType::Utf8, false),
        Field::new("started_at_ns", DataType::Int64, false),
        Field::new("ended_at_ns", DataType::Int64, false),
        Field::new("duration_s", DataType::Float64, false),
        Field::new("vus", DataType::Int32, false),
        Field::new("throughput_rps", DataType::Float64, false),
        Field::new("latency_p50_ms", DataType::Float64, false),
        Field::new("latency_p95_ms", DataType::Float64, false),
        Field::new("latency_p99_ms", DataType::Float64, false),
        Field::new("error_rate", DataType::Float64, false),
        Field::new("total_requests", DataType::UInt64, false),
        Field::new("thresholds_met", DataType::Boolean, false),
    ])
}

/// Converts a `LoadResult` into an Arrow `RecordBatch` for the `load_results` table.
///
/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn journal_to_load_batch(
    experiment_id: &str,
    load: &LoadResult,
) -> Result<RecordBatch, AnalyticsError> {
    let schema = Arc::new(load_results_schema());
    #[allow(clippy::cast_possible_wrap)]
    let vus = load.vus as i32;
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![experiment_id])) as ArrayRef,
            Arc::new(StringArray::from(vec![load.tool.to_string()])),
            Arc::new(Int64Array::from(vec![load.started_at_ns])),
            Arc::new(Int64Array::from(vec![load.ended_at_ns])),
            Arc::new(Float64Array::from(vec![load.duration_s])),
            Arc::new(Int32Array::from(vec![vus])),
            Arc::new(Float64Array::from(vec![load.throughput_rps])),
            Arc::new(Float64Array::from(vec![load.latency_p50_ms])),
            Arc::new(Float64Array::from(vec![load.latency_p95_ms])),
            Arc::new(Float64Array::from(vec![load.latency_p99_ms])),
            Arc::new(Float64Array::from(vec![load.error_rate])),
            Arc::new(UInt64Array::from(vec![load.total_requests])),
            Arc::new(BooleanArray::from(vec![load.thresholds_met])),
        ],
    )?)
}

/// The load result's windowed samples as `probe_samples` rows: one row per
/// window per `load.*` series, phase `during`, `ok` when the window saw
/// no failed requests. Empty when the tool streamed nothing.
///
/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn load_samples_to_probe_samples_batch(
    experiment_id: &str,
    load: &LoadResult,
) -> Result<RecordBatch, AnalyticsError> {
    let n = load.samples.len() * LOAD_SERIES.len();
    let mut ids = Vec::with_capacity(n);
    let mut names = Vec::with_capacity(n);
    let mut phases = Vec::with_capacity(n);
    let mut ts = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    let mut oks = Vec::with_capacity(n);
    for sample in &load.samples {
        for (name, value) in LOAD_SERIES {
            ids.push(experiment_id);
            names.push(name);
            phases.push("during");
            ts.push(sample.ts_ns);
            values.push(value(sample));
            oks.push(sample.error_rate <= 0.0);
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(journal_probe_samples_schema()),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(phases)),
            Arc::new(Int64Array::from(ts)),
            Arc::new(Float64Array::from(values)),
            Arc::new(BooleanArray::from(oks)),
        ],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Array;
    use tumult_core::types::LoadTool;

    fn sample(ts_ns: i64, p95: f64, error_rate: f64) -> LoadSample {
        LoadSample {
            ts_ns,
            duration_s: 10.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: p95,
            latency_p99_ms: 90.0,
            error_rate,
            requests: 4,
        }
    }

    fn result_with_samples() -> LoadResult {
        LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 0,
            ended_at_ns: 20,
            duration_s: 20.0,
            vus: 4,
            throughput_rps: 50.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 80.0,
            latency_p99_ms: 100.0,
            error_rate: 0.0,
            total_requests: 8,
            thresholds_met: true,
            samples: vec![sample(1, 70.0, 0.0), sample(2, 90.0, 0.25)],
            execution: tumult_core::types::LoadExecution::default(),
        }
    }

    #[test]
    fn samples_become_one_row_per_window_per_series() {
        let batch = load_samples_to_probe_samples_batch("exp-1", &result_with_samples()).unwrap();
        assert_eq!(batch.num_rows(), 10);
        assert_eq!(batch.schema().fields().len(), 6);
        let names = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "load.throughput_rps");
        assert_eq!(names.value(7), "load.latency_p95_ms");
        let values = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(7) - 90.0).abs() < f64::EPSILON);
        let oks = batch
            .column(5)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(oks.value(0));
        assert!(!oks.value(5));
    }

    #[test]
    fn no_samples_is_an_empty_batch() {
        let mut load = result_with_samples();
        load.samples.clear();
        let batch = load_samples_to_probe_samples_batch("exp-1", &load).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }
}
