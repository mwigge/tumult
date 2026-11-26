//! Journal and agentic-run ingestion into the `DuckDB` analytics store.

use arrow::record_batch::RecordBatch;
use duckdb::{params, Connection};
use tumult_core::types::{Experiment, Journal};

use crate::arrow_convert::{
    journal_to_activity_batch, journal_to_experiment_batch, journal_to_load_batch,
    journal_to_nfr_verdicts_batch, journal_to_probe_samples_batch,
    load_samples_to_probe_samples_batch,
};
use crate::error::AnalyticsError;
use crate::sql;
use crate::telemetry;

use super::{AgenticRunAnalytics, AnalyticsStore};

/// Check if an `experiment_id` already exists in the store, on `conn`.
pub(crate) fn experiment_exists(
    conn: &Connection,
    experiment_id: &str,
) -> Result<bool, AnalyticsError> {
    let mut stmt = conn.prepare(sql::analytics::EXPERIMENT_EXISTS)?;
    let count: i64 = stmt.query_row(params![experiment_id], |row| row.get(0))?;
    Ok(count > 0)
}

/// Append an Arrow batch to `table`, on `conn`.
pub(crate) fn insert_batch(
    conn: &Connection,
    table: &str,
    batch: &RecordBatch,
) -> Result<(), AnalyticsError> {
    let mut appender = conn.appender(table)?;
    appender.append_record_batch(batch.clone())?;
    appender.flush()?;
    Ok(())
}

/// Connection-based journal ingest shared by [`AnalyticsStore`] and
/// [`crate::Writer`] (the single-writer connection behind the ingest daemon).
/// See [`AnalyticsStore::ingest_journal_with_experiment`] for semantics.
pub(crate) fn ingest_journal_with_experiment(
    conn: &Connection,
    journal: &Journal,
    experiment: Option<&Experiment>,
) -> Result<bool, AnalyticsError> {
    let _span = telemetry::begin_ingest(&journal.experiment_id, &journal.experiment_title);

    if experiment_exists(conn, &journal.experiment_id)? {
        telemetry::event_journal_duplicate(&journal.experiment_id);
        return Ok(false);
    }

    // Atomic ingest: experiments + activities + load + graph rows commit
    // together or not at all. Without the transaction a mid-ingest failure
    // committed the experiments row, and re-ingest then skipped the
    // experiment as a duplicate — permanently losing the remaining rows.
    conn.execute_batch(sql::tx::BEGIN)?;
    match ingest_journal_inner(conn, journal, experiment) {
        Ok(activity_count) => {
            conn.execute_batch(sql::tx::COMMIT)?;
            telemetry::event_journal_ingested(&journal.experiment_id, activity_count);
            Ok(true)
        }
        Err(e) => {
            let _ = conn.execute_batch(sql::tx::ROLLBACK);
            Err(e)
        }
    }
}

/// A table that holds rows of one run beside the run's `experiments` row,
/// and the column that holds the run's id.
pub(crate) struct RunTable {
    pub(crate) table: &'static str,
    pub(crate) run_key: &'static str,
}

pub(crate) const ACTIVITY_RESULTS: RunTable = RunTable {
    table: "activity_results",
    run_key: "experiment_id",
};
pub(crate) const LOAD_RESULTS: RunTable = RunTable {
    table: "load_results",
    run_key: "experiment_id",
};
pub(crate) const PROBE_SAMPLES: RunTable = RunTable {
    table: "probe_samples",
    run_key: "experiment_id",
};
const NFR_VERDICTS: RunTable = RunTable {
    table: "nfr_verdicts",
    run_key: "experiment_id",
};
/// Written by [`super::graph::populate_graph`]: each edge of a run holds
/// the run's id.
const GRAPH_EDGES: RunTable = RunTable {
    table: "graph_edges",
    run_key: "run_id",
};

/// Every table [`ingest_journal_inner`] writes rows of one run to, beside
/// its `experiments` row. The graph nodes are not here: a run shares most
/// of its nodes with other runs, and owns only the nodes of
/// [`tumult_graph::run_node_ids`].
pub(crate) const RUN_TABLES: [RunTable; 5] = [
    ACTIVITY_RESULTS,
    LOAD_RESULTS,
    PROBE_SAMPLES,
    NFR_VERDICTS,
    GRAPH_EDGES,
];

/// Delete what [`ingest_journal_inner`] wrote for run `experiment_id`
/// beside its `experiments` row, on `conn`, inside the caller's
/// transaction: its rows of [`RUN_TABLES`] and the graph nodes it owns.
/// The nodes it shares with other runs stay, also when no edge is left on
/// them.
pub(crate) fn delete_run_rows(
    conn: &Connection,
    experiment_id: &str,
) -> Result<(), AnalyticsError> {
    for run_table in &RUN_TABLES {
        conn.execute(&delete_run_sql(run_table), params![experiment_id])?;
    }
    for node in tumult_graph::run_node_ids(experiment_id) {
        conn.execute(tumult_graph::sql::DELETE_NODE, params![node])?;
    }
    Ok(())
}

/// The insert half of [`ingest_journal_with_experiment`], run inside the
/// caller's transaction. Returns the number of activity rows written.
fn ingest_journal_inner(
    conn: &Connection,
    journal: &Journal,
    experiment: Option<&Experiment>,
) -> Result<usize, AnalyticsError> {
    let exp_batch = journal_to_experiment_batch(journal)?;
    let act_batch = journal_to_activity_batch(journal)?;
    let activity_count = act_batch.num_rows();
    insert_batch(conn, "experiments", &exp_batch)?;
    if activity_count > 0 {
        insert_batch(conn, ACTIVITY_RESULTS.table, &act_batch)?;
    }
    if let Some(ref load_result) = journal.load_result {
        let load_batch = journal_to_load_batch(&journal.experiment_id, load_result)?;
        insert_batch(conn, LOAD_RESULTS.table, &load_batch)?;
        // v17: a streaming adapter's windows land next to the probe
        // samples as `load.*` series.
        if !load_result.samples.is_empty() {
            insert_batch(
                conn,
                PROBE_SAMPLES.table,
                &load_samples_to_probe_samples_batch(&journal.experiment_id, load_result)?,
            )?;
        }
    }
    // v16: raw probe samples and objective verdicts.
    if !journal.probe_samples.is_empty() {
        insert_batch(
            conn,
            PROBE_SAMPLES.table,
            &journal_to_probe_samples_batch(journal)?,
        )?;
    }
    let verdicts = journal_to_nfr_verdicts_batch(journal)?;
    if verdicts.num_rows() > 0 {
        insert_batch(conn, NFR_VERDICTS.table, &verdicts)?;
    }
    // ChaosGraph: upsert this run's nodes/edges (schema v2).
    super::graph::populate_graph(conn, journal, experiment)?;
    Ok(activity_count)
}

impl AnalyticsStore {
    /// Ingest a single experiment journal into the analytics store.
    /// Skips ingestion if the `experiment_id` already exists (incremental/dedup).
    ///
    /// Returns true if the journal was ingested, false if it was a duplicate.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DuckDB` insert or Arrow conversion fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use tumult_lake::AnalyticsStore;
    /// use tumult_core::types::*;
    ///
    /// let store = AnalyticsStore::in_memory().unwrap();
    ///
    /// let journal = Journal {
    ///     experiment_title: "demo".into(),
    ///     experiment_id: "e-001".into(),
    ///     status: ExperimentStatus::Completed,
    ///     started_at_ns: 1_700_000_000_000_000_000,
    ///     ended_at_ns: 1_700_000_060_000_000_000,
    ///     duration_ms: 60_000,
    ///     steady_state_before: None,
    ///     steady_state_after: None,
    ///     method_results: vec![],
    ///     rollback_results: vec![],
    ///     estimate: None,
    ///     baseline_result: None,
    ///     during_result: None,
    ///     post_result: None,
    ///     load_result: None,
    ///     analysis: None,
    ///     regulatory: None,
    ///     rollback_failures: 0,
    ///     halt: None,
    ///     blast_radius: None,
    ///     kind: TestKind::default(),
    ///     objectives: None,
    ///     probe_samples: vec![],
    ///     target_version: None,
    ///     target: None,
    /// };
    ///
    /// store.ingest_journal(&journal).unwrap();
    /// assert_eq!(store.experiment_count().unwrap(), 1);
    /// ```
    #[must_use = "callers must check whether the journal was ingested or skipped as a duplicate"]
    pub fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        self.ingest_journal_with_experiment(journal, None)
    }

    /// Ingest a journal, enriching the `ChaosGraph` with the experiment
    /// definition when available.
    ///
    /// Identical to [`Self::ingest_journal`] for the analytics tables, but the
    /// graph population uses `experiment` (when `Some`) to record
    /// `Fault = plugin::function` and `Service` nodes; with `None` the graph
    /// falls back to deriving faults from the journal's action results.
    /// Skips ingestion (and graph population) if the `experiment_id` already
    /// exists, so re-ingesting a run never duplicates graph rows.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DuckDB` insert or Arrow conversion fails.
    #[must_use = "callers must check whether the journal was ingested or skipped as a duplicate"]
    pub fn ingest_journal_with_experiment(
        &self,
        journal: &Journal,
        experiment: Option<&Experiment>,
    ) -> Result<bool, AnalyticsError> {
        ingest_journal_with_experiment(&self.conn, journal, experiment)
    }

    /// Ingest multiple journals, skipping duplicates.
    /// Returns the count of newly ingested journals.
    ///
    /// # Errors
    ///
    /// Returns an error if any individual journal ingestion fails.
    #[must_use = "callers must check the count of newly ingested journals"]
    pub fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        let mut count = 0;
        for journal in journals {
            if self.ingest_journal(journal)? {
                count += 1;
            }
        }
        // Record store gauges after batch ingestion
        if let Ok(stats) = self.stats() {
            telemetry::record_store_gauges(stats.experiment_count, stats.activity_count, None);
        }
        Ok(count)
    }

    /// Ingest one agentic run into analytics tables.
    ///
    /// # Errors
    ///
    /// Returns an error if any `DuckDB` insert fails.
    #[must_use = "callers must check whether the agentic run was ingested"]
    pub fn ingest_agentic_run(&self, run: &AgenticRunAnalytics) -> Result<bool, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql::analytics::AGENTIC_RUN_EXISTS)?;
        let count: i64 = stmt.query_row(params![run.run_id], |row| row.get(0))?;
        if count > 0 {
            return Ok(false);
        }

        self.conn.execute(
            sql::analytics::INSERT_AGENTIC_RUN,
            params![
                run.run_id,
                run.experiment_id,
                run.target_type,
                run.scenario,
                run.resilience_score,
                run.trace_id,
                run.replay_id
            ],
        )?;

        for contract in &run.contracts {
            self.conn.execute(
                sql::analytics::INSERT_CONTRACT_OUTCOME,
                params![
                    run.run_id,
                    contract.scenario,
                    contract.contract_type,
                    contract.passed,
                    contract.reason,
                    contract.severity
                ],
            )?;
        }

        for fault in &run.faults {
            self.conn.execute(
                sql::analytics::INSERT_FAULT_APPLICATION,
                params![run.run_id, fault.scenario, fault.fault_type, fault.applied],
            )?;
        }

        if let Some(replay_id) = &run.replay_id {
            let passed = run.contracts.iter().all(|contract| contract.passed);
            self.conn.execute(
                sql::analytics::INSERT_REPLAY_OUTCOME,
                params![run.run_id, replay_id, run.scenario, passed],
            )?;
        }

        Ok(true)
    }

    pub(crate) fn insert_batch(
        &self,
        table: &str,
        batch: &RecordBatch,
    ) -> Result<(), AnalyticsError> {
        insert_batch(&self.conn, table, batch)
    }
}

/// Delete the rows of one run from `run_table`.
fn delete_run_sql(run_table: &RunTable) -> String {
    format!(
        "DELETE FROM {} WHERE {} = ?",
        run_table.table, run_table.run_key
    )
}

/// The statements that delete the rows of a run, for the SQL corpus: one
/// for each table of [`RUN_TABLES`].
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    RUN_TABLES
        .iter()
        .map(|run_table| {
            tumult_test_utils::sql_corpus::Statement::new(
                format!("{}:delete_run_sql({})", file!(), run_table.table),
                delete_run_sql(run_table),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::sample_journal;
    use super::super::{
        AgenticContractAnalytics, AgenticFaultAnalytics, AgenticRunAnalytics, AnalyticsStore,
    };
    use tumult_core::types::*;

    #[test]
    fn create_store() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert_eq!(s.experiment_count().unwrap(), 0);
    }
    #[test]
    fn ingest_single() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        assert_eq!(s.experiment_count().unwrap(), 1);
    }
    #[test]
    fn ingest_multiple() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert_eq!(
            s.ingest_journals(&[
                sample_journal("e1", ExperimentStatus::Completed),
                sample_journal("e2", ExperimentStatus::Deviated),
                sample_journal("e3", ExperimentStatus::Completed)
            ])
            .unwrap(),
            3
        );
        assert_eq!(s.experiment_count().unwrap(), 3);
    }

    #[test]
    fn ingest_skips_duplicate_experiment_id() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        // Should only have 1 row, not 2
        assert_eq!(s.experiment_count().unwrap(), 1);
    }

    /// A failure mid-ingest must roll back the whole journal: previously the
    /// `experiments` row stayed committed in autocommit, so a re-ingest was
    /// skipped as a duplicate and the activity/load/graph rows were lost
    /// permanently.
    #[test]
    fn failed_ingest_rolls_back_and_retry_succeeds() {
        let s = AnalyticsStore::in_memory().unwrap();
        let journal = sample_journal("e1", ExperimentStatus::Completed);

        // Sabotage a late ingest step: `populate_graph` upserts into
        // `graph_nodes`, so dropping the table forces a failure AFTER the
        // experiments and activity rows were inserted.
        s.conn.execute_batch("DROP TABLE graph_nodes").unwrap();

        assert!(s.ingest_journal(&journal).is_err());
        // Nothing partial survives: no experiments row, no activity rows.
        assert_eq!(s.experiment_count().unwrap(), 0);
        let rows = s.query("SELECT count(*) FROM activity_results").unwrap();
        assert_eq!(rows[0][0], "0");

        // Repair the schema and retry — must ingest, not skip as duplicate.
        s.conn
            .execute_batch(tumult_graph::sql::CREATE_TABLES)
            .unwrap();
        assert!(s.ingest_journal(&journal).unwrap());
        assert_eq!(s.experiment_count().unwrap(), 1);
        let rows = s.query("SELECT count(*) FROM activity_results").unwrap();
        assert_eq!(rows[0][0], "1");
    }

    #[test]
    fn ingest_journals_returns_only_new_count() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        let ingested = s
            .ingest_journals(&[
                sample_journal("e1", ExperimentStatus::Completed), // duplicate
                sample_journal("e2", ExperimentStatus::Deviated),  // new
                sample_journal("e3", ExperimentStatus::Completed), // new
            ])
            .unwrap();
        assert_eq!(ingested, 2); // only 2 new
        assert_eq!(s.experiment_count().unwrap(), 3);
    }

    #[test]
    fn load_result_ingested_into_duckdb() {
        use tumult_core::types::{LoadResult, LoadTool};

        let s = AnalyticsStore::in_memory().unwrap();
        let mut journal = sample_journal("load-test-1", ExperimentStatus::Completed);
        journal.load_result = Some(LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 11_000_000_000,
            duration_s: 10.0,
            vus: 5,
            throughput_rps: 100.0,
            latency_p50_ms: 15.0,
            latency_p95_ms: 150.0,
            latency_p99_ms: 500.0,
            error_rate: 0.02,
            total_requests: 1000,
            thresholds_met: true,
            samples: vec![],
            execution: tumult_core::types::LoadExecution::default(),
        });
        s.ingest_journal(&journal).unwrap();

        let rows = s
            .query("SELECT experiment_id, tool, vus, throughput_rps, latency_p95_ms, error_rate, total_requests FROM load_results")
            .unwrap();
        assert_eq!(rows.len(), 1, "should have 1 load result row");
        assert_eq!(rows[0][0], "load-test-1");
        assert_eq!(rows[0][1], "k6");
        assert_eq!(rows[0][2], "5");
    }

    #[test]
    fn probe_samples_and_verdicts_ingested_into_duckdb() {
        use tumult_core::types::{
            NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict, ObjectiveOutcome,
            ProbeSample, SamplePhase, TestKind,
        };

        let s = AnalyticsStore::in_memory().unwrap();
        let mut journal = sample_journal("dr-1", ExperimentStatus::Completed);
        journal.kind = TestKind::Dr;
        journal.probe_samples = vec![
            ProbeSample {
                probe: "health".into(),
                phase: SamplePhase::During,
                ts_ns: 1_000,
                duration_ms: 120,
                ok: true,
                reading: None,
            },
            ProbeSample {
                probe: "health".into(),
                phase: SamplePhase::Post,
                ts_ns: 2_000,
                duration_ms: 900,
                ok: false,
                reading: None,
            },
        ];
        journal.objectives = Some(ObjectiveOutcome {
            rto_target_s: Some(900.0),
            rpo_target_s: None,
            rto_measured_s: Some(42.5),
            rpo_measured_s: None,
            verdicts: vec![NfrVerdict {
                nfr_id: "rto".into(),
                kind: NfrKind::Rto,
                metric: "recovery.mttr_s".into(),
                operator: NfrOperator::Lte,
                threshold: 900.0,
                observed: Some(42.5),
                state: NfrState::Met,
                margin: Some(857.5),
                criticality: NfrCriticality::Blocking,
                recovered: None,
                reason: None,
            }],
        });
        s.ingest_journal(&journal).unwrap();

        let rows = s
            .query("SELECT test_kind, rto_target_s, rto_measured_s FROM experiments WHERE experiment_id = 'dr-1'")
            .unwrap();
        assert_eq!(rows[0][0], "dr");
        assert_eq!(rows[0][1].parse::<f64>().unwrap(), 900.0);
        assert_eq!(rows[0][2].parse::<f64>().unwrap(), 42.5);

        let rows = s
            .query("SELECT probe_name, phase, ts_ns, value, ok FROM probe_samples ORDER BY ts_ns")
            .unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0][1], "during");
        assert_eq!(rows[1][1], "post");
        assert_eq!(rows[1][3].parse::<f64>().unwrap(), 900.0);
        assert_eq!(rows[1][4], "false");

        let rows = s
            .query("SELECT nfr_id, kind, state, margin, criticality FROM nfr_verdicts")
            .unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0][0], "rto");
        assert_eq!(rows[0][1], "rto");
        assert_eq!(rows[0][2], "met");
        assert_eq!(rows[0][4], "blocking");
    }

    #[test]
    fn plain_journal_writes_no_samples_or_verdicts() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("plain", ExperimentStatus::Completed))
            .unwrap();
        assert_eq!(
            s.query("SELECT count(*) FROM probe_samples").unwrap()[0][0],
            "0"
        );
        assert_eq!(
            s.query("SELECT count(*) FROM nfr_verdicts").unwrap()[0][0],
            "0"
        );
        let rows = s
            .query("SELECT test_kind FROM experiments WHERE experiment_id = 'plain'")
            .unwrap();
        assert_eq!(rows[0][0], "chaos");
    }

    #[test]
    fn load_samples_ingested_as_probe_series() {
        use tumult_core::types::{LoadResult, LoadSample, LoadTool};

        let s = AnalyticsStore::in_memory().unwrap();
        let mut journal = sample_journal("soak-1", ExperimentStatus::Completed);
        let window = |ts_ns: i64, p95: f64| LoadSample {
            ts_ns,
            duration_s: 10.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: p95,
            latency_p99_ms: 90.0,
            error_rate: 0.0,
            requests: 400,
            counts: None,
            request_timeout_ms: None,
        };
        journal.load_result = Some(LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 21_000_000_000,
            duration_s: 20.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 75.0,
            latency_p99_ms: 90.0,
            error_rate: 0.0,
            total_requests: 800,
            thresholds_met: true,
            samples: vec![window(1_000_000_000, 70.0), window(11_000_000_000, 80.0)],
            execution: tumult_core::types::LoadExecution::default(),
        });
        s.ingest_journal(&journal).unwrap();

        let rows = s
            .query(
                "SELECT ts_ns, value FROM probe_samples \
                 WHERE probe_name = 'load.latency_p95_ms' AND phase = 'during' ORDER BY ts_ns",
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1].parse::<f64>().unwrap(), 70.0);
        assert_eq!(rows[1][1].parse::<f64>().unwrap(), 80.0);
        let count = s.query("SELECT count(*) FROM probe_samples").unwrap();
        assert_eq!(count[0][0], "12");
        let summary = s.query("SELECT tool FROM load_results").unwrap();
        assert_eq!(summary[0][0], "oha");
    }

    fn lines(rows: &[crate::QueryRow]) -> Vec<String> {
        rows.iter().map(|row| row.join("|")).collect()
    }

    #[test]
    fn the_version_a_run_recorded_is_kept_with_its_source() {
        let s = AnalyticsStore::in_memory().unwrap();
        let recorded = |id: &str, version: &str, version_source: VersionSource| Journal {
            target_version: Some(TargetVersion {
                version: version.into(),
                version_source,
                shares: Vec::new(),
            }),
            ..sample_journal(id, ExperimentStatus::Completed)
        };
        let share = |version: &str, requests: u64| VersionShare {
            version: version.into(),
            requests,
        };
        let mixed = Journal {
            target_version: TargetVersion::observed(vec![share("1.5.1", 60), share("1.6.0", 40)]),
            ..sample_journal("v-mixed", ExperimentStatus::Completed)
        };
        s.ingest_journals(&[
            recorded("v-declared", "1.5.0", VersionSource::Declared),
            recorded("v-observed", "1.5.1", VersionSource::Observed),
            mixed,
            sample_journal("v-unknown", ExperimentStatus::Completed),
        ])
        .unwrap();

        // A window more than one version served is kept with its shares.
        let shares = s
            .query(
                "SELECT experiment_id, target_version, version_source, version_shares \
                 FROM experiments WHERE version_shares IS NOT NULL",
            )
            .unwrap();
        assert_eq!(
            lines(&shares),
            [concat!(
                "v-mixed|1.5.1|mixed|",
                r#"[{"version":"1.5.1","requests":60},{"version":"1.6.0","requests":40}]"#
            )]
        );

        let rows = s
            .query(
                "SELECT experiment_id, target_version, version_source FROM experiments \
                 WHERE version_shares IS NULL AND target_version IS NOT NULL \
                 ORDER BY experiment_id",
            )
            .unwrap();
        assert_eq!(
            lines(&rows),
            ["v-declared|1.5.0|declared", "v-observed|1.5.1|observed"]
        );
        let unknown = s
            .query(
                "SELECT experiment_id FROM experiments \
                 WHERE target_version IS NULL AND version_source IS NULL",
            )
            .unwrap();
        assert_eq!(lines(&unknown), ["v-unknown"]);
    }

    #[test]
    fn how_a_load_was_executed_is_kept_with_its_result() {
        use tumult_core::types::{LoadExecution, LoadResult, LoadSample, LoadTool, RequestCounts};

        let s = AnalyticsStore::in_memory().unwrap();
        let counts = RequestCounts {
            sent: 800,
            ok: 500,
            timed_out: 200,
            transport: 10,
            http_4xx: 20,
            http_5xx: 60,
            http_other: 5,
            unclassified: 5,
        };
        let window = |ts_ns: i64, duration_s: f64| LoadSample {
            ts_ns,
            duration_s,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 70.0,
            latency_p99_ms: 90.0,
            error_rate: 0.0,
            requests: 400,
            counts: Some(RequestCounts {
                sent: 400,
                ok: 250,
                timed_out: 100,
                ..RequestCounts::default()
            }),
            request_timeout_ms: Some(2000.0),
        };
        let load = |samples: Vec<LoadSample>, execution: LoadExecution| LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 61_000_000_000,
            duration_s: 60.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 75.0,
            latency_p99_ms: 90.0,
            error_rate: 0.0,
            total_requests: 800,
            thresholds_met: false,
            samples,
            execution,
        };
        let mut cut_short = sample_journal("cut-short", ExperimentStatus::Halted);
        cut_short.load_result = Some(load(
            vec![window(1_000_000_000, 10.0), window(11_000_000_000, 2.5)],
            LoadExecution {
                planned_windows: Some(24),
                completed_windows: Some(20),
                unanswered_windows: 3,
                lost_to_unanswered: 2,
                aborted: true,
                reasons: vec!["2 of 24 windows skipped".into()],
                requests: Some(counts),
                censored_windows: 1,
            },
        ));
        // A tool that counts no windows and streamed none.
        let mut plain = sample_journal("plain", ExperimentStatus::Completed);
        plain.load_result = Some(load(vec![], LoadExecution::default()));
        s.ingest_journals(&[cut_short, plain]).unwrap();

        let rows = s
            .query(
                "SELECT CAST(aborted AS INTEGER), CAST(truncated AS INTEGER), planned_windows, \
                 completed_windows, unanswered_windows, lost_to_unanswered, \
                 CAST(sampled_duration_s * 10 AS INTEGER) \
                 FROM load_results WHERE experiment_id = 'cut-short'",
            )
            .unwrap();
        assert_eq!(lines(&rows), ["1|1|24|20|3|2|125"]);
        let plain = s
            .query(
                "SELECT count(*) FROM load_results WHERE experiment_id = 'plain' \
                 AND NOT aborted AND NOT truncated AND planned_windows IS NULL \
                 AND completed_windows IS NULL AND unanswered_windows = 0 \
                 AND lost_to_unanswered = 0 AND sampled_duration_s IS NULL",
            )
            .unwrap();
        assert_eq!(plain[0][0], "1");

        // What became of the requests, by class, and the censored windows.
        let requests = s
            .query(
                "SELECT requests_sent, requests_ok, requests_timed_out, requests_transport, \
                 requests_http_4xx, requests_http_5xx, requests_http_other, \
                 requests_unclassified, censored_windows \
                 FROM load_results WHERE experiment_id = 'cut-short'",
            )
            .unwrap();
        assert_eq!(lines(&requests), ["800|500|200|10|20|60|5|5|1"]);
        // A tool that does not say leaves them unknown, not zero.
        let unknown = s
            .query(
                "SELECT count(*) FROM load_results WHERE experiment_id = 'plain' \
                 AND requests_sent IS NULL AND requests_ok IS NULL \
                 AND requests_timed_out IS NULL AND requests_unclassified IS NULL \
                 AND censored_windows = 0",
            )
            .unwrap();
        assert_eq!(unknown[0][0], "1");
        // Each window keeps its own counts and timeout as series.
        let windows = s
            .query(
                "SELECT probe_name, count(*), CAST(sum(value) AS BIGINT) FROM probe_samples \
                 WHERE experiment_id = 'cut-short' AND (probe_name LIKE 'load.request%' \
                 OR probe_name = 'load.window_s') GROUP BY probe_name ORDER BY probe_name",
            )
            .unwrap();
        assert_eq!(
            lines(&windows),
            [
                "load.request_timeout_ms|2|4000",
                "load.requests_ok|2|500",
                "load.requests_sent|2|800",
                "load.requests_timed_out|2|200",
                "load.window_s|2|12",
            ]
        );
    }

    #[test]
    fn no_load_result_row_when_none() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("no-load", ExperimentStatus::Completed))
            .unwrap();
        let rows = s.query("SELECT count(*) FROM load_results").unwrap();
        assert_eq!(rows[0][0], "0");
    }

    #[test]
    fn ingest_agentic_run_writes_queryable_tables() {
        let s = AnalyticsStore::in_memory().unwrap();
        let run = AgenticRunAnalytics {
            run_id: "agentic-run-1".to_string(),
            experiment_id: "agentic-exp-1".to_string(),
            target_type: "http".to_string(),
            scenario: "malformed-json-recovery".to_string(),
            resilience_score: 0.0,
            trace_id: Some("trace-agentic-1".to_string()),
            replay_id: Some("replay-agentic-1".to_string()),
            contracts: vec![AgenticContractAnalytics {
                contract_type: "valid_json".to_string(),
                scenario: "malformed-json-recovery".to_string(),
                passed: false,
                reason: Some("invalid_json".to_string()),
                severity: 1.0,
            }],
            faults: vec![AgenticFaultAnalytics {
                fault_type: "malformed_output".to_string(),
                scenario: "malformed-json-recovery".to_string(),
                applied: true,
            }],
        };

        assert!(s.ingest_agentic_run(&run).unwrap());
        assert!(!s.ingest_agentic_run(&run).unwrap());

        let runs = s
            .query("SELECT scenario, resilience_score FROM agentic_runs")
            .unwrap();
        assert_eq!(runs[0][0], "malformed-json-recovery");
        assert_eq!(runs[0][1], "0.0000");

        let contracts = s
            .query("SELECT contract_type, reason FROM agentic_contract_outcomes")
            .unwrap();
        assert_eq!(contracts[0][0], "valid_json");
        assert_eq!(contracts[0][1], "invalid_json");

        let faults = s
            .query("SELECT fault_type, applied FROM agentic_fault_applications")
            .unwrap();
        assert_eq!(faults[0][0], "malformed_output");
        assert_eq!(faults[0][1], "true");

        let replay = s
            .query("SELECT replay_id, passed FROM agentic_replay_outcomes")
            .unwrap();
        assert_eq!(replay[0][0], "replay-agentic-1");
        assert_eq!(replay[0][1], "false");
    }
}
