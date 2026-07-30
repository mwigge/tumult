//! Manual import: CSV files and tumult journal JSON into the store.
//!
//! The journal mapping is deliberately simple for v1: the journal becomes an
//! experiment root span plus one span per method/rollback activity. Gaps are
//! marked `// TODO(journal):`.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tumult_lake::{ImportBatch, SpanRow, Writer};
use serde::Deserialize;

use crate::error::IngestError;

static BATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What happened in one manual import.
#[derive(Debug)]
pub struct ImportSummary {
    pub batch_id: String,
    pub format: &'static str,
    pub rows: usize,
}

/// Duck-typed slice of the tumult journal (`tumult_core::types::Journal`)
/// covering what v1 maps to spans.
#[derive(Debug, Deserialize)]
struct Journal {
    experiment_title: String,
    experiment_id: String,
    status: String,
    started_at_ns: i64,
    ended_at_ns: i64,
    #[serde(default)]
    method_results: Vec<JournalActivity>,
    #[serde(default)]
    rollback_results: Vec<JournalActivity>,
    // TODO(journal): map steady_state_before/after hypothesis verdicts,
    // analysis.resilience_score / estimate_accuracy, load_result and
    // regulatory evidence into spans/metrics.
}

#[derive(Debug, Deserialize)]
struct JournalActivity {
    name: String,
    activity_type: String,
    status: String,
    started_at_ns: i64,
    duration_ms: u64,
    #[serde(default)]
    trace_id: String,
    #[serde(default)]
    span_id: String,
}

/// Imports local files into the store through a [`Writer`].
pub struct ManualImporter<'a> {
    writer: &'a Writer,
}

impl<'a> ManualImporter<'a> {
    #[must_use]
    pub fn new(writer: &'a Writer) -> Self {
        Self { writer }
    }

    /// Import `path`, sniffing the format: a leading `{` means a tumult
    /// journal JSON document; anything else is tried as CSV with a header row.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read, parsed or written to the store.
    pub fn import_file(
        &self,
        path: &Path,
        label: Option<String>,
    ) -> Result<ImportSummary, IngestError> {
        let content = std::fs::read_to_string(path)?;
        let source = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );

        let trimmed = content.trim_start();
        let (format, rows) = if trimmed.starts_with('{') {
            ("json", self.import_journal(trimmed)?)
        } else if trimmed.lines().next().is_some_and(|l| l.contains(',')) {
            ("csv", self.import_csv(trimmed)?)
        } else {
            return Err(IngestError::UnknownFormat(source));
        };

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as i64);
        let batch_id = format!(
            "import-{nanos}-{}",
            BATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        self.writer.record_import_batch(&ImportBatch {
            id: batch_id.clone(),
            source,
            imported_at_ns: nanos,
            rows: i32::try_from(rows.len()).unwrap_or(i32::MAX),
            label,
        })?;
        self.writer.insert_spans(&rows)?;
        Ok(ImportSummary {
            batch_id,
            format,
            rows: rows.len(),
        })
    }

    /// tumult journal JSON → experiment span + activity spans.
    fn import_journal(&self, content: &str) -> Result<Vec<SpanRow>, IngestError> {
        let journal: Journal = serde_json::from_str(content)?;
        let trace_id = format!("journal-{}", journal.experiment_id);
        let root_span_id = format!("journal-{}-root", journal.experiment_id);

        let mut rows = vec![SpanRow {
            ts_ns: journal.started_at_ns,
            trace_id: trace_id.clone(),
            span_id: root_span_id.clone(),
            parent_span_id: None,
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: journal.ended_at_ns.saturating_sub(journal.started_at_ns),
            status_code: "Unset".into(),
            status_message: String::new(),
            service_name: "tumult".into(),
            service_version: None,
            experiment_id: Some(journal.experiment_id.clone()),
            experiment_name: Some(journal.experiment_title.clone()),
            outcome_status: Some(journal.status.clone()),
            ..SpanRow::default()
        }];

        let mut activities = journal.method_results;
        activities.extend(journal.rollback_results);
        for activity in activities {
            let span_name = match activity.activity_type.as_str() {
                "action" => "resilience.action".to_string(),
                "probe" => "resilience.probe".to_string(),
                // Rollback activities and future types keep their name.
                other => format!("resilience.{other}"),
            };
            rows.push(SpanRow {
                ts_ns: activity.started_at_ns,
                trace_id: if activity.trace_id.is_empty() {
                    trace_id.clone()
                } else {
                    activity.trace_id
                },
                span_id: if activity.span_id.is_empty() {
                    format!("journal-{}-{}", journal.experiment_id, activity.name)
                } else {
                    activity.span_id
                },
                parent_span_id: Some(root_span_id.clone()),
                span_name,
                span_kind: "Internal".into(),
                duration_ns: (activity.duration_ms as i64).saturating_mul(1_000_000),
                status_code: "Unset".into(),
                status_message: String::new(),
                service_name: "tumult".into(),
                service_version: None,
                experiment_id: Some(journal.experiment_id.clone()),
                experiment_name: Some(journal.experiment_title.clone()),
                outcome_status: Some(activity.status),
                span_attrs: vec![("journal.activity.name".into(), activity.name)],
                ..SpanRow::default()
            });
        }
        Ok(rows)
    }

    /// CSV with a header row → span rows. Known columns map onto the wide
    /// table; unknown columns land in `span_attrs` under their header name.
    fn import_csv(&self, content: &str) -> Result<Vec<SpanRow>, IngestError> {
        let mut reader = csv::Reader::from_reader(content.as_bytes());
        let headers: Vec<String> = reader.headers()?.iter().map(str::to_string).collect();
        let mut rows = Vec::new();
        for record in reader.records() {
            let record = record?;
            let mut row = SpanRow::default();
            for (header, value) in headers.iter().zip(record.iter()) {
                if value.is_empty() {
                    continue;
                }
                match header.as_str() {
                    "ts_ns" => row.ts_ns = value.parse().unwrap_or(0),
                    "trace_id" => row.trace_id = value.into(),
                    "span_id" => row.span_id = value.into(),
                    "parent_span_id" => row.parent_span_id = Some(value.into()),
                    "span_name" => row.span_name = value.into(),
                    "span_kind" => row.span_kind = value.into(),
                    "duration_ns" => row.duration_ns = value.parse().unwrap_or(0),
                    "status_code" => row.status_code = value.into(),
                    "status_message" => row.status_message = value.into(),
                    "service_name" => row.service_name = value.into(),
                    "service_version" => row.service_version = Some(value.into()),
                    "experiment_id" => row.experiment_id = Some(value.into()),
                    "experiment_name" => row.experiment_name = Some(value.into()),
                    "outcome_status" => row.outcome_status = Some(value.into()),
                    "fault_type" => row.fault_type = Some(value.into()),
                    "fault_subtype" => row.fault_subtype = Some(value.into()),
                    "fault_severity" => row.fault_severity = Some(value.into()),
                    "blast_radius" => row.blast_radius = Some(value.into()),
                    "target_system" => row.target_system = Some(value.into()),
                    "target_technology" => row.target_technology = Some(value.into()),
                    "target_environment" => row.target_environment = Some(value.into()),
                    "plugin_name" => row.plugin_name = Some(value.into()),
                    "hypothesis_met" => row.hypothesis_met = value.parse().ok(),
                    "recovery_time_s" => row.recovery_time_s = value.parse().ok(),
                    other => row.span_attrs.push((other.to_string(), value.into())),
                }
            }
            if row.span_name.is_empty() {
                row.span_name = "csv.import".into();
            }
            if row.span_kind.is_empty() {
                row.span_kind = "Internal".into();
            }
            if row.status_code.is_empty() {
                row.status_code = "Unset".into();
            }
            if row.events.is_empty() {
                row.events = "[]".into();
            }
            rows.push(row);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_lake::Store;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        (d, store)
    }

    #[test]
    fn journal_json_maps_to_spans() {
        let (_d, store) = temp_store();
        let writer = store.writer().unwrap();
        let importer = ManualImporter::new(&writer);
        let journal = serde_json::json!({
            "experiment_title": "pg-failover",
            "experiment_id": "exp-1",
            "status": "completed",
            "started_at_ns": 1_000_000_000_i64,
            "ended_at_ns": 301_000_000_000_i64,
            "method_results": [{
                "name": "kill-primary",
                "activity_type": "action",
                "status": "succeeded",
                "started_at_ns": 2_000_000_000_i64,
                "duration_ms": 500,
                "trace_id": "",
                "span_id": ""
            }],
            "rollback_results": []
        });
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("journal.json");
        std::fs::write(&path, journal.to_string()).unwrap();

        let summary = importer.import_file(&path, Some("manual".into())).unwrap();
        assert_eq!(summary.format, "json");
        assert_eq!(summary.rows, 2);

        let reader = store.read_only().unwrap();
        let runs = reader.experiment_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].experiment_id.as_deref(), Some("exp-1"));
        let spans = reader
            .query_json_rows(
                "SELECT span_name FROM spans WHERE span_name != 'resilience.experiment'",
            )
            .unwrap();
        assert_eq!(
            spans[0]["span_name"],
            serde_json::json!("resilience.action")
        );
        // The import batch is recorded.
        let batches = reader
            .query_json_rows("SELECT rows, label FROM import_batches")
            .unwrap();
        assert_eq!(batches[0]["rows"], serde_json::json!(2));
    }

    #[test]
    fn csv_maps_known_columns_and_keeps_rest() {
        let (_d, store) = temp_store();
        let writer = store.writer().unwrap();
        let importer = ManualImporter::new(&writer);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("spans.csv");
        std::fs::write(
            &path,
            "ts_ns,span_name,experiment_id,outcome_status,custom.note\n\
             1000000000,resilience.experiment,exp-2,deviated,hello\n",
        )
        .unwrap();

        let summary = importer.import_file(&path, None).unwrap();
        assert_eq!(summary.format, "csv");
        assert_eq!(summary.rows, 1);

        let reader = store.read_only().unwrap();
        let rows = reader
            .query_json_rows("SELECT outcome_status, span_attrs['custom.note'] AS note FROM spans")
            .unwrap();
        assert_eq!(rows[0]["outcome_status"], serde_json::json!("deviated"));
        assert_eq!(rows[0]["note"], serde_json::json!("hello"));
    }
}
