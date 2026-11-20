//! The telemetry and the journals of real runs, kept with the tests.
//!
//! The directory `recorded` of this crate holds what the `tumult` binary
//! sent and wrote when it ran the experiments in `recorded/experiments` on
//! a lab: its OTLP trace, metric and log requests, one request a line, and
//! the journal of each run. The example `record_telemetry` of this crate
//! makes the recording.
//!
//! [`load`] puts the recording into a store the way the daemon does: each
//! request goes through the OTLP conversion and the writer of the lake, and
//! each journal is ingested with its experiment. A test that reads the store
//! then reads what a user of `tumult run` gets, and reads the same at every
//! run of the test.

use std::path::{Path, PathBuf};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use tumult_core::types::Journal;
use tumult_lake::Writer;

/// The trace requests of the recording, one a line.
pub const TRACES: &str = "traces.jsonl";
/// The metric requests of the recording, one a line.
pub const METRICS: &str = "metrics.jsonl";
/// The log requests of the recording, one a line.
pub const LOGS: &str = "logs.jsonl";
/// The directory of the experiments the recording ran.
pub const EXPERIMENTS: &str = "experiments";
/// The directory of the journals the runs wrote.
pub const JOURNALS: &str = "journals";

const DAY_NS: i64 = 86_400_000_000_000;

/// The start of the day after the last of the runs `journals`, in UTC: a
/// time to make a report of the recording at that no run of the test
/// changes.
///
/// # Panics
///
/// When there is no run.
#[must_use]
pub fn day_after(journals: &[Journal]) -> i64 {
    let last = journals.iter().map(|journal| journal.ended_at_ns).max();
    (last.expect("a run") / DAY_NS + 1) * DAY_NS
}

/// The directory of the recording.
#[must_use]
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("recorded")
}

/// The file of the journal of run `number` of the recording, which ran the
/// experiment `experiment`.
#[must_use]
pub fn journal_file(number: usize, experiment: &str) -> String {
    format!("{number:02}-{experiment}.journal.toon")
}

/// The experiment a journal file of the recording is of.
fn experiment_of(journal_file: &str) -> &str {
    journal_file
        .split_once('-')
        .and_then(|(_, rest)| rest.strip_suffix(".journal.toon"))
        .unwrap_or_else(|| panic!("{journal_file} is not a journal of the recording"))
}

/// The lines of a file of the recording.
fn lines(name: &str) -> Vec<String> {
    let path = dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines().map(str::to_string).collect()
}

/// The journal files of the recording, in the order of the runs.
fn journal_files() -> Vec<String> {
    let path = dir().join(JOURNALS);
    let entries = std::fs::read_dir(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut files: Vec<String> = entries
        .map(|entry| entry.expect("an entry").file_name())
        .map(|name| name.to_str().expect("a name in UTF-8").to_string())
        .collect();
    files.sort();
    files
}

/// Ingest the journal in `file` with the experiment it is a run of.
fn ingest(writer: &Writer, file: &str) -> Journal {
    let journal = tumult_core::journal::read_journal(&dir().join(JOURNALS).join(file))
        .unwrap_or_else(|e| panic!("{file}: {e}"));
    let definition = dir()
        .join(EXPERIMENTS)
        .join(format!("{}.toon", experiment_of(file)));
    let text = std::fs::read_to_string(&definition)
        .unwrap_or_else(|e| panic!("{}: {e}", definition.display()));
    let experiment = tumult_core::engine::parse_experiment(&text)
        .unwrap_or_else(|e| panic!("{}: {e}", definition.display()));
    let new = writer
        .ingest_journal(&journal, Some(&experiment))
        .unwrap_or_else(|e| panic!("{file}: {e}"));
    assert!(new, "{file}: the store already had this run");
    journal
}

/// Put the recording into the store of `writer`; the journals of the runs,
/// in the order of the runs.
///
/// # Panics
///
/// When a file of the recording cannot be read or the store refuses a row.
pub fn load(writer: &Writer) -> Vec<Journal> {
    let journals: Vec<Journal> = journal_files()
        .iter()
        .map(|file| ingest(writer, file))
        .collect();
    for line in lines(TRACES) {
        let request: ExportTraceServiceRequest =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{TRACES}: {e}"));
        writer
            .insert_spans(&tumult_otlp::trace_request_to_spans(&request))
            .unwrap_or_else(|e| panic!("{TRACES}: {e}"));
    }
    for line in lines(METRICS) {
        let request: ExportMetricsServiceRequest =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{METRICS}: {e}"));
        let rows = tumult_otlp::metrics_request_to_rows(&request);
        assert_eq!(rows.rejected_data_points, 0, "{METRICS}");
        writer
            .insert_metrics(&rows.sums, &rows.gauges, &rows.histograms)
            .unwrap_or_else(|e| panic!("{METRICS}: {e}"));
    }
    // A record without a time of its own gets the time the request came in.
    // For a recording that is the end of its last run.
    let received_at_ns = journals.iter().map(|j| j.ended_at_ns).max().unwrap_or(0);
    for line in lines(LOGS) {
        let request: ExportLogsServiceRequest =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{LOGS}: {e}"));
        writer
            .insert_logs(&tumult_otlp::logs_request_to_rows(&request, received_at_ns))
            .unwrap_or_else(|e| panic!("{LOGS}: {e}"));
    }
    journals
}
