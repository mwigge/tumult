//! `LoadResult` → Arrow `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_core::types::{LoadResult, LoadSample, RequestCounts};

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

/// The `probe_samples` series a window carries beside its measurements,
/// each only where the tool told it: how long the window was, what
/// became of its requests, and the timeout its requests were held to.
pub const SERIES_WINDOW_S: &str = "load.window_s";
pub const SERIES_REQUESTS_SENT: &str = "load.requests_sent";
pub const SERIES_REQUESTS_OK: &str = "load.requests_ok";
pub const SERIES_REQUESTS_TIMED_OUT: &str = "load.requests_timed_out";
pub const SERIES_REQUEST_TIMEOUT_MS: &str = "load.request_timeout_ms";

type WindowAccessor = fn(&LoadSample) -> Option<f64>;
const WINDOW_SERIES: [(&str, WindowAccessor); 5] = [
    (SERIES_WINDOW_S, |s| Some(s.duration_s)),
    (SERIES_REQUESTS_SENT, |s| s.counts.map(|c| as_value(c.sent))),
    (SERIES_REQUESTS_OK, |s| s.counts.map(|c| as_value(c.ok))),
    (SERIES_REQUESTS_TIMED_OUT, |s| {
        s.counts.map(|c| as_value(c.timed_out))
    }),
    (SERIES_REQUEST_TIMEOUT_MS, |s| s.request_timeout_ms),
];

/// The name of every `probe_samples` series a load window is stored as.
#[must_use]
pub fn load_series_names() -> Vec<&'static str> {
    let measured = LOAD_SERIES.iter().map(|(name, _)| *name);
    let told = WINDOW_SERIES.iter().map(|(name, _)| *name);
    measured.chain(told).collect()
}

/// A request count as a series value; counts are far below 2^53.
#[allow(clippy::cast_precision_loss)]
fn as_value(count: u64) -> f64 {
    count as f64
}

/// A request count as the lake's `BIGINT`.
fn as_bigint(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

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
        // How the load was executed; NULL on rows written before these
        // columns existed.
        Field::new("aborted", DataType::Boolean, true),
        Field::new("truncated", DataType::Boolean, true),
        Field::new("planned_windows", DataType::Int32, true),
        Field::new("completed_windows", DataType::Int32, true),
        Field::new("unanswered_windows", DataType::Int32, true),
        Field::new("lost_to_unanswered", DataType::Int32, true),
        // What the samples add up to; NULL when the tool streamed none.
        Field::new("sampled_duration_s", DataType::Float64, true),
        // What became of the requests; NULL where the tool does not say
        // and on rows written before these columns existed.
        Field::new("requests_sent", DataType::Int64, true),
        Field::new("requests_ok", DataType::Int64, true),
        Field::new("requests_timed_out", DataType::Int64, true),
        Field::new("requests_transport", DataType::Int64, true),
        Field::new("requests_http_4xx", DataType::Int64, true),
        Field::new("requests_http_5xx", DataType::Int64, true),
        Field::new("requests_http_other", DataType::Int64, true),
        Field::new("requests_unclassified", DataType::Int64, true),
        // Windows whose percentile fell among the timed-out requests.
        Field::new("censored_windows", DataType::Int32, true),
    ])
}

/// A window count as the lake's `INTEGER`.
fn windows(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// The time the streamed samples cover, or `None` when there are none.
fn sampled_duration_s(load: &LoadResult) -> Option<f64> {
    if load.samples.is_empty() {
        return None;
    }
    Some(load.samples.iter().map(|s| s.duration_s).sum())
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
    let execution = &load.execution;
    let requests = |class: fn(&RequestCounts) -> u64| -> ArrayRef {
        let count = execution.requests.as_ref().map(|c| as_bigint(class(c)));
        Arc::new(Int64Array::from(vec![count]))
    };
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
            Arc::new(BooleanArray::from(vec![Some(execution.aborted)])),
            Arc::new(BooleanArray::from(vec![Some(execution.truncated())])),
            Arc::new(Int32Array::from(vec![execution
                .planned_windows
                .map(windows)])),
            Arc::new(Int32Array::from(vec![execution
                .completed_windows
                .map(windows)])),
            Arc::new(Int32Array::from(vec![Some(windows(
                execution.unanswered_windows,
            ))])),
            Arc::new(Int32Array::from(vec![Some(windows(
                execution.lost_to_unanswered,
            ))])),
            Arc::new(Float64Array::from(vec![sampled_duration_s(load)])),
            requests(|c| c.sent),
            requests(|c| c.ok),
            requests(|c| c.timed_out),
            requests(|c| c.transport),
            requests(|c| c.http_4xx),
            requests(|c| c.http_5xx),
            requests(|c| c.http_other),
            requests(|c| c.unclassified),
            Arc::new(Int32Array::from(vec![Some(windows(
                execution.censored_windows,
            ))])),
        ],
    )?)
}

/// The load result's windowed samples as `probe_samples` rows: one row per
/// window per `load.*` series, then one per window series the tool told,
/// phase `during`, `ok` when the window saw no failed requests, no
/// `reading`. Empty when the tool streamed nothing.
///
/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn load_samples_to_probe_samples_batch(
    experiment_id: &str,
    load: &LoadResult,
) -> Result<RecordBatch, AnalyticsError> {
    let n = load.samples.len() * (LOAD_SERIES.len() + WINDOW_SERIES.len());
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
        for (name, value) in WINDOW_SERIES {
            let Some(value) = value(sample) else {
                continue;
            };
            ids.push(experiment_id);
            names.push(name);
            phases.push("during");
            ts.push(sample.ts_ns);
            values.push(value);
            oks.push(sample.error_rate <= 0.0);
        }
    }
    let rows = ids.len();
    Ok(RecordBatch::try_new(
        Arc::new(journal_probe_samples_schema()),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(phases)),
            Arc::new(Int64Array::from(ts)),
            Arc::new(Float64Array::from(values)),
            Arc::new(BooleanArray::from(oks)),
            Arc::new(Float64Array::from(vec![None::<f64>; rows])),
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
            counts: None,
            request_timeout_ms: None,
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
        // Five measurements and the window's length, per window.
        assert_eq!(batch.num_rows(), 12);
        assert_eq!(batch.schema().fields().len(), 7);
        let names = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "load.throughput_rps");
        assert_eq!(names.value(5), "load.window_s");
        assert_eq!(names.value(8), "load.latency_p95_ms");
        let values = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(5) - 10.0).abs() < f64::EPSILON);
        assert!((values.value(8) - 90.0).abs() < f64::EPSILON);
        let oks = batch
            .column(5)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(oks.value(0));
        assert!(!oks.value(6));
    }

    #[test]
    fn a_window_carries_what_became_of_its_requests_and_its_timeout() {
        let mut load = result_with_samples();
        load.samples.truncate(1);
        load.samples[0].counts = Some(RequestCounts {
            sent: 40,
            ok: 30,
            timed_out: 6,
            transport: 1,
            http_5xx: 3,
            ..RequestCounts::default()
        });
        load.samples[0].request_timeout_ms = Some(2000.0);
        let batch = load_samples_to_probe_samples_batch("exp-1", &load).unwrap();
        let names = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let values = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let series: Vec<(&str, f64)> = (5..batch.num_rows())
            .map(|row| (names.value(row), values.value(row)))
            .collect();
        assert_eq!(
            series,
            [
                ("load.window_s", 10.0),
                ("load.requests_sent", 40.0),
                ("load.requests_ok", 30.0),
                ("load.requests_timed_out", 6.0),
                ("load.request_timeout_ms", 2000.0),
            ]
        );
    }

    #[test]
    fn the_names_of_the_load_series_are_the_ones_a_whole_window_is_stored_as() {
        let mut load = result_with_samples();
        load.samples.truncate(1);
        load.samples[0].counts = Some(RequestCounts::default());
        load.samples[0].request_timeout_ms = Some(2000.0);
        let batch = load_samples_to_probe_samples_batch("exp-1", &load).unwrap();
        let names = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let stored: Vec<&str> = (0..batch.num_rows()).map(|row| names.value(row)).collect();
        assert_eq!(stored.len(), 10);
        assert_eq!(load_series_names(), stored);
    }

    #[test]
    fn no_samples_is_an_empty_batch() {
        let mut load = result_with_samples();
        load.samples.clear();
        let batch = load_samples_to_probe_samples_batch("exp-1", &load).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }
}
