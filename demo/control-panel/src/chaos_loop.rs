//! The chaos-loop showcase: driving discover → validate → run → analyze →
//! recommend as five separate MCP tool calls.

use serde::Serialize;
use serde_json::json;

use crate::mcp::{ChaosLoopClient, McpError};

/// SQL the chaos-loop's analyze step runs over the persistent analytics store:
/// the five most recent experiments with their status and duration.
const LOOP_ANALYZE_SQL: &str =
    "SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC LIMIT 5";

/// One step of the full discover→validate→run→analyze→recommend loop, as
/// rendered on the UI timeline. Each step is exactly one MCP `tools/call`.
#[derive(Serialize, Clone)]
pub(crate) struct LoopStep {
    /// 1-based position in the sequence.
    index: usize,
    /// Human step name, e.g. "Discover".
    name: String,
    /// The MCP tool this step invoked, e.g. "tumult_discover".
    tool: String,
    /// "ok" | "error".
    status: String,
    /// Wall-clock time this single MCP call took.
    elapsed_ms: u64,
    /// One-line result summary for the timeline.
    summary: String,
    /// Structured payload for the step (counts, table rows, recommendations…).
    detail: serde_json::Value,
    /// Present only when `status == "error"`.
    error: Option<String>,
}

impl LoopStep {
    fn ok(
        index: usize,
        name: &str,
        tool: &str,
        elapsed_ms: u64,
        summary: String,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            index,
            name: name.to_string(),
            tool: tool.to_string(),
            status: "ok".to_string(),
            elapsed_ms,
            summary,
            detail,
            error: None,
        }
    }

    fn failed(index: usize, name: &str, tool: &str, elapsed_ms: u64, err: &McpError) -> Self {
        Self {
            index,
            name: name.to_string(),
            tool: tool.to_string(),
            status: "error".to_string(),
            elapsed_ms,
            summary: err.to_string(),
            detail: json!({}),
            error: Some(err.to_string()),
        }
    }
}

/// Full result of one chaos-loop run.
#[derive(Serialize)]
pub(crate) struct LoopReport {
    /// True only when all five steps completed successfully.
    pub(crate) ok: bool,
    pub(crate) experiment_path: String,
    pub(crate) steps: Vec<LoopStep>,
    /// Deep link to SigNoz traces for the run step.
    pub(crate) signoz_trace_link: String,
}

/// Drive discover → validate → run → analyze → recommend as five separate MCP
/// tool calls, stopping at the first failure. Never panics; a failing step is
/// recorded and the loop returns early with `ok == false`. Generic over the
/// client so the orchestration is unit-tested against a mock.
pub(crate) async fn run_chaos_loop<C: ChaosLoopClient>(
    client: &C,
    experiment_path: &str,
) -> (bool, Vec<LoopStep>) {
    use std::time::Instant;
    let mut steps = Vec::with_capacity(5);

    // 1 · Discover
    let t = Instant::now();
    match client.discover().await {
        Ok(d) => steps.push(LoopStep::ok(
            1,
            "Discover",
            "tumult_discover",
            elapsed_ms(t),
            format!("{} plugins · {} actions available", d.plugins, d.actions),
            json!({ "plugins": d.plugins, "actions": d.actions }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(
                1,
                "Discover",
                "tumult_discover",
                elapsed_ms(t),
                &e,
            ));
            return (false, steps);
        }
    }

    // 2 · Validate
    let t = Instant::now();
    match client.validate(experiment_path).await {
        Ok(v) => steps.push(LoopStep::ok(
            2,
            "Validate",
            "tumult_validate",
            elapsed_ms(t),
            format!(
                "{} · {} method step{}",
                if v.valid { "valid" } else { "invalid" },
                v.method_steps,
                if v.method_steps == 1 { "" } else { "s" }
            ),
            json!({
                "valid": v.valid,
                "title": v.title,
                "method_steps": v.method_steps,
                "rollbacks": v.rollbacks,
            }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(
                2,
                "Validate",
                "tumult_validate",
                elapsed_ms(t),
                &e,
            ));
            return (false, steps);
        }
    }

    // 3 · Run
    let t = Instant::now();
    match client.run_experiment(experiment_path).await {
        Ok(r) => {
            let dur = r
                .duration_ms
                .map_or_else(|| "—".to_string(), |d| format!("{d} ms"));
            steps.push(LoopStep::ok(
                3,
                "Run",
                "tumult_run_experiment",
                elapsed_ms(t),
                format!("{} · {}", r.status, dur),
                json!({
                    "status": r.status,
                    "outcome": r.outcome,
                    "duration_ms": r.duration_ms,
                    "journal_path": r.journal_path,
                    "ingestion": r.ingestion,
                }),
            ));
        }
        Err(e) => {
            steps.push(LoopStep::failed(
                3,
                "Run",
                "tumult_run_experiment",
                elapsed_ms(t),
                &e,
            ));
            return (false, steps);
        }
    }

    // 4 · Analyze
    let t = Instant::now();
    match client.analyze_store(LOOP_ANALYZE_SQL).await {
        Ok(table) => steps.push(LoopStep::ok(
            4,
            "Analyze",
            "tumult_analyze_store",
            elapsed_ms(t),
            format!(
                "{} recent experiment{}",
                table.row_count,
                if table.row_count == 1 { "" } else { "s" }
            ),
            json!({
                "sql": LOOP_ANALYZE_SQL,
                "columns": table.columns,
                "rows": table.rows,
                "row_count": table.row_count,
            }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(
                4,
                "Analyze",
                "tumult_analyze_store",
                elapsed_ms(t),
                &e,
            ));
            return (false, steps);
        }
    }

    // 5 · Recommend
    let t = Instant::now();
    match client.recommend().await {
        Ok(rec) => {
            let summary = if let Some(msg) = &rec.message {
                msg.clone()
            } else if let Some(top) = rec.recommendations.first() {
                format!(
                    "{} recommendation{} · top: {}",
                    rec.recommendations.len(),
                    if rec.recommendations.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    top.title
                )
            } else {
                "no recommendations".to_string()
            };
            steps.push(LoopStep::ok(
                5,
                "Recommend",
                "tumult_recommend",
                elapsed_ms(t),
                summary,
                json!({
                    "message": rec.message,
                    "recommendations": rec.recommendations,
                }),
            ));
        }
        Err(e) => {
            steps.push(LoopStep::failed(
                5,
                "Recommend",
                "tumult_recommend",
                elapsed_ms(t),
                &e,
            ));
            return (false, steps);
        }
    }

    (true, steps)
}

fn elapsed_ms(t: std::time::Instant) -> u64 {
    // Saturating cast is fine: step durations never approach u64::MAX ms.
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{
        ChaosLoopClient, DiscoverOutcome, McpError, RecommendOutcome, Recommendation, RunOutcome,
        TableOutcome, ValidateOutcome,
    };

    /// A canned [`ChaosLoopClient`] for testing the orchestration without a
    /// live MCP server. `fail_step` (1-5) makes that step error; `offline`
    /// makes the failure an `Unreachable` (as a down MCP server would).
    struct MockClient {
        fail_step: usize,
        offline: bool,
        run_status: &'static str,
    }

    impl MockClient {
        fn happy() -> Self {
            Self {
                fail_step: 0,
                offline: false,
                run_status: "completed",
            }
        }
        fn failing_at(step: usize) -> Self {
            Self {
                fail_step: step,
                offline: false,
                run_status: "completed",
            }
        }
        fn offline() -> Self {
            Self {
                fail_step: 1,
                offline: true,
                run_status: "completed",
            }
        }
        fn err(&self) -> McpError {
            if self.offline {
                McpError::Unreachable("connection refused".into())
            } else {
                McpError::Protocol("injected step failure".into())
            }
        }
    }

    impl ChaosLoopClient for MockClient {
        async fn discover(&self) -> Result<DiscoverOutcome, McpError> {
            if self.fail_step == 1 {
                return Err(self.err());
            }
            Ok(DiscoverOutcome {
                plugins: 12,
                actions: 34,
            })
        }
        async fn validate(&self, _experiment_path: &str) -> Result<ValidateOutcome, McpError> {
            if self.fail_step == 2 {
                return Err(self.err());
            }
            Ok(ValidateOutcome {
                valid: true,
                title: Some("Kill Postgres connections".into()),
                method_steps: 3,
                rollbacks: 1,
                summary: "Valid: 'Kill Postgres connections' — 3 method steps, 1 rollbacks".into(),
            })
        }
        async fn run_experiment(&self, _experiment_path: &str) -> Result<RunOutcome, McpError> {
            if self.fail_step == 3 {
                return Err(self.err());
            }
            Ok(RunOutcome {
                outcome: crate::mcp::verdict_for(self.run_status).to_string(),
                status: self.run_status.to_string(),
                duration_ms: Some(228),
                journal_path: Some("/demo/journals/demo-postgres.toon".into()),
                ingestion: Some("ingested".into()),
            })
        }
        async fn analyze_store(&self, _query: &str) -> Result<TableOutcome, McpError> {
            if self.fail_step == 4 {
                return Err(self.err());
            }
            Ok(TableOutcome {
                columns: vec!["title".into(), "status".into(), "duration_ms".into()],
                rows: vec![vec![
                    "Kill connections".into(),
                    "completed".into(),
                    "228".into(),
                ]],
                row_count: 1,
            })
        }
        async fn recommend(&self) -> Result<RecommendOutcome, McpError> {
            if self.fail_step == 5 {
                return Err(self.err());
            }
            Ok(RecommendOutcome {
                message: None,
                recommendations: vec![Recommendation {
                    rank: 1,
                    title: "Test Postgres failover".into(),
                    rationale: "never exercised".into(),
                }],
            })
        }
    }

    #[tokio::test]
    async fn happy_path_runs_all_five_steps_in_order() {
        let (ok, steps) = run_chaos_loop(&MockClient::happy(), "demo-postgres.toon").await;
        assert!(ok);
        assert_eq!(steps.len(), 5);
        let tools: Vec<&str> = steps.iter().map(|s| s.tool.as_str()).collect();
        assert_eq!(
            tools,
            vec![
                "tumult_discover",
                "tumult_validate",
                "tumult_run_experiment",
                "tumult_analyze_store",
                "tumult_recommend",
            ]
        );
        assert!(steps.iter().all(|s| s.status == "ok" && s.error.is_none()));
        // Discover step carries the counts.
        assert_eq!(steps[0].detail["plugins"], 12);
        // Recommend step surfaces the top recommendation title in its summary.
        assert!(steps[4].summary.contains("Test Postgres failover"));
    }

    #[tokio::test]
    async fn run_step_surfaces_halted_status() {
        let mut client = MockClient::happy();
        client.run_status = "halted";
        let (ok, steps) = run_chaos_loop(&client, "demo-postgres.toon").await;
        assert!(ok);
        let run = &steps[2];
        assert_eq!(run.detail["status"], "halted");
        assert_eq!(run.detail["outcome"], "halted");
        assert!(run.summary.starts_with("halted"));
    }

    #[tokio::test]
    async fn mid_loop_failure_stops_and_reports_failure() {
        let (ok, steps) = run_chaos_loop(&MockClient::failing_at(3), "demo-postgres.toon").await;
        assert!(!ok);
        // Steps 1-2 ran; step 3 errored; 4-5 never ran.
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].tool, "tumult_run_experiment");
        assert_eq!(steps[2].status, "error");
        assert!(steps[2].error.is_some());
        assert!(steps[0].status == "ok" && steps[1].status == "ok");
    }

    #[tokio::test]
    async fn mcp_offline_fails_cleanly_at_first_step() {
        let (ok, steps) = run_chaos_loop(&MockClient::offline(), "demo-postgres.toon").await;
        assert!(!ok);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].status, "error");
        assert!(steps[0]
            .error
            .as_deref()
            .unwrap()
            .to_lowercase()
            .contains("unreachable"));
    }
}
