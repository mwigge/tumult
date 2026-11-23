//! Journal → `probe_samples` / `nfr_verdicts` `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_core::types::Journal;

use crate::error::AnalyticsError;

/// Schema of the `probe_samples` table: one row per raw probe observation.
#[must_use]
pub fn journal_probe_samples_schema() -> Schema {
    Schema::new(vec![
        Field::new("experiment_id", DataType::Utf8, false),
        Field::new("probe_name", DataType::Utf8, false),
        Field::new("phase", DataType::Utf8, false),
        Field::new("ts_ns", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
        Field::new("ok", DataType::Boolean, false),
        Field::new("reading", DataType::Float64, true),
    ])
}

/// Schema of the `nfr_verdicts` table: one row per evaluated objective.
#[must_use]
pub fn nfr_verdicts_schema() -> Schema {
    Schema::new(vec![
        Field::new("experiment_id", DataType::Utf8, false),
        Field::new("nfr_id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("operator", DataType::Utf8, false),
        Field::new("threshold", DataType::Float64, false),
        Field::new("observed", DataType::Float64, true),
        Field::new("state", DataType::Utf8, false),
        Field::new("margin", DataType::Float64, true),
        Field::new("criticality", DataType::Utf8, false),
    ])
}

/// The journal's raw probe samples as a `probe_samples` batch. `value` is
/// the probe duration in milliseconds, `reading` the number the probe
/// printed.
///
/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn journal_to_probe_samples_batch(journal: &Journal) -> Result<RecordBatch, AnalyticsError> {
    let n = journal.probe_samples.len();
    let mut ids = Vec::with_capacity(n);
    let mut names = Vec::with_capacity(n);
    let mut phases = Vec::with_capacity(n);
    let mut ts = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    let mut oks = Vec::with_capacity(n);
    let mut readings = Vec::with_capacity(n);
    for s in &journal.probe_samples {
        ids.push(journal.experiment_id.as_str());
        names.push(s.probe.as_str());
        phases.push(s.phase.to_string());
        ts.push(s.ts_ns);
        // u64 → f64: millisecond durations are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        values.push(s.duration_ms as f64);
        oks.push(s.ok);
        readings.push(s.reading);
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
            Arc::new(Float64Array::from(readings)),
        ],
    )?)
}

/// The journal's objective verdicts as an `nfr_verdicts` batch (empty when
/// the run declared no objectives).
///
/// # Errors
///
/// Returns an error if the Arrow `RecordBatch` construction fails.
pub fn journal_to_nfr_verdicts_batch(journal: &Journal) -> Result<RecordBatch, AnalyticsError> {
    let verdicts = journal
        .objectives
        .as_ref()
        .map_or(&[][..], |o| o.verdicts.as_slice());
    let n = verdicts.len();
    let mut ids = Vec::with_capacity(n);
    let mut nfr_ids = Vec::with_capacity(n);
    let mut kinds = Vec::with_capacity(n);
    let mut metrics = Vec::with_capacity(n);
    let mut ops = Vec::with_capacity(n);
    let mut thresholds = Vec::with_capacity(n);
    let mut observed = Vec::with_capacity(n);
    let mut states = Vec::with_capacity(n);
    let mut margins = Vec::with_capacity(n);
    let mut crits = Vec::with_capacity(n);
    for v in verdicts {
        ids.push(journal.experiment_id.as_str());
        nfr_ids.push(v.nfr_id.as_str());
        kinds.push(v.kind.to_string());
        metrics.push(v.metric.as_str());
        ops.push(v.operator.to_string());
        thresholds.push(v.threshold);
        observed.push(v.observed);
        states.push(v.state.to_string());
        margins.push(v.margin);
        crits.push(v.criticality.to_string());
    }
    Ok(RecordBatch::try_new(
        Arc::new(nfr_verdicts_schema()),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(nfr_ids)),
            Arc::new(StringArray::from(kinds)),
            Arc::new(StringArray::from(metrics)),
            Arc::new(StringArray::from(ops)),
            Arc::new(Float64Array::from(thresholds)),
            Arc::new(Float64Array::from(observed)),
            Arc::new(StringArray::from(states)),
            Arc::new(Float64Array::from(margins)),
            Arc::new(StringArray::from(crits)),
        ],
    )?)
}
