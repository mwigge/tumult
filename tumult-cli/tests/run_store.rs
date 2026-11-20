//! What `tumult run` leaves in the analytics store.
//!
//! The runs here are real: the binary runs `sh` steps on a state file and
//! writes each journal to a private store. The store is then read with
//! `tumult analyze`, as a user reads it.
#![cfg(unix)]

mod lab;

use lab::{experiment, heal, inject, leave_marker, reads, step, with_block, Lab};
use tumult_lake::lake::{committed_file, export, LakeConfig};
use tumult_lake::{Reader, Store};

/// Run `method` as the experiment `name` and store its journal; what the
/// binary printed.
fn run_into_the_store(lab: &Lab, name: &str, method: &str) -> String {
    let definition = format!("{name}.toon");
    std::fs::write(lab.path(&definition), experiment(method, &leave_marker())).expect("experiment");
    let journal = format!("{name}-journal.toon");
    let output = lab.tumult(&["run", &definition, "--journal-path", &journal]);
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The rows `tumult analyze --query` prints for `sql`, cell by cell.
fn rows(lab: &Lab, sql: &str) -> Vec<Vec<String>> {
    let output = lab.tumult(&["analyze", "--query", sql]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
        .lines()
        .skip_while(|line| !line.starts_with("---"))
        .skip(1)
        .take_while(|line| !line.is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

#[test]
fn the_store_holds_every_run_as_it_ended() {
    let lab = Lab::with_state("ok");
    let runs = [
        ("stays-up", [inject(), heal()].concat()),
        (
            "probe-fails",
            [inject(), reads("during", "ok"), heal()].concat(),
        ),
        ("fault-fails", step("inject", "action", "exit 3")),
        ("down-before", inject()),
    ];

    for (name, method) in &runs {
        // The last run finds its target down before the fault.
        let state = if *name == "down-before" { "down" } else { "ok" };
        std::fs::write(lab.path("state"), format!("{state}\n")).expect("state file");
        let stdout = run_into_the_store(&lab, name, method);
        assert!(
            stdout.contains("Ingested into persistent analytics store\n"),
            "{name}: {stdout}"
        );
    }

    let roots = rows(
        &lab,
        "SELECT outcome_status, hypothesis_met, journal_only, trace_id \
         FROM run_roots ORDER BY outcome_status",
    );
    assert_eq!(
        roots,
        [
            ["aborted", "false", "true", "NULL"],
            ["completed", "true", "true", "NULL"],
            ["deviated", "true", "true", "NULL"],
            ["failed", "true", "true", "NULL"],
        ]
    );
    let steps = rows(
        &lab,
        "SELECT name, status, phase FROM activity_results ORDER BY started_at_ns",
    );
    assert_eq!(
        steps,
        [
            // The target stays up.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "succeeded", "method"],
            ["heal", "succeeded", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            // A probe of the method does not hold.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "succeeded", "method"],
            ["during", "failed", "method"],
            ["heal", "succeeded", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            ["leave-marker", "succeeded", "rollback"],
            // The fault itself fails.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "failed", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            ["leave-marker", "succeeded", "rollback"],
            // The target is down before the fault.
            ["steady", "failed", "hypothesis_before"],
            ["leave-marker", "succeeded", "rollback"],
        ]
    );
}

/// The tables that hold the evidence of the verdict of a run.
const EVIDENCE: [&str; 2] = ["probe_samples", "nfr_verdicts"];

/// Every row of `table`, as JSON, in a stable order.
fn table_rows(reader: &Reader, table: &str) -> Vec<String> {
    let mut rows: Vec<String> = reader
        .query_json_rows(&format!("SELECT * FROM {table}"))
        .expect("the table is read")
        .iter()
        .map(ToString::to_string)
        .collect();
    rows.sort();
    rows
}

#[test]
fn the_archive_of_the_store_holds_the_samples_and_the_verdict_of_a_run() {
    let lab = Lab::with_state("ok");
    // The target is down for more than a second, so a sample of the run
    // sees it down.
    let method = [inject(), step("stay-down", "action", "sleep 1.2"), heal()].concat();
    run_into_the_store(&lab, "down-and-back", &method);
    let store = Store::open(&lab.path("lake.duckdb")).expect("the store of the run");
    let reader = store.read_only().expect("reader");
    let archive = LakeConfig::new(lab.path("archive"), 0);

    export(&reader, &archive).expect("export");

    let other = Store::open(&lab.path("restored.duckdb")).expect("a new store");
    let restore = other.writer().expect("writer");
    for table in EVIDENCE {
        let file = committed_file(&archive, table).expect("the table is in the archive");
        restore
            .execute(
                &format!(
                    "INSERT INTO {table} BY NAME SELECT * FROM read_parquet('{}')",
                    file.display()
                ),
                [],
            )
            .expect("the archive is read");
    }
    let restored = other.read_only().expect("reader");
    for table in EVIDENCE {
        let stored = table_rows(&reader, table);
        assert!(!stored.is_empty(), "the run left no {table}");
        assert_eq!(table_rows(&restored, table), stored, "{table}");
    }
    let seen = restored
        .query_json_rows("SELECT DISTINCT phase, ok FROM probe_samples ORDER BY ALL")
        .expect("samples");
    assert!(
        seen.contains(&serde_json::json!({"phase": "during", "ok": false})),
        "no sample saw the target down: {seen:?}"
    );
    assert!(
        seen.contains(&serde_json::json!({"phase": "post", "ok": true})),
        "no sample saw the target back: {seen:?}"
    );
    let verdicts = restored
        .query_json_rows("SELECT nfr_id, state FROM nfr_verdicts")
        .expect("verdicts");
    assert_eq!(
        verdicts,
        [serde_json::json!({"nfr_id": "rto", "state": "met"})]
    );
}

/// The target a definition declares.
const TARGET: &str = "target:
  system: lab-target
  technology: file
  environment: lab
";

#[test]
fn a_run_without_a_trace_is_placed_by_the_target_of_its_definition() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    let plain = experiment(&method, &leave_marker());
    let on_a_target = with_block(&plain, TARGET).replacen("Probe tolerance", "On a target", 1);
    for (name, definition) in [("plain", &plain), ("on-a-target", &on_a_target)] {
        let file = format!("{name}.toon");
        std::fs::write(lab.path(&file), definition).expect("experiment");
        let journal = format!("{name}-journal.toon");
        let output = lab.tumult(&["run", &file, "--journal-path", &journal]);
        assert_eq!(output.status.code(), Some(0), "{name}");
    }

    let roots = rows(
        &lab,
        "SELECT experiment_name, target_system, target_technology, target_environment \
         FROM run_roots ORDER BY experiment_name",
    );

    assert_eq!(
        roots,
        [
            ["On a target", "lab-target", "file", "lab"],
            ["Probe tolerance", "NULL", "NULL", "NULL"],
        ]
    );
}

#[test]
fn a_run_that_writes_to_clickhouse_warns_once_that_clickhouse_is_deprecated() {
    let lab = Lab::with_state("ok");
    // No ClickHouse answers here: the listener takes a connection and closes it.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("loopback listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            drop(connection);
        }
    });
    let method = [inject(), heal()].concat();
    std::fs::write(
        lab.path("clickhouse.toon"),
        experiment(&method, &leave_marker()),
    )
    .expect("experiment");

    let output = lab
        .command(&[
            "run",
            "clickhouse.toon",
            "--journal-path",
            "clickhouse-journal.toon",
        ])
        .env("TUMULT_CLICKHOUSE_URL", &url)
        .output()
        .expect("run tumult");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let warnings: Vec<serde_json::Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["event.name"] == "cli.clickhouse.deprecated")
        .collect();
    assert_eq!(warnings.len(), 1, "{stderr}");
    assert_eq!(warnings[0]["level"], "WARN");
    let message = warnings[0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("3.1"), "{message}");
}

#[test]
fn a_run_in_baseline_mode_skip_runs_and_is_stored_as_a_full_run() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    std::fs::write(lab.path("skip.toon"), experiment(&method, &leave_marker()))
        .expect("experiment");

    let output = lab.tumult(&[
        "run",
        "skip.toon",
        "--journal-path",
        "skip-journal.toon",
        "--baseline-mode",
        "skip",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("Ingested into persistent analytics store\n"),
        "{stdout}"
    );
    let journal = tumult_core::journal::read_journal(&lab.path("skip-journal.toon"))
        .expect("the journal of the run");
    let steps: Vec<&str> = journal
        .method_results
        .iter()
        .map(|result| result.name.as_str())
        .collect();
    assert_eq!(steps, ["inject", "heal"]);
}
