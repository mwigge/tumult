//! `tumultd report --template dr-test|dr-exercise`: the DR reports of the
//! lake from the command line, written to a file, with the document id,
//! the SHA-256 of the file and the time it was built on stdout.
//!
//! Each test seeds a private lake with a DR run of `ledger` stored as its
//! journal and a verified failover of `ledger`, then runs the binary
//! against it with a private HOME and no org file.

use std::path::Path;
use std::process::{Command, Output};

use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, ObjectiveOutcome, TestKind,
};
use tumult_lake::{ExerciseDetail, ExerciseType, ManualOutcome, NewManualExperiment, Store};

const S: i64 = 1_000_000_000;
const DAY: i64 = 86_400 * S;

fn now_ns() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(since.as_nanos()).unwrap()
}

/// A lake in `dir` with the DR run `ledger-dr` of 10 days ago and a
/// failover of `ledger` of 5 days ago. Returns the id of the failover.
fn seed(dir: &Path) -> String {
    let store = Store::open(&dir.join("lake.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let now = now_ns();
    let mut journal =
        tumult_lake::duckdb_store::sample_journal("ledger-dr", ExperimentStatus::Completed);
    journal.kind = TestKind::Dr;
    journal.experiment_title = "ledger promote standby".into();
    journal.started_at_ns = now - 10 * DAY;
    journal.ended_at_ns = journal.started_at_ns + 60 * S;
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: Some(4.0),
        ..ObjectiveOutcome::default()
    });
    let definition = Experiment {
        title: journal.experiment_title.clone(),
        target: Some(ExperimentTarget {
            system: "ledger".into(),
            technology: None,
            environment: Some("prod".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    assert!(writer.ingest_journal(&journal, Some(&definition)).unwrap());
    let id = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "ledger failover".into(),
            exercise_type: ExerciseType::Failover,
            executed_at_ns: now - 5 * DAY,
            hypothesis: "The standby takes the writes within its RTO".into(),
            method: "Stop the primary and promote the standby".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: Some(true),
            findings: None,
            action_items: vec![],
            target_system: Some("ledger".into()),
            target_environment: Some("prod".into()),
            blast_radius: None,
            recovery_time_s: Some(600.0),
            duration_s: None,
            entered_by: "carol".into(),
            attestation: "I attest this record reflects the exercise as executed.".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
            exercise: ExerciseDetail {
                rto_target_s: Some(900.0),
                ..ExerciseDetail::default()
            },
        })
        .unwrap();
    writer.submit_manual(&id, None, "carol").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    id
}

/// `tumultd report` with `args` against the lake in `dir`.
fn report(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tumultd"))
        .arg("report")
        .args(args)
        .env("TUMULT_LAKE_PATH", dir.join("lake.duckdb"))
        .env("HOME", dir)
        .env("KRONIKA_ORG_FILE", dir.join("org.yaml"))
        .env_remove("KRONIKA_MANIFESTS_DIR")
        .env_remove("KRONIKA_DB")
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .output()
        .expect("run tumultd")
}

/// The value after `label` on its line of `stdout`.
fn line<'a>(stdout: &'a str, label: &str) -> &'a str {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(label))
        .unwrap_or_else(|| panic!("no line {label:?} in {stdout}"))
}

fn sha256_of(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let bytes = std::fs::read(path).unwrap();
    Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

#[test]
fn a_dr_test_report_is_written_as_a_pdf_and_states_its_sha256() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let out = dir.path().join("ledger.pdf");

    let output = report(
        dir.path(),
        &[
            "--template",
            "dr-test",
            "--target",
            "ledger",
            "--out",
            out.to_str().unwrap(),
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(line(&stdout, "Report ").starts_with("KRK-R4-"), "{stdout}");
    assert_eq!(line(&stdout, "SHA-256: "), sha256_of(&out), "{stdout}");
    assert!(line(&stdout, "Generated: ").ends_with(" UTC"), "{stdout}");
    assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF"));
}

#[test]
fn a_dr_exercise_report_is_written_as_html() {
    let dir = tempfile::tempdir().unwrap();
    let record = seed(dir.path());
    let out = dir.path().join("failover.html");

    let output = report(
        dir.path(),
        &[
            "--template",
            "dr-exercise",
            "--record",
            &record,
            "--format",
            "html",
            "--out",
            out.to_str().unwrap(),
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(line(&stdout, "Report ").starts_with("KRK-R5-"), "{stdout}");
    assert_eq!(line(&stdout, "SHA-256: "), sha256_of(&out));
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.contains(&record), "{html}");
    assert!(
        html.contains("DR exercise report: ledger failover"),
        "{html}"
    );
}

#[test]
fn a_dr_report_of_a_record_that_is_not_there_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let out = dir.path().join("none.pdf");

    let output = report(
        dir.path(),
        &[
            "--template",
            "dr-exercise",
            "--record",
            "no-such-record",
            "--out",
            out.to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no-such-record"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn a_dr_test_report_needs_a_target_and_a_file() {
    let dir = tempfile::tempdir().unwrap();

    let output = report(dir.path(), &["--template", "dr-test", "--out", "x.pdf"]);
    assert_eq!(output.status.code(), Some(2), "a usage error");
    let output = report(dir.path(), &["--template", "dr-test", "--target", "ledger"]);
    assert_eq!(output.status.code(), Some(2), "a usage error");
}
