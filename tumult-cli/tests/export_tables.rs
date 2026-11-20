//! What `tumult export` writes for the journal of a run.
//!
//! Each test runs a real experiment against the lab, exports the journal of
//! that run with the binary, and reads the files back with a reader of
//! their format.
#![cfg(unix)]

mod lab;

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, Int64Array, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use lab::{heal, inject, step, Lab};
use tumult_core::journal::write_journal;
use tumult_core::types::{ActivityResult, Journal};

/// The columns of a file of steps, in the order of the file.
const STEP_COLUMNS: [&str; 9] = [
    "experiment_id",
    "name",
    "activity_type",
    "status",
    "started_at_ns",
    "duration_ms",
    "output",
    "error",
    "phase",
];

/// What the run of the lab did, step by step: the phase, the name, the kind
/// and the status of each step.
const RAN: [[&str; 4]; 6] = [
    ["hypothesis_before", "steady", "probe", "succeeded"],
    ["method", "inject", "action", "succeeded"],
    ["method", "complain", "action", "failed"],
    ["method", "heal", "action", "succeeded"],
    ["hypothesis_after", "steady", "probe", "succeeded"],
    ["rollback", "leave-marker", "action", "succeeded"],
];

/// One row of a file of steps.
#[derive(Debug, PartialEq)]
struct Step {
    experiment_id: String,
    phase: String,
    name: String,
    activity_type: String,
    status: String,
    started_at_ns: i64,
    duration_ms: u64,
    output: Option<String>,
    error: Option<String>,
}

/// A lab whose run broke the target, had a step fail after 50 ms and healed
/// the target; the journal of that run.
fn lab_with_a_run() -> (Lab, Journal) {
    let lab = Lab::with_state("ok");
    let complain = step(
        "complain",
        "action",
        "sleep 0.05; echo no, such >&2; exit 3",
    );
    let (code, journal) = lab.run(&[inject(), complain, heal()].concat());
    assert_eq!(code, Some(1), "{journal:?}");
    (lab, journal)
}

/// Export the journal of the lab; what the binary printed.
fn export(lab: &Lab, format: &str) -> String {
    export_file(lab, "journal.toon", format)
}

/// Export the journal `journal` of the lab; what the binary printed.
fn export_file(lab: &Lab, journal: &str, format: &str) -> String {
    let output = lab.tumult(&["export", journal, "--format", format]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The results of the journal in the order a file of steps has them.
fn results(journal: &Journal) -> Vec<&ActivityResult> {
    let before = journal.steady_state_before.iter();
    let after = journal.steady_state_after.iter();
    before
        .flat_map(|hypothesis| &hypothesis.probe_results)
        .chain(&journal.method_results)
        .chain(after.flat_map(|hypothesis| &hypothesis.probe_results))
        .chain(&journal.rollback_results)
        .collect()
}

/// The rows a file of steps must have for the run of the lab: what the run
/// did, with the times and the texts its journal has.
fn steps_of(journal: &Journal) -> Vec<Step> {
    let results = results(journal);
    assert_eq!(results.len(), RAN.len(), "{journal:?}");
    RAN.iter()
        .zip(results)
        .map(|([phase, name, kind, status], result)| Step {
            experiment_id: journal.experiment_id.clone(),
            phase: (*phase).to_string(),
            name: (*name).to_string(),
            activity_type: (*kind).to_string(),
            status: (*status).to_string(),
            started_at_ns: result.started_at_ns,
            duration_ms: result.duration_ms,
            output: result.output.clone(),
            error: result.error.clone(),
        })
        .collect()
}

fn column<'batch, T: Array + 'static>(batch: &'batch RecordBatch, name: &str) -> &'batch T {
    let values = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("no column {name}"));
    values
        .as_any()
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("column {name} is a {}", values.data_type()))
}

fn text(batch: &RecordBatch, name: &str, row: usize) -> Option<String> {
    let values: &StringArray = column(batch, name);
    values.is_valid(row).then(|| values.value(row).to_string())
}

fn needed(batch: &RecordBatch, name: &str, row: usize) -> String {
    text(batch, name, row).unwrap_or_else(|| panic!("row {row} has no {name}"))
}

fn step_in(batch: &RecordBatch, row: usize) -> Step {
    Step {
        experiment_id: needed(batch, "experiment_id", row),
        phase: needed(batch, "phase", row),
        name: needed(batch, "name", row),
        activity_type: needed(batch, "activity_type", row),
        status: needed(batch, "status", row),
        started_at_ns: column::<Int64Array>(batch, "started_at_ns").value(row),
        duration_ms: column::<UInt64Array>(batch, "duration_ms").value(row),
        output: text(batch, "output", row),
        error: text(batch, "error", row),
    }
}

fn column_names(batch: &RecordBatch) -> Vec<String> {
    let schema = batch.schema();
    schema.fields().iter().map(|f| f.name().clone()).collect()
}

/// The rows of a file of steps. A file whose columns are not those of a
/// file of steps fails the test.
fn steps_in(batches: &[RecordBatch]) -> Vec<Step> {
    let mut steps = Vec::new();
    for batch in batches {
        assert_eq!(column_names(batch), STEP_COLUMNS);
        steps.extend((0..batch.num_rows()).map(|row| step_in(batch, row)));
    }
    steps
}

/// The ids of the runs in a file of runs.
fn run_ids(batches: &[RecordBatch]) -> Vec<String> {
    let mut ids = Vec::new();
    for batch in batches {
        ids.extend((0..batch.num_rows()).map(|row| needed(batch, "experiment_id", row)));
    }
    ids
}

fn parquet(path: &Path) -> Vec<RecordBatch> {
    tumult_lake::export::import_parquet(path)
        .unwrap_or_else(|e| panic!("{} is not Parquet: {e}", path.display()))
}

fn arrow_reader(path: &Path) -> arrow::ipc::reader::FileReader<File> {
    let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    arrow::ipc::reader::FileReader::try_new(file, None).expect("an Arrow file")
}

fn arrow_file(path: &Path) -> Vec<RecordBatch> {
    arrow_reader(path)
        .map(|batch| batch.expect("a batch"))
        .collect()
}

/// A CSV file of steps, read as text, whole numbers and text that may be
/// missing.
fn csv_steps(path: &Path) -> Vec<RecordBatch> {
    let text = |name: &str, nullable: bool| Field::new(name, DataType::Utf8, nullable);
    let schema = Schema::new(vec![
        text("experiment_id", false),
        text("name", false),
        text("activity_type", false),
        text("status", false),
        Field::new("started_at_ns", DataType::Int64, false),
        Field::new("duration_ms", DataType::UInt64, false),
        text("output", true),
        text("error", true),
        text("phase", false),
    ]);
    let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let reader = arrow::csv::ReaderBuilder::new(Arc::new(schema))
        .with_header(true)
        .build(file)
        .expect("a CSV file");
    reader.map(|batch| batch.expect("a batch")).collect()
}

#[test]
fn a_parquet_export_writes_the_steps_of_the_run_next_to_the_run() {
    let (lab, journal) = lab_with_a_run();

    export(&lab, "parquet");

    let steps = steps_in(&parquet(&lab.path("activities/journal.parquet")));
    assert_eq!(steps, steps_of(&journal));
    assert_eq!(steps[0].output.as_deref(), Some("ok"));
    assert_eq!(steps[2].error.as_deref(), Some("no, such"));
    assert!(steps[2].duration_ms >= 50, "{:?}", steps[2]);
    let runs = parquet(&lab.path("journal.parquet"));
    assert_eq!(run_ids(&runs), [journal.experiment_id]);
}

#[test]
fn an_arrow_export_writes_the_steps_of_the_run_next_to_the_run() {
    let (lab, journal) = lab_with_a_run();

    export(&lab, "arrow");

    let steps = steps_in(&arrow_file(&lab.path("activities/journal.arrow")));
    assert_eq!(steps, steps_of(&journal));
    let runs = arrow_file(&lab.path("journal.arrow"));
    assert_eq!(run_ids(&runs), [journal.experiment_id]);
}

#[test]
fn a_csv_export_writes_the_steps_of_the_run_next_to_the_run() {
    let (lab, journal) = lab_with_a_run();

    export(&lab, "csv");

    let path = lab.path("activities/journal.csv");
    let written = std::fs::read_to_string(&path).expect("the file of steps");
    assert_eq!(
        written.lines().next(),
        Some(STEP_COLUMNS.join(",").as_str())
    );
    let steps = steps_in(&csv_steps(&path));
    assert_eq!(steps, steps_of(&journal));
    // The text of an error keeps its comma.
    assert_eq!(steps[2].error.as_deref(), Some("no, such"));
    let runs = std::fs::read_to_string(lab.path("journal.csv")).expect("the file of the run");
    let rows: Vec<&str> = runs.lines().skip(1).collect();
    assert_eq!(rows.len(), 1, "{runs}");
    let start = format!("{},Probe tolerance,failed,", journal.experiment_id);
    assert!(rows[0].starts_with(&start), "{runs}");
}

#[test]
fn an_export_says_which_files_it_wrote() {
    let (lab, _) = lab_with_a_run();

    let said = export(&lab, "parquet");

    assert_eq!(
        said,
        "Exported to: journal.parquet\nExported to: activities/journal.parquet\n"
    );
}

#[test]
fn a_journal_without_steps_exports_a_file_of_steps_without_rows() {
    let (lab, mut journal) = lab_with_a_run();
    journal.steady_state_before = None;
    journal.steady_state_after = None;
    journal.method_results.clear();
    journal.rollback_results.clear();
    write_journal(&journal, &lab.path("journal.toon")).expect("write the journal");

    export(&lab, "arrow");

    // The file is there with its columns, so a reader finds both files for
    // every run.
    let path = lab.path("activities/journal.arrow");
    assert_eq!(steps_in(&arrow_file(&path)), []);
    let schema = arrow_reader(&path).schema();
    let columns: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(columns, STEP_COLUMNS);
}

#[test]
fn a_json_export_writes_the_whole_journal_in_one_file() {
    let (lab, journal) = lab_with_a_run();

    let said = export(&lab, "json");

    assert_eq!(said, "Exported to: journal.json\n");
    let written = std::fs::read_to_string(lab.path("journal.json")).expect("the JSON file");
    // One key on each line, for a person to read.
    assert!(written.starts_with("{\n  \""), "{written}");
    let read: Journal = serde_json::from_str(&written).expect("a journal");
    assert_eq!(read, journal);
    assert!(!lab.path("activities").exists());
}

/// The Parquet files of `dir` itself, in the order of their names.
fn parquet_files(dir: &Path) -> Vec<RecordBatch> {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let mut files: Vec<_> = entries
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "parquet"))
        .collect();
    files.sort();
    files.iter().flat_map(|file| parquet(file)).collect()
}

#[test]
fn the_exports_of_two_runs_read_as_one_table_of_runs_and_one_of_steps() {
    let (lab, first) = lab_with_a_run();
    std::fs::rename(lab.path("journal.toon"), lab.path("first.toon")).expect("keep the journal");
    let (code, second) = lab.run(&[inject(), heal()].concat());
    assert_eq!(code, Some(0), "{second:?}");
    std::fs::rename(lab.path("journal.toon"), lab.path("second.toon")).expect("keep the journal");

    export_file(&lab, "first.toon", "parquet");
    export_file(&lab, "second.toon", "parquet");

    // Every Parquet file of a directory is one table: the runs.
    let runs = parquet_files(&lab.path(""));
    assert_eq!(
        run_ids(&runs),
        [first.experiment_id.clone(), second.experiment_id.clone()]
    );
    // And the steps, each with the id of its run, in the directory of steps.
    let steps = steps_in(&parquet_files(&lab.path("activities")));
    let of_run = |id: &str| steps.iter().filter(|step| step.experiment_id == id).count();
    assert_eq!(of_run(&first.experiment_id), 6);
    // The second run completed, so it had nothing to roll back.
    assert_eq!(of_run(&second.experiment_id), 4);
    assert_eq!(steps.len(), 10);
}
