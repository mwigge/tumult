//! Journal → Arrow RecordBatch conversion.

use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_core::types::{ActivityResult, Journal};

use crate::error::AnalyticsError;

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
            Arc::new(StringArray::from(vec![format!("{:?}", journal.status)])),
            Arc::new(Int64Array::from(vec![journal.started_at_ns])),
            Arc::new(Int64Array::from(vec![journal.ended_at_ns])),
            Arc::new(UInt64Array::from(vec![journal.duration_ms])),
            Arc::new(Int64Array::from(vec![journal.method_results.len() as i64])),
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
            types.push(format!("{:?}", r.activity_type));
            statuses.push(format!("{:?}", r.status));
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Array;
    use tumult_core::types::*;

    fn sample_journal() -> Journal {
        Journal {
            experiment_title: "DB failover test".into(),
            experiment_id: "test-001".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1774980000000000000,
            ended_at_ns: 1774980300000000000,
            duration_ms: 300000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "kill-connections".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1774980135000000000,
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
}
