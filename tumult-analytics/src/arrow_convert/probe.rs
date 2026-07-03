//! [`ProbeSamples`] → Arrow `RecordBatch` conversion.

use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use tumult_baseline::ProbeSamples;

use crate::error::AnalyticsError;

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

    // Pre-calculate total rows to avoid repeated reallocations.
    let total: usize = samples
        .iter()
        .map(|ps| ps.values.len().min(ps.sampled_at.len()))
        .sum();

    let mut probe_names: Vec<String> = Vec::with_capacity(total);
    let mut timestamps: Vec<i64> = Vec::with_capacity(total);
    let mut values: Vec<f64> = Vec::with_capacity(total);

    for ps in samples {
        // Clone the probe name once per ProbeSamples entry, not once per sample.
        let name = ps.name.clone();
        for (i, &value) in ps.values.iter().enumerate() {
            if let Some(&ts) = ps.sampled_at.get(i) {
                probe_names.push(name.clone());
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
