//! Journal → Arrow `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use arrow::array::Int32Array;
use tumult_baseline::ProbeSamples;
use tumult_core::types::{ActivityResult, Journal, LoadResult};

use crate::error::AnalyticsError;

#[must_use]
pub fn experiments_schema() -> Schema {
    Schema::new(vec![
        Field::new("experiment_id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("started_at_ns", DataType::Int64, false),
        Field::new("ended_at_ns", DataType::Int64, false),
        Field::new("duration_ms", DataType::UInt64, false),
        Field::new("method_step_count", DataType::Int64, false),
        Field::new("rollback_count", DataType::Int64, false),
        Field::new("hypothesis_before_met", DataType::Boolean, true),
        Field::new("hypothesis_after_met", DataType::Boolean, true),
        Field::new("estimate_accuracy", DataType::Float64, true),
        Field::new("resilience_score", DataType::Float64, true),
    ])
}

#[must_use]
pub fn activity_results_schema() -> Schema {
    Schema::new(vec![
        Field::new("experiment_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("activity_type", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("started_at_ns", DataType::Int64, false),
        Field::new("duration_ms", DataType::UInt64, false),
        Field::new("output", DataType::Utf8, true),
        Field::new("error", DataType::Utf8, true),
        Field::new("phase", DataType::Utf8, false),
    ])
}

/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn journal_to_experiment_batch(journal: &Journal) -> Result<RecordBatch, AnalyticsError> {
    let schema = Arc::new(experiments_schema());
    let hyp_before = journal.steady_state_before.as_ref().map(|h| h.met);
    let hyp_after = journal.steady_state_after.as_ref().map(|h| h.met);
    let accuracy = journal.analysis.as_ref().and_then(|a| a.estimate_accuracy);
    let resilience = journal.analysis.as_ref().and_then(|a| a.resilience_score);

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![journal.experiment_id.as_str()])) as ArrayRef,
            Arc::new(StringArray::from(vec![journal.experiment_title.as_str()])),
            Arc::new(StringArray::from(vec![journal.status.to_string()])),
            Arc::new(Int64Array::from(vec![journal.started_at_ns])),
            Arc::new(Int64Array::from(vec![journal.ended_at_ns])),
            Arc::new(UInt64Array::from(vec![journal.duration_ms])),
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            Arc::new(Int64Array::from(vec![journal.method_results.len() as i64])),
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            Arc::new(Int64Array::from(
                vec![journal.rollback_results.len() as i64],
            )),
            Arc::new(BooleanArray::from(vec![hyp_before])),
            Arc::new(BooleanArray::from(vec![hyp_after])),
            Arc::new(Float64Array::from(vec![accuracy])),
            Arc::new(Float64Array::from(vec![resilience])),
        ],
    )?)
}

/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn journal_to_activity_batch(journal: &Journal) -> Result<RecordBatch, AnalyticsError> {
    let schema = Arc::new(activity_results_schema());
    let mut exp_ids: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut types: Vec<String> = Vec::new();
    let mut statuses: Vec<String> = Vec::new();
    let mut started_ns: Vec<i64> = Vec::new();
    let mut durations: Vec<u64> = Vec::new();
    let mut outputs: Vec<Option<String>> = Vec::new();
    let mut errors: Vec<Option<String>> = Vec::new();
    let mut phases: Vec<String> = Vec::new();

    let mut push = |results: &[ActivityResult], phase: &str| {
        for r in results {
            exp_ids.push(journal.experiment_id.clone());
            names.push(r.name.clone());
            types.push(r.activity_type.to_string());
            statuses.push(r.status.to_string());
            started_ns.push(r.started_at_ns);
            durations.push(r.duration_ms);
            outputs.push(r.output.clone());
            errors.push(r.error.clone());
            phases.push(phase.to_string());
        }
    };

    if let Some(ref hyp) = journal.steady_state_before {
        push(&hyp.probe_results, "hypothesis_before");
    }
    push(&journal.method_results, "method");
    if let Some(ref hyp) = journal.steady_state_after {
        push(&hyp.probe_results, "hypothesis_after");
    }
    push(&journal.rollback_results, "rollback");

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(exp_ids)) as ArrayRef,
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(types)),
            Arc::new(StringArray::from(statuses)),
            Arc::new(Int64Array::from(started_ns)),
            Arc::new(UInt64Array::from(durations)),
            Arc::new(StringArray::from(outputs)),
            Arc::new(StringArray::from(errors)),
            Arc::new(StringArray::from(phases)),
        ],
    )?)
}

/// # Errors
///
/// Returns an error if any batch construction or concatenation fails.
pub fn journal_to_record_batch(
    journals: &[Journal],
) -> Result<(RecordBatch, RecordBatch), AnalyticsError> {
    if journals.is_empty() {
        return Ok((
            RecordBatch::new_empty(Arc::new(experiments_schema())),
            RecordBatch::new_empty(Arc::new(activity_results_schema())),
        ));
    }
    let mut exp_batches = Vec::with_capacity(journals.len());
    let mut act_batches = Vec::with_capacity(journals.len());
    for journal in journals {
        exp_batches.push(journal_to_experiment_batch(journal)?);
        let act = journal_to_activity_batch(journal)?;
        if act.num_rows() > 0 {
            act_batches.push(act);
        }
    }
    let exp = arrow::compute::concat_batches(&Arc::new(experiments_schema()), &exp_batches)?;
    let act = if act_batches.is_empty() {
        RecordBatch::new_empty(Arc::new(activity_results_schema()))
    } else {
        arrow::compute::concat_batches(&Arc::new(activity_results_schema()), &act_batches)?
    };
    Ok((exp, act))
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

/// Schema for a `RecordBatch` produced from [`ProbeSamples`].
///
/// Columns:
/// - `probe_name`: the probe identifier (`Utf8`)
/// - `timestamp_ns`: epoch-nanosecond sample timestamp (`Int64`)
/// - `value`: the observed numeric value (`Float64`)
#[must_use]
pub fn probe_samples_schema() -> Schema {
    Schema::new(vec![
        Field::new("probe_name", DataType::Utf8, false),
        Field::new("timestamp_ns", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
    ])
}

/// Convert a slice of [`ProbeSamples`] into an Arrow `RecordBatch`.
///
/// Each row in the resulting batch represents a single (probe, timestamp, value)
/// observation. Samples without corresponding `sampled_at` entries (i.e. where
/// `sampled_at` is shorter than `values`) are skipped silently.
///
/// # Errors
///
/// Returns an error if Arrow `RecordBatch` construction fails.
pub fn probe_samples_to_batch(samples: &[ProbeSamples]) -> Result<RecordBatch, AnalyticsError> {
    let schema = Arc::new(probe_samples_schema());

    let mut probe_names: Vec<String> = Vec::new();
    let mut timestamps: Vec<i64> = Vec::new();
    let mut values: Vec<f64> = Vec::new();

    for ps in samples {
        for (i, &value) in ps.values.iter().enumerate() {
            if let Some(&ts) = ps.sampled_at.get(i) {
                probe_names.push(ps.name.clone());
                timestamps.push(ts);
                values.push(value);
            }
        }
    }

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(probe_names)) as ArrayRef,
            Arc::new(Int64Array::from(timestamps)),
            Arc::new(Float64Array::from(values)),
        ],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Array;
    use tumult_baseline::ProbeSamples;
    use tumult_core::types::*;

    fn sample_journal() -> Journal {
        Journal {
            experiment_title: "DB failover test".into(),
            experiment_id: "test-001".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "kill-connections".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_135_000_000_000,
                duration_ms: 342,
                output: Some("done".into()),
                error: None,
                trace_id: "t1".into(),
                span_id: "s1".into(),
            }],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: Some(AnalysisResult {
                estimate_accuracy: Some(0.85),
                estimate_recovery_delta_s: None,
                trend: None,
                resilience_score: Some(0.92),
            }),
            regulatory: None,
        }
    }

    #[test]
    fn experiment_batch_schema() {
        let batch = journal_to_experiment_batch(&sample_journal()).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 12);
    }
    #[test]
    fn experiment_batch_values() {
        let batch = journal_to_experiment_batch(&sample_journal()).unwrap();
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids.value(0), "test-001");
    }
    #[test]
    fn activity_batch_method() {
        let batch = journal_to_activity_batch(&sample_journal()).unwrap();
        assert_eq!(batch.num_rows(), 1);
    }
    #[test]
    fn multiple_journals() {
        let (exp, act) = journal_to_record_batch(&[sample_journal(), sample_journal()]).unwrap();
        assert_eq!(exp.num_rows(), 2);
        assert_eq!(act.num_rows(), 2);
    }
    #[test]
    fn empty_journals() {
        let (exp, act) = journal_to_record_batch(&[]).unwrap();
        assert_eq!(exp.num_rows(), 0);
        assert_eq!(act.num_rows(), 0);
    }
    #[test]
    fn nullable_none() {
        let mut j = sample_journal();
        j.analysis = None;
        let batch = journal_to_experiment_batch(&j).unwrap();
        let acc = batch
            .column(10)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(acc.is_null(0));
    }

    // ── probe_samples_to_batch ─────────────────────────────────

    #[test]
    fn probe_samples_batch_schema() {
        let samples = vec![ProbeSamples {
            name: "latency".into(),
            values: vec![42.0, 43.0],
            errors: 0,
            total_attempts: 2,
            sampled_at: vec![1_000_000_000, 2_000_000_000],
        }];
        let batch = probe_samples_to_batch(&samples).unwrap();
        assert_eq!(batch.num_columns(), 3);
        assert_eq!(batch.schema().field(0).name(), "probe_name");
        assert_eq!(batch.schema().field(1).name(), "timestamp_ns");
        assert_eq!(batch.schema().field(2).name(), "value");
    }

    #[test]
    fn probe_samples_batch_values() {
        let samples = vec![
            ProbeSamples {
                name: "latency".into(),
                values: vec![10.0, 20.0],
                errors: 0,
                total_attempts: 2,
                sampled_at: vec![100, 200],
            },
            ProbeSamples {
                name: "errors".into(),
                values: vec![0.0],
                errors: 0,
                total_attempts: 1,
                sampled_at: vec![300],
            },
        ];
        let batch = probe_samples_to_batch(&samples).unwrap();
        assert_eq!(batch.num_rows(), 3);

        let names = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "latency");
        assert_eq!(names.value(2), "errors");

        let vals = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((vals.value(0) - 10.0).abs() < f64::EPSILON);
        assert!((vals.value(1) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn probe_samples_empty_sampled_at_produces_no_rows() {
        // When sampled_at is empty, no rows are emitted (timestamps missing).
        let samples = vec![ProbeSamples {
            name: "latency".into(),
            values: vec![42.0],
            errors: 0,
            total_attempts: 1,
            sampled_at: vec![],
        }];
        let batch = probe_samples_to_batch(&samples).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }
}
