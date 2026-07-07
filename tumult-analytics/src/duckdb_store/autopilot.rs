//! Autopilot decision store: two INSERT-ONLY tables, event-sourcing style.
//!
//! `autopilot_decisions` is the immutable record of every decision the
//! autopilot ever made — verdict, every gate rule evaluated, the policy
//! hash it is reproducible from. `autopilot_events` records what happened
//! afterwards (run started/completed, human approved/denied) as separate
//! appended rows, so a decision row is NEVER updated. DuckDB has no
//! triggers to enforce this at the schema level (the reason sluss-style
//! SQLite triggers don't transfer), so immutability is enforced here: this
//! module exposes no update or delete surface, and none may be added.
//!
//! Ordering contract (audit-before-act): callers MUST persist the decision
//! row before enacting it, and append events as they occur — a crash
//! between decision and run leaves the decision visible with no run event,
//! which is exactly the truthful record.

use duckdb::params;

use crate::error::AnalyticsError;

use super::AnalyticsStore;

pub(super) const CREATE_AUTOPILOT_TABLES: &str = "
CREATE TABLE IF NOT EXISTS autopilot_decisions (
    id              VARCHAR PRIMARY KEY,
    decided_at_ns   BIGINT NOT NULL,
    trigger         VARCHAR NOT NULL,
    service_id      VARCHAR NOT NULL,
    tier            VARCHAR,
    plugin          VARCHAR NOT NULL,
    action          VARCHAR NOT NULL,
    article_id      VARCHAR NOT NULL,
    score           DOUBLE NOT NULL,
    reasons         JSON NOT NULL,
    confidence      VARCHAR NOT NULL,
    playbook        VARCHAR,
    validator       JSON NOT NULL,
    verdict         VARCHAR NOT NULL,
    gate_rules      JSON NOT NULL,
    gate_detail     JSON NOT NULL,
    policy_hash     VARCHAR NOT NULL,
    autonomy_score  DOUBLE
);
CREATE TABLE IF NOT EXISTS autopilot_events (
    decision_id     VARCHAR NOT NULL,
    at_ns           BIGINT NOT NULL,
    kind            VARCHAR NOT NULL,
    detail          JSON NOT NULL
);
CREATE INDEX IF NOT EXISTS autopilot_events_by_decision
    ON autopilot_events (decision_id, at_ns);
";

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

/// Aggregated per-fault-class autonomy history: how often enacted runs of
/// this class completed without incident. Feeds the earned-autonomy ladder.
#[derive(Debug, Clone)]
pub struct ClassHistory {
    pub class_key: String,
    pub enacted_total: u32,
    pub enacted_clean: u32,
}

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

    /// Decisions newest first with their latest event, optionally filtered
    /// to one verdict (e.g. `propose` for the approval queue).
    pub fn autopilot_decisions(
        &self,
        verdict: Option<&str>,
        limit: u64,
    ) -> Result<Vec<DecisionStatus>, AnalyticsError> {
        let sql = format!(
            "SELECT d.id, d.decided_at_ns, d.trigger, d.service_id, d.tier, d.plugin,
                    d.action, d.article_id, d.score, CAST(d.reasons AS VARCHAR),
                    d.confidence, d.playbook, CAST(d.validator AS VARCHAR), d.verdict,
                    CAST(d.gate_rules AS VARCHAR), CAST(d.gate_detail AS VARCHAR),
                    d.policy_hash, d.autonomy_score,
                    e.kind, CAST(e.detail AS VARCHAR)
             FROM autopilot_decisions d
             LEFT JOIN (
                 SELECT decision_id, kind, detail,
                        row_number() OVER (PARTITION BY decision_id ORDER BY at_ns DESC) rn
                 FROM autopilot_events) e
               ON e.decision_id = d.id AND e.rn = 1
             {} ORDER BY d.decided_at_ns DESC LIMIT ?",
            if verdict.is_some() { "WHERE d.verdict = ?" } else { "" }
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let map_row = |row: &duckdb::Row<'_>| {
            let json = |i: usize| -> duckdb::Result<serde_json::Value> {
                let text: String = row.get(i)?;
                Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
            };
            Ok(DecisionStatus {
                record: DecisionRecord {
                    id: row.get(0)?,
                    decided_at_ns: row.get(1)?,
                    trigger: row.get(2)?,
                    service_id: row.get(3)?,
                    tier: row.get(4)?,
                    plugin: row.get(5)?,
                    action: row.get(6)?,
                    article_id: row.get(7)?,
                    score: row.get(8)?,
                    reasons: json(9)?,
                    confidence: row.get(10)?,
                    playbook: row.get(11)?,
                    validator: json(12)?,
                    verdict: row.get(13)?,
                    gate_rules: json(14)?,
                    gate_detail: json(15)?,
                    policy_hash: row.get(16)?,
                    autonomy_score: row.get(17)?,
                },
                last_event: row.get(18)?,
                last_event_detail: row.get(19)?,
            })
        };
        let rows = if let Some(v) = verdict {
            stmt.query_map(params![v, limit], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![limit], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// One decision by id, with latest event.
    pub fn autopilot_decision(&self, id: &str) -> Result<Option<DecisionStatus>, AnalyticsError> {
        let all = self.autopilot_decisions(None, 10_000)?;
        Ok(all.into_iter().find(|d| d.record.id == id))
    }

    /// Per-fault-class enactment history for the earned-autonomy ladder.
    /// Clean = an enacted decision whose latest event is `run_completed`.
    pub fn autopilot_class_history(&self) -> Result<Vec<ClassHistory>, AnalyticsError> {
        let mut stmt = self.conn.prepare(
            "SELECT d.plugin || '::' || d.action || '@' || COALESCE(d.tier, '-') AS class_key,
                    COUNT(*) AS total,
                    SUM(CASE WHEN e.kind = 'run_completed' THEN 1 ELSE 0 END) AS clean
             FROM autopilot_decisions d
             LEFT JOIN (
                 SELECT decision_id, kind,
                        row_number() OVER (PARTITION BY decision_id ORDER BY at_ns DESC) rn
                 FROM autopilot_events) e
               ON e.decision_id = d.id AND e.rn = 1
             WHERE d.verdict = 'enact'
             GROUP BY 1 ORDER BY 1",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ClassHistory {
                    class_key: row.get(0)?,
                    enacted_total: row.get::<_, i64>(1)? as u32,
                    enacted_clean: row.get::<_, i64>(2)? as u32,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Count of decisions made since `since_ns` (the daily budget input).
    pub fn autopilot_decisions_since(&self, since_ns: i64) -> Result<u32, AnalyticsError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM autopilot_decisions WHERE decided_at_ns >= ? AND verdict = 'enact'",
            params![since_ns],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    /// Most recent enacted decision timestamp for a service, for cooldowns.
    pub fn autopilot_last_enacted_on(&self, service_id: &str) -> Result<Option<i64>, AnalyticsError> {
        let ts = self
            .conn
            .query_row(
                "SELECT MAX(decided_at_ns) FROM autopilot_decisions
                 WHERE service_id = ? AND verdict = 'enact'",
                params![service_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap_or(None);
        Ok(ts)
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
        std::fs::create_dir_all(dir)
            .map_err(AnalyticsError::Io)?;
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
    fn decision_lifecycle_is_append_only_events() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.insert_autopilot_decision(&decision("d1", "enact", "svc:db")).unwrap();
        s.append_autopilot_event("d1", 1_100, "run_started", &serde_json::json!({})).unwrap();
        s.append_autopilot_event("d1", 1_200, "run_completed", &serde_json::json!({"experiment_id": "x"})).unwrap();

        let rows = s.autopilot_decisions(None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_event.as_deref(), Some("run_completed"));

        let history = s.autopilot_class_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].enacted_total, 1);
        assert_eq!(history[0].enacted_clean, 1);
        assert_eq!(history[0].class_key, "tumult-postgres::kill-connections@data");
    }

    #[test]
    fn queue_filter_budget_and_cooldown_queries() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.insert_autopilot_decision(&decision("d1", "propose", "svc:db")).unwrap();
        s.insert_autopilot_decision(&decision("d2", "enact", "svc:db")).unwrap();

        assert_eq!(s.autopilot_decisions(Some("propose"), 10).unwrap().len(), 1);
        assert_eq!(s.autopilot_decisions_since(0).unwrap(), 1);
        assert_eq!(s.autopilot_last_enacted_on("svc:db").unwrap(), Some(1_000));
        assert_eq!(s.autopilot_last_enacted_on("svc:none").unwrap(), None);
        assert!(s.autopilot_decision("d1").unwrap().is_some());
        assert!(s.autopilot_decision("nope").unwrap().is_none());
    }

    #[test]
    fn parquet_export_writes_both_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let s = AnalyticsStore::in_memory().unwrap();
        s.insert_autopilot_decision(&decision("d1", "enact", "svc:db")).unwrap();
        s.export_autopilot_parquet(tmp.path()).unwrap();
        assert!(tmp.path().join("autopilot_decisions.parquet").exists());
        assert!(tmp.path().join("autopilot_events.parquet").exists());
    }
}
