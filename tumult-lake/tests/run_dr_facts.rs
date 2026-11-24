//! Schema v24: the lake keeps what a DR run measured, as its journal says
//! it. A run keeps whether it saw an outage, when the outage started, when
//! the target was back and the time to recovery. A verdict keeps whether
//! the run recovered and why it is what it is. A lake that v21, v22 or v23
//! left opens with the columns added and its rows kept.

#![cfg(feature = "duckdb")]

use serde_json::{json, Value};
use tumult_core::types::{
    ExperimentStatus, Journal, NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict,
    ObjectiveOutcome, PostResult,
};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::{Store, CURRENT_SCHEMA_VERSION};

const S: i64 = 1_000_000_000;
/// The end of the method of every run here.
const METHOD_END_NS: i64 = 1_774_980_140_000_000_000;

/// What the post phase of a run saw: the outage `outage_s` seconds before
/// the end of the method, and the target back `back_s` seconds after it.
fn post(outage_s: Option<i64>, back_s: Option<i64>) -> PostResult {
    let outage_started_at_ns = outage_s.map(|s| METHOD_END_NS - s * S);
    let recovered_at_ns = back_s.map(|s| METHOD_END_NS + s * S);
    #[allow(clippy::cast_precision_loss)]
    let mttr_s = outage_started_at_ns
        .zip(recovered_at_ns)
        .map(|(from, to)| (to - from) as f64 / 1e9);
    PostResult {
        started_at_ns: METHOD_END_NS,
        ended_at_ns: METHOD_END_NS + 30 * S,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 29.1,
        full_recovery: back_s.is_some() || outage_s.is_none(),
        mttr_s,
        impact_observed: Some(outage_s.is_some()),
        outage_started_at_ns,
        recovered_at_ns,
    }
}

/// The verdict on an RTO of 10 s.
fn rto(observed: Option<f64>, state: NfrState, recovered: Option<bool>) -> NfrVerdict {
    NfrVerdict {
        nfr_id: "rto".into(),
        kind: NfrKind::Rto,
        metric: "recovery.mttr_s".into(),
        operator: NfrOperator::Lte,
        threshold: 10.0,
        observed,
        state,
        margin: None,
        criticality: NfrCriticality::Blocking,
        recovered,
        reason: None,
    }
}

/// A DR run with an RTO of 10 s that saw `post` and got `verdict`.
fn dr_run(id: &str, status: ExperimentStatus, post: PostResult, verdict: NfrVerdict) -> Journal {
    let mut journal = sample_journal(id, status);
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: post.mttr_s,
        verdicts: vec![verdict],
        ..ObjectiveOutcome::default()
    });
    journal.post_result = Some(post);
    journal
}

/// Out 1 s before the end of the method, back 3 s after it: 4 s.
fn recovered() -> Journal {
    let verdict = rto(Some(4.0), NfrState::Met, Some(true));
    dr_run(
        "recovered",
        ExperimentStatus::Completed,
        post(Some(1), Some(3)),
        verdict,
    )
}

/// Out 1 s before the end of the method, and never back.
fn never_back() -> Journal {
    let verdict = rto(Some(30.1), NfrState::Breached, Some(false));
    dr_run(
        "never-back",
        ExperimentStatus::Deviated,
        post(Some(1), None),
        verdict,
    )
}

/// A fault the probes never saw.
fn no_outage() -> Journal {
    let verdict = NfrVerdict {
        reason: Some(NfrVerdict::NO_OUTAGE_OBSERVED.into()),
        ..rto(None, NfrState::Inconclusive, None)
    };
    dr_run(
        "no-outage",
        ExperimentStatus::Completed,
        post(None, None),
        verdict,
    )
}

/// A run that declares no objective, out 2 s before the end of the method
/// and back 4 s after it: 6 s.
fn without_objectives() -> Journal {
    let mut journal = sample_journal("plain", ExperimentStatus::Completed);
    journal.post_result = Some(post(Some(2), Some(4)));
    journal
}

fn temp_store() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn ingest(store: &Store, journals: &[Journal]) {
    let writer = store.writer().unwrap();
    for journal in journals {
        assert!(writer.ingest_journal(journal, None).unwrap());
    }
}

fn rows(store: &Store, sql: &str) -> Vec<Value> {
    store.read_only().unwrap().query_json_rows(sql).unwrap()
}

/// The row of the run `id` in `sql`, which names the run by `$1`.
fn row_of(store: &Store, sql: &str, id: &str) -> Value {
    let rows = store
        .read_only()
        .unwrap()
        .query_json_rows_bound(sql, &[id])
        .unwrap();
    assert_eq!(rows.len(), 1, "{id}: {rows:?}");
    rows[0].clone()
}

const RUN_FACTS: &str = "SELECT impact_observed, outage_started_at_ns, recovered_at_ns, mttr_s \
     FROM experiments WHERE experiment_id = $1";
const VERDICT_FACTS: &str =
    "SELECT state, observed, recovered, reason FROM nfr_verdicts WHERE experiment_id = $1";
const ROOT: &str = "SELECT recovery_time_s, journal_only FROM run_roots WHERE experiment_id = $1";

fn columns_of(store: &Store, table: &str) -> Vec<String> {
    store
        .read_only()
        .unwrap()
        .query_json_rows_bound(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = $1 ORDER BY ordinal_position",
            &[table],
        )
        .unwrap()
        .iter()
        .map(|row| row["column_name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_run_keeps_the_outage_it_saw_and_its_verdict_whether_it_recovered_and_why() {
    let (_d, store) = temp_store();

    ingest(&store, &[recovered(), never_back(), no_outage()]);

    assert_eq!(
        row_of(&store, RUN_FACTS, "recovered"),
        json!({
            "impact_observed": true,
            "outage_started_at_ns": METHOD_END_NS - S,
            "recovered_at_ns": METHOD_END_NS + 3 * S,
            "mttr_s": 4.0,
        })
    );
    assert_eq!(
        row_of(&store, VERDICT_FACTS, "recovered"),
        json!({"state": "met", "observed": 4.0, "recovered": true, "reason": null})
    );
    assert_eq!(
        row_of(&store, RUN_FACTS, "never-back"),
        json!({
            "impact_observed": true,
            "outage_started_at_ns": METHOD_END_NS - S,
            "recovered_at_ns": null,
            "mttr_s": null,
        })
    );
    assert_eq!(
        row_of(&store, VERDICT_FACTS, "never-back"),
        json!({"state": "breached", "observed": 30.1, "recovered": false, "reason": null})
    );
    assert_eq!(
        row_of(&store, RUN_FACTS, "no-outage"),
        json!({
            "impact_observed": false,
            "outage_started_at_ns": null,
            "recovered_at_ns": null,
            "mttr_s": null,
        })
    );
    assert_eq!(
        row_of(&store, VERDICT_FACTS, "no-outage"),
        json!({
            "state": "inconclusive",
            "observed": null,
            "recovered": null,
            "reason": "no outage observed",
        })
    );
}

#[test]
fn a_run_without_objectives_keeps_its_recovery_time_in_run_roots() {
    let (_d, store) = temp_store();

    ingest(&store, &[without_objectives(), recovered(), never_back()]);

    assert_eq!(
        row_of(&store, ROOT, "plain"),
        json!({"recovery_time_s": 6.0, "journal_only": true})
    );
    assert_eq!(
        row_of(&store, ROOT, "recovered"),
        json!({"recovery_time_s": 4.0, "journal_only": true})
    );
    assert_eq!(
        row_of(&store, ROOT, "never-back"),
        json!({"recovery_time_s": null, "journal_only": true})
    );
    let mean = rows(
        &store,
        &format!(
            "SELECT {} AS v FROM run_roots AS s",
            tumult_lake::mean_recovery_time_sql("s")
        ),
    );
    assert_eq!(mean[0]["v"], json!(5.0));
}

/// `experiments` and `nfr_verdicts` as v16 to v23 left them, each with one
/// row of a run stored only as a journal, and the version set back to
/// `version`. Before v23 `probe_samples` has no `reading` either.
fn before_the_dr_facts(version: i64) -> Vec<String> {
    let mut sql: Vec<String> = vec![
        "DROP VIEW experiment_runs".into(),
        "DROP VIEW run_roots".into(),
        "DROP TABLE experiments".into(),
        "CREATE TABLE experiments (
            experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
            status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
            ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
            method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
            hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
            estimate_accuracy DOUBLE, resilience_score DOUBLE,
            test_kind VARCHAR, rto_target_s DOUBLE, rpo_target_s DOUBLE,
            rto_measured_s DOUBLE, rpo_measured_s DOUBLE,
            target_version VARCHAR, version_source VARCHAR, version_shares VARCHAR
        )"
        .into(),
        "CREATE UNIQUE INDEX idx_experiments_id ON experiments (experiment_id)".into(),
        "INSERT INTO experiments VALUES ('old-1', 'Old', 'completed', 0, 1, 1, 1, 0, \
         true, true, NULL, NULL, 'dr', 30.0, NULL, 12.0, NULL, NULL, NULL, NULL)"
            .into(),
        "DROP TABLE nfr_verdicts".into(),
        "CREATE TABLE nfr_verdicts (
            experiment_id VARCHAR NOT NULL, nfr_id VARCHAR NOT NULL,
            kind VARCHAR NOT NULL, metric VARCHAR NOT NULL,
            operator VARCHAR NOT NULL, threshold DOUBLE NOT NULL,
            observed DOUBLE, state VARCHAR NOT NULL, margin DOUBLE,
            criticality VARCHAR NOT NULL
        )"
        .into(),
        "CREATE INDEX idx_nfr_verdicts_experiment_id ON nfr_verdicts (experiment_id)".into(),
        "INSERT INTO nfr_verdicts VALUES ('old-1', 'rto', 'rto', 'recovery.mttr_s', 'lte', \
         30.0, 12.0, 'met', 18.0, 'blocking')"
            .into(),
    ];
    if version < 23 {
        sql.extend([
            "DROP TABLE probe_samples".into(),
            "CREATE TABLE probe_samples (
                experiment_id VARCHAR NOT NULL, probe_name VARCHAR NOT NULL,
                phase VARCHAR NOT NULL, ts_ns BIGINT NOT NULL,
                value DOUBLE NOT NULL, ok BOOLEAN NOT NULL
            )"
            .into(),
        ]);
    }
    sql.push(format!(
        "UPDATE schema_meta SET value = {version} WHERE key = 'version'"
    ));
    sql
}

fn assert_an_upgrade_keeps_the_runs(version: i64) {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in before_the_dr_facts(version) {
            writer.execute(&sql, []).unwrap();
        }
        assert_eq!(writer.schema_version().unwrap(), version);
    }
    // Opening twice proves the migration is idempotent.
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.writer().unwrap().schema_version().unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }
    let store = Store::open(&path).unwrap();

    let experiments = columns_of(&store, "experiments");
    assert_eq!(
        experiments[experiments.len() - 5..],
        [
            "version_shares",
            "impact_observed",
            "outage_started_at_ns",
            "recovered_at_ns",
            "mttr_s"
        ]
    );
    let verdicts = columns_of(&store, "nfr_verdicts");
    assert_eq!(
        verdicts[verdicts.len() - 3..],
        ["criticality", "recovered", "reason"]
    );
    assert_eq!(
        columns_of(&store, "probe_samples").last().unwrap(),
        "reading"
    );
    // The rows stored before keep what they had and know nothing new.
    assert_eq!(
        row_of(&store, RUN_FACTS, "old-1"),
        json!({
            "impact_observed": null,
            "outage_started_at_ns": null,
            "recovered_at_ns": null,
            "mttr_s": null,
        })
    );
    assert_eq!(
        row_of(&store, VERDICT_FACTS, "old-1"),
        json!({"state": "met", "observed": 12.0, "recovered": null, "reason": null})
    );
    assert_eq!(
        row_of(&store, ROOT, "old-1"),
        json!({"recovery_time_s": 12.0, "journal_only": true})
    );

    ingest(&store, &[never_back(), without_objectives()]);
    assert_eq!(
        row_of(&store, VERDICT_FACTS, "never-back")["recovered"],
        json!(false)
    );
    assert_eq!(row_of(&store, ROOT, "plain")["recovery_time_s"], json!(6.0));
}

#[test]
fn a_v21_lake_gains_the_dr_facts_and_keeps_its_runs() {
    assert_an_upgrade_keeps_the_runs(21);
}

#[test]
fn a_v22_lake_gains_the_dr_facts_and_keeps_its_runs() {
    assert_an_upgrade_keeps_the_runs(22);
}

#[test]
fn a_v23_lake_gains_the_dr_facts_and_keeps_its_runs() {
    assert_an_upgrade_keeps_the_runs(23);
}
