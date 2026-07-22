//! Journal ingest and raw query execution for [`ClickHouseStore`].

use tumult_analytics::error::AnalyticsError;
use tumult_analytics::query_row::QueryRow;
use tumult_analytics::telemetry;
use tumult_core::types::Journal;

use super::rows::{ActivityRow, CountRow, ExperimentRow};
use super::ClickHouseStore;

impl ClickHouseStore {
    /// Async ingest using typed Row inserts (no SQL interpolation).
    ///
    /// # Errors
    ///
    /// Returns an error if the `ClickHouse` insert or duplicate check fails.
    pub async fn ingest_journal_async(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        let _span = telemetry::begin_ingest(&journal.experiment_id, &journal.experiment_title);

        // Check duplicate via parameterized bind
        let count = self
            .with_timeout(async {
                self.client
                    .query("SELECT count() as count FROM experiments WHERE experiment_id = ?")
                    .bind(&journal.experiment_id)
                    .fetch_one::<CountRow>()
                    .await
                    .map_err(|e| Self::ch_err(&e))
            })
            .await?;

        if count.count > 0 {
            telemetry::event_journal_duplicate(&journal.experiment_id);
            return Ok(false);
        }

        // Type-safe insert for experiment
        let exp_row = ExperimentRow {
            experiment_id: journal.experiment_id.clone(),
            title: journal.experiment_title.clone(),
            status: journal.status.to_string(),
            started_at_ns: journal.started_at_ns,
            ended_at_ns: journal.ended_at_ns,
            duration_ms: journal.duration_ms,
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            method_step_count: journal.method_results.len() as i64,
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            rollback_count: journal.rollback_results.len() as i64,
            hypothesis_before_met: journal
                .steady_state_before
                .as_ref()
                .map(|h| u8::from(h.met)),
            hypothesis_after_met: journal.steady_state_after.as_ref().map(|h| u8::from(h.met)),
            estimate_accuracy: journal.analysis.as_ref().and_then(|a| a.estimate_accuracy),
            resilience_score: journal.analysis.as_ref().and_then(|a| a.resilience_score),
        };

        let mut insert = self
            .client
            .insert::<ExperimentRow>("experiments")
            .await
            .map_err(|e| Self::ch_err(&e))?;
        insert.write(&exp_row).await.map_err(|e| Self::ch_err(&e))?;
        insert.end().await.map_err(|e| Self::ch_err(&e))?;

        // Type-safe insert for activity results
        let mut activity_count = 0usize;
        // Clone once outside the loop instead of once per ActivityRow.
        let experiment_id = journal.experiment_id.clone();
        let phases: Vec<(&str, &[tumult_core::types::ActivityResult])> = vec![
            (
                "hypothesis_before",
                journal
                    .steady_state_before
                    .as_ref()
                    .map_or(&[], |h| h.probe_results.as_slice()),
            ),
            ("method", &journal.method_results),
            (
                "hypothesis_after",
                journal
                    .steady_state_after
                    .as_ref()
                    .map_or(&[], |h| h.probe_results.as_slice()),
            ),
            ("rollback", &journal.rollback_results),
        ];

        let mut act_insert = self
            .client
            .insert::<ActivityRow>("activity_results")
            .await
            .map_err(|e| Self::ch_err(&e))?;

        for (phase, results) in phases {
            for r in results {
                let row = ActivityRow {
                    experiment_id: experiment_id.clone(),
                    name: r.name.clone(),
                    activity_type: r.activity_type.to_string(),
                    status: r.status.to_string(),
                    started_at_ns: r.started_at_ns,
                    duration_ms: r.duration_ms,
                    output: r.output.clone(),
                    error: r.error.clone(),
                    phase: phase.to_string(),
                };
                act_insert.write(&row).await.map_err(|e| Self::ch_err(&e))?;
                activity_count += 1;
            }
        }

        act_insert.end().await.map_err(|e| Self::ch_err(&e))?;

        telemetry::event_journal_ingested(&journal.experiment_id, activity_count);
        crate::telemetry::record_store_gauges(0, 0); // will be updated on next stats call
        Ok(true)
    }

    /// Async query execution — returns rows as TSV-parsed string vectors.
    ///
    /// Bounded by the configured query timeout like every other query path;
    /// without it a hung `ClickHouse` connection would stall the caller
    /// indefinitely.
    pub(crate) async fn query_async(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError> {
        let _span = telemetry::begin_query(sql);

        self.with_timeout(async {
            let mut cursor = self
                .client
                .query(sql)
                .fetch_bytes("TabSeparated")
                .map_err(|e| Self::ch_err(&e))?;

            let mut result = Vec::new();
            while let Some(bytes) = cursor.next().await.map_err(|e| Self::ch_err(&e))? {
                let line = String::from_utf8_lossy(&bytes);
                let fields: Vec<String> = line
                    .split('\t')
                    .map(std::string::ToString::to_string)
                    .collect();
                result.push(QueryRow::from(fields));
            }

            telemetry::event_query_executed(result.len(), 0);
            Ok(result)
        })
        .await
    }

    pub(crate) async fn query_columns_async(
        &self,
        sql: &str,
    ) -> Result<Vec<String>, AnalyticsError> {
        self.with_timeout(async {
            let mut cursor = self
                .client
                .query(sql)
                .fetch_bytes("TabSeparatedWithNames")
                .map_err(|e| Self::ch_err(&e))?;

            // First row is header with column names
            if let Some(bytes) = cursor.next().await.map_err(|e| Self::ch_err(&e))? {
                let line = String::from_utf8_lossy(&bytes);
                return Ok(line
                    .split('\t')
                    .map(std::string::ToString::to_string)
                    .collect());
            }
            Ok(vec![])
        })
        .await
    }
}
