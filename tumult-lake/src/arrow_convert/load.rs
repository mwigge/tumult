//! `LoadResult` → Arrow `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_core::types::LoadResult;

use crate::error::AnalyticsError;

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
