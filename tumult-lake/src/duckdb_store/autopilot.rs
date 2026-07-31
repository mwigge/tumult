//! Autopilot decision store: two INSERT-ONLY tables, event-sourcing style.
//!
//! `autopilot_decisions` is the immutable record of every decision the
//! autopilot ever made — verdict, every gate rule evaluated, the policy
//! hash it is reproducible from. `autopilot_events` records what happened
//! afterwards (run started/completed, human approved/denied) as separate
//! appended rows, so a decision row is NEVER updated. `DuckDB` has no
//! triggers to enforce this at the schema level (the reason sluss-style
//! `SQLite` triggers don't transfer), so immutability is enforced here: this
//! module exposes no update or delete surface, and none may be added.
//!
//! Ordering contract (audit-before-act): callers MUST persist the decision
//! row before enacting it, and append events as they occur — a crash
//! between decision and run leaves the decision visible with no run event,
//! which is exactly the truthful record.
//!
//! The read side (status listings, class history, budget/cooldown queries)
//! lives in `tumult-query`.

use duckdb::params;

use crate::error::AnalyticsError;

use super::AnalyticsStore;

/// One decision, as persisted. Field meanings mirror `tumult-autopilot`'s
/// gate output; JSON columns carry the structured detail verbatim.
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub id: String,
    pub decided_at_ns: i64,
    pub trigger: String,
    pub service_id: String,
    pub tier: Option<String>,
    pub plugin: String,
    pub action: String,
    pub article_id: String,
    pub score: f64,
    pub reasons: serde_json::Value,
    pub confidence: String,
    pub playbook: Option<String>,
    pub validator: serde_json::Value,
    pub verdict: String,
    pub gate_rules: serde_json::Value,
    pub gate_detail: serde_json::Value,
    pub policy_hash: String,
    pub autonomy_score: Option<f64>,
}

/// A decision joined with its latest lifecycle event, for status listings.
#[derive(Debug, Clone)]
pub struct DecisionStatus {
    pub record: DecisionRecord,
    /// Latest event kind (`run_started`, `run_completed`, `run_failed`,
    /// `human_approved`, `human_denied`), or None when nothing happened yet.
    pub last_event: Option<String>,
    pub last_event_detail: Option<String>,
}

/// One recorded change event.
#[derive(Debug, Clone)]
pub struct ChangeEventRecord {
    pub service_id: String,
    pub at_ns: i64,
    pub source: String,
    pub detail: Option<String>,
}

/// Aggregated per-fault-class autonomy history: how often enacted runs of
/// this class completed without incident. Feeds the earned-autonomy ladder.
#[derive(Debug, Clone)]
pub struct ClassHistory {
    pub class_key: String,
    pub enacted_total: u32,
    pub enacted_clean: u32,
}

// Every method below delegates fallible storage operations to DuckDB and
// returns `AnalyticsError`; documenting that same condition on each small
// store accessor would obscure their behavioral contract.
#[allow(clippy::missing_errors_doc)]
impl AnalyticsStore {
    /// Append one decision. INSERT only — a decision is immutable once
    /// written; later developments are `append_autopilot_event` rows.
    pub fn insert_autopilot_decision(&self, d: &DecisionRecord) -> Result<(), AnalyticsError> {
        self.conn.execute(
            "INSERT INTO autopilot_decisions VALUES
             (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS JSON), ?, ?, CAST(? AS JSON), ?,
              CAST(? AS JSON), CAST(? AS JSON), ?, ?)",
            params![
                d.id,
                d.decided_at_ns,
                d.trigger,
                d.service_id,
                d.tier,
                d.plugin,
                d.action,
                d.article_id,
                d.score,
                d.reasons.to_string(),
                d.confidence,
                d.playbook,
                d.validator.to_string(),
                d.verdict,
                d.gate_rules.to_string(),
                d.gate_detail.to_string(),
                d.policy_hash,
                d.autonomy_score,
            ],
        )?;
        Ok(())
    }

    /// Append one lifecycle event for a decision.
    pub fn append_autopilot_event(
        &self,
        decision_id: &str,
        at_ns: i64,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), AnalyticsError> {
        self.conn.execute(
            "INSERT INTO autopilot_events VALUES (?, ?, ?, CAST(? AS JSON))",
            params![decision_id, at_ns, kind, detail.to_string()],
        )?;
        Ok(())
    }

    /// Record an external change event (deploy, config change) against a
    /// service — the change-event trigger's insert-only input.
    pub fn record_change_event(
        &self,
        service_id: &str,
        at_ns: i64,
        source: &str,
        detail: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        self.conn.execute(
            "INSERT INTO autopilot_change_events VALUES (?, ?, ?, ?)",
            params![service_id, at_ns, source, detail],
        )?;
        Ok(())
    }

    /// Record the decision's graph lineage node (`rec:<id>`,
    /// kind `recommendation`). Attrs carry verdict/article/service/policy
    /// hash so lineage is queryable in-graph and via Cypher.
    pub fn record_recommendation_node(
        &self,
        decision_id: &str,
        label: &str,
        attrs: &serde_json::Value,
    ) -> Result<(), AnalyticsError> {
        self.conn.execute(
            tumult_graph::sql::UPSERT_NODE,
            params![
                format!("rec:{decision_id}"),
                "recommendation",
                label,
                attrs.to_string()
            ],
        )?;
        Ok(())
    }

    /// Link an enacted decision to the run it produced:
    /// `rec:<id> -[enacted]-> run:<experiment_id>`.
    pub fn record_enacted_edge(
        &self,
        decision_id: &str,
        experiment_id: &str,
        ts_ns: i64,
    ) -> Result<(), AnalyticsError> {
        self.conn.execute(
            tumult_graph::sql::INSERT_EDGE,
            params![
                format!("rec:{decision_id}"),
                "enacted",
                format!("run:{experiment_id}"),
                format!("autopilot:{decision_id}"),
                ts_ns,
                "{}"
            ],
        )?;
        Ok(())
    }

    /// Export both decision tables to Parquet files under `dir`
    /// (`autopilot_decisions.parquet`, `autopilot_events.parquet`) — the
    /// immutable cold archive any analytics tool can read.
    pub fn export_autopilot_parquet(&self, dir: &std::path::Path) -> Result<(), AnalyticsError> {
        std::fs::create_dir_all(dir).map_err(AnalyticsError::Io)?;
        for table in ["autopilot_decisions", "autopilot_events"] {
            let out = dir.join(format!("{table}.parquet"));
            let sql = format!(
                "COPY {table} TO '{}' (FORMAT PARQUET)",
                out.display().to_string().replace('\'', "''")
            );
            self.conn.execute_batch(&sql)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(id: &str, verdict: &str, service: &str) -> DecisionRecord {
        DecisionRecord {
            id: id.into(),
            decided_at_ns: 1_000,
            trigger: "staleness".into(),
            service_id: service.into(),
            tier: Some("data".into()),
            plugin: "tumult-postgres".into(),
            action: "kill-connections".into(),
            article_id: "compliance:NIS2/Art.21(2)(c)".into(),
            score: 1.5,
            reasons: serde_json::json!(["r1"]),
            confidence: "high".into(),
            playbook: Some("demo/experiments/demo-topo-recommended.toon".into()),
            validator: serde_json::json!({"hollow": [], "enactable": true}),
            verdict: verdict.into(),
            gate_rules: serde_json::json!([["enabled", true]]),
            gate_detail: serde_json::json!({}),
            policy_hash: "abc123".into(),
            autonomy_score: Some(1.0),
        }
    }

    #[test]
    fn parquet_export_writes_both_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let s = AnalyticsStore::in_memory().unwrap();
        s.insert_autopilot_decision(&decision("d1", "enact", "svc:db"))
            .unwrap();
        s.export_autopilot_parquet(tmp.path()).unwrap();
        assert!(tmp.path().join("autopilot_decisions.parquet").exists());
        assert!(tmp.path().join("autopilot_events.parquet").exists());
    }
}
