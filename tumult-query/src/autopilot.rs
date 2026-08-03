//! Autopilot decision read queries: status listings, per-fault-class
//! autonomy history, and the budget/cooldown/change-event lookups the gate
//! consumes. The tables are INSERT-ONLY; the write side (decision/event
//! persistence) stays on [`tumult_lake::AnalyticsStore`].

use duckdb::params;
use tumult_lake::{
    AnalyticsError, AnalyticsStore, ChangeEventRecord, ClassHistory, DecisionRecord, DecisionStatus,
};

/// Decisions newest first with their latest event, optionally filtered
/// to one verdict (e.g. `propose` for the approval queue).
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned decision statuses"]
pub fn autopilot_decisions(
    store: &AnalyticsStore,
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
        if verdict.is_some() {
            "WHERE d.verdict = ?"
        } else {
            ""
        }
    );
    let mut stmt = store.__connection().prepare(&sql)?;
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
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned decision status"]
pub fn autopilot_decision(
    store: &AnalyticsStore,
    id: &str,
) -> Result<Option<DecisionStatus>, AnalyticsError> {
    let all = autopilot_decisions(store, None, 10_000)?;
    Ok(all.into_iter().find(|d| d.record.id == id))
}

/// Per-fault-class enactment history for the earned-autonomy ladder.
/// Clean = an enacted decision whose latest event is `run_completed`.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned class history"]
pub fn autopilot_class_history(
    store: &AnalyticsStore,
) -> Result<Vec<ClassHistory>, AnalyticsError> {
    let mut stmt = store.__connection().prepare(
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
            let enacted_total = u32::try_from(row.get::<_, i64>(1)?).map_err(|error| {
                duckdb::Error::FromSqlConversionFailure(
                    1,
                    duckdb::types::Type::BigInt,
                    Box::new(error),
                )
            })?;
            let enacted_clean = u32::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                duckdb::Error::FromSqlConversionFailure(
                    2,
                    duckdb::types::Type::BigInt,
                    Box::new(error),
                )
            })?;
            Ok(ClassHistory {
                class_key: row.get(0)?,
                enacted_total,
                enacted_clean,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Count of decisions made since `since_ns` (the daily budget input).
///
/// # Errors
///
/// Returns an error if the query fails, or the count does not fit a `u32`.
#[must_use = "callers must use the returned decision count"]
pub fn autopilot_decisions_since(
    store: &AnalyticsStore,
    since_ns: i64,
) -> Result<u32, AnalyticsError> {
    let n: i64 = store.__connection().query_row(
        "SELECT COUNT(*) FROM autopilot_decisions WHERE decided_at_ns >= ? AND verdict = 'enact'",
        params![since_ns],
        |r| r.get(0),
    )?;
    u32::try_from(n).map_err(|error| {
        AnalyticsError::Internal(format!("autopilot decision count is outside u32: {error}"))
    })
}

/// Most recent enacted decision timestamp for a service, for cooldowns.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned timestamp"]
pub fn autopilot_last_enacted_on(
    store: &AnalyticsStore,
    service_id: &str,
) -> Result<Option<i64>, AnalyticsError> {
    let ts = store.__connection().query_row(
        "SELECT MAX(decided_at_ns) FROM autopilot_decisions
             WHERE service_id = ? AND verdict = 'enact'",
        params![service_id],
        |r| r.get::<_, Option<i64>>(0),
    )?;
    Ok(ts)
}

/// Change events newer than `since_ns`, newest first per service.
///
/// # Errors
///
/// Returns an error if the query fails.
#[must_use = "callers must use the returned change events"]
pub fn change_events_since(
    store: &AnalyticsStore,
    since_ns: i64,
) -> Result<Vec<ChangeEventRecord>, AnalyticsError> {
    let mut stmt = store.__connection().prepare(
        "SELECT service_id, at_ns, source, detail FROM autopilot_change_events
         WHERE at_ns >= ? ORDER BY service_id, at_ns DESC",
    )?;
    let rows = stmt
        .query_map(params![since_ns], |row| {
            Ok(ChangeEventRecord {
                service_id: row.get(0)?,
                at_ns: row.get(1)?,
                source: row.get(2)?,
                detail: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use tumult_lake::{AnalyticsStore, DecisionRecord};

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
        s.insert_autopilot_decision(&decision("d1", "enact", "svc:db"))
            .unwrap();
        s.append_autopilot_event("d1", 1_100, "run_started", &serde_json::json!({}))
            .unwrap();
        s.append_autopilot_event(
            "d1",
            1_200,
            "run_completed",
            &serde_json::json!({"experiment_id": "x"}),
        )
        .unwrap();

        let rows = autopilot_decisions(&s, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_event.as_deref(), Some("run_completed"));

        let history = autopilot_class_history(&s).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].enacted_total, 1);
        assert_eq!(history[0].enacted_clean, 1);
        assert_eq!(
            history[0].class_key,
            "tumult-postgres::kill-connections@data"
        );
    }

    #[test]
    fn queue_filter_budget_and_cooldown_queries() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.insert_autopilot_decision(&decision("d1", "propose", "svc:db"))
            .unwrap();
        s.insert_autopilot_decision(&decision("d2", "enact", "svc:db"))
            .unwrap();

        assert_eq!(
            autopilot_decisions(&s, Some("propose"), 10).unwrap().len(),
            1
        );
        assert_eq!(autopilot_decisions_since(&s, 0).unwrap(), 1);
        assert_eq!(
            autopilot_last_enacted_on(&s, "svc:db").unwrap(),
            Some(1_000)
        );
        assert_eq!(autopilot_last_enacted_on(&s, "svc:none").unwrap(), None);
        assert!(autopilot_decision(&s, "d1").unwrap().is_some());
        assert!(autopilot_decision(&s, "nope").unwrap().is_none());
    }

    #[test]
    fn change_events_since_filters_and_orders() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.record_change_event("svc:db", 100, "deploy", Some("v1.2.3"))
            .unwrap();
        s.record_change_event("svc:db", 200, "config", None)
            .unwrap();
        s.record_change_event("svc:api", 150, "deploy", Some("v9"))
            .unwrap();
        // Older than the cutoff: excluded.
        s.record_change_event("svc:db", 10, "deploy", None).unwrap();

        let rows = change_events_since(&s, 50).unwrap();
        assert_eq!(rows.len(), 3);
        // Ordered by service_id, then newest first within a service.
        assert_eq!(rows[0].service_id, "svc:api");
        assert_eq!(rows[0].at_ns, 150);
        assert_eq!(rows[1].service_id, "svc:db");
        assert_eq!(rows[1].at_ns, 200);
        assert_eq!(rows[1].source, "config");
        assert_eq!(rows[1].detail, None);
        assert_eq!(rows[2].service_id, "svc:db");
        assert_eq!(rows[2].at_ns, 100);
        assert_eq!(rows[2].detail.as_deref(), Some("v1.2.3"));

        // A cutoff beyond every event yields nothing.
        assert!(change_events_since(&s, 1_000).unwrap().is_empty());
    }
}
