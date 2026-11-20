//! Analytics summary rendering and the `analyze` command handler.

use std::path::Path;

use anyhow::{bail, Context, Result};

// ── Analyze command ───────────────────────────────────────────

/// # Errors
///
/// Returns an error if any journal cannot be read, the in-memory store cannot
/// be created, or the query fails.
/// Prints a structured summary of the last N experiments.
///
/// Shows experiment title, status, duration, method timeline with activity
/// names and durations, hypothesis results, and load test metrics if present.
#[allow(clippy::too_many_lines)] // Timeline rendering requires verbose formatting
fn print_experiment_summary(store: &tumult_lake::AnalyticsStore, last_n: usize) -> Result<()> {
    let experiments = store.query(&format!(
        "SELECT experiment_id, title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT {last_n}"
    ))?;

    if experiments.is_empty() {
        crate::outln!("No experiments found.");
        return Ok(());
    }

    for (i, exp) in experiments.iter().enumerate() {
        let exp_id = &exp[0];
        let title = &exp[1];
        let status = &exp[2];
        let duration_ms = &exp[3];

        if i > 0 {
            crate::outln!("\n{}", "─".repeat(60));
        }

        let status_marker = match status.as_str() {
            "completed" => "PASS",
            "deviated" => "DEVIATED",
            "aborted" => "ABORTED",
            "failed" => "FAIL",
            _ => status.as_str(),
        };

        crate::outln!("Experiment: {title}");
        crate::outln!("Status:     {status_marker} ({duration_ms}ms)");

        // Method timeline — the experiment id comes from the store itself,
        // but bind it as a parameter rather than interpolating it into SQL.
        let activities = store.query_with_param(
            "SELECT name, activity_type, status, duration_ms, output, phase \
             FROM activity_results \
             WHERE experiment_id = ? \
             ORDER BY started_at_ns",
            exp_id,
        )?;

        if !activities.is_empty() {
            crate::outln!("\nTimeline:");
            let total = activities.len();
            for (j, act) in activities.iter().enumerate() {
                let connector = if j == total - 1 { "└─" } else { "├─" };
                let name = &act[0];
                let act_type = &act[1];
                let act_status = &act[2];
                let act_dur = &act[3];
                let output = &act[4];
                let phase = &act[5];

                let phase_label = match phase.as_str() {
                    "hypothesis_before" => " (hypothesis before)",
                    "hypothesis_after" => " (hypothesis after)",
                    "rollback" => " (rollback)",
                    _ => "",
                };

                let status_icon = if act_status == "succeeded" {
                    ""
                } else {
                    " FAILED"
                };

                let type_label = if act_type == "probe" {
                    "probe"
                } else {
                    "action"
                };

                // Truncate output for display
                let output_preview = if output.is_empty() || output == "NULL" {
                    String::new()
                } else {
                    let trimmed = output.replace('\n', " ");
                    if trimmed.len() > 60 {
                        format!("  → {}…", &trimmed[..57])
                    } else {
                        format!("  → {trimmed}")
                    }
                };

                crate::outln!(
                    "  {connector} {name} ({type_label}){phase_label}  {act_dur}ms{status_icon}{output_preview}"
                );
            }
        }

        // Load result
        let load = store.query_with_param(
            "SELECT tool, vus, throughput_rps, latency_p50_ms, latency_p95_ms, \
                    latency_p99_ms, error_rate, total_requests, thresholds_met, duration_s \
             FROM load_results WHERE experiment_id = ?",
            exp_id,
        )?;

        if !load.is_empty() {
            let lr = &load[0];
            crate::outln!("\nLoad Test ({}):", lr[0]);
            crate::outln!(
                "  VUs: {}  Duration: {}s  Requests: {}",
                lr[1],
                lr[9],
                lr[7]
            );
            crate::outln!(
                "  Latency: p50={}ms  p95={}ms  p99={}ms",
                lr[3],
                lr[4],
                lr[5]
            );
            crate::outln!("  Throughput: {} req/s  Error rate: {}", lr[2], lr[6]);
            let met = if lr[8] == "true" { "PASS" } else { "FAIL" };
            crate::outln!("  Thresholds: {met}");
        }
    }

    // Aggregate if showing multiple
    if last_n > 1 && experiments.len() > 1 {
        let agg = store.query(
            "SELECT count(*) as total, \
             count(CASE WHEN status = 'completed' THEN 1 END) as passed, \
             avg(duration_ms) as avg_ms \
             FROM experiments",
        )?;
        if !agg.is_empty() {
            crate::outln!("\n{}", "═".repeat(60));
            crate::outln!(
                "Store: {} experiments, {} completed, avg {}ms",
                agg[0][0],
                agg[0][1],
                agg[0][2]
            );
        }
    }

    Ok(())
}

/// Prints a store-wide aggregate summary.
fn print_store_aggregate(store: &tumult_lake::AnalyticsStore) -> Result<()> {
    let total = store.experiment_count()?;

    // An empty store makes every aggregate NULL (e.g. `avg(duration_ms)`),
    // which would otherwise render as `Duration: avg=NULLms`. Surface a clean
    // message instead of a wall of NULLs.
    if total == 0 {
        crate::outln!("Analytics Store Summary");
        crate::outln!("{}", "═".repeat(60));
        crate::outln!("  No experiments recorded yet.");
        crate::outln!(
            "  Run an experiment (e.g. `tumult run experiment.toon`) to populate the store."
        );
        return Ok(());
    }

    let act_rows = store.query("SELECT count(*) FROM activity_results")?;
    let activities = act_rows
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    crate::outln!("Analytics Store Summary");
    crate::outln!("{}", "═".repeat(60));
    crate::outln!("  Experiments: {total}");
    crate::outln!("  Activities:  {activities}");

    // Status breakdown
    let statuses = store.query(
        "SELECT status, count(*) as cnt FROM experiments GROUP BY status ORDER BY cnt DESC",
    )?;
    if !statuses.is_empty() {
        let status_line: Vec<String> = statuses
            .iter()
            .map(|r| format!("{}={}", r[0], r[1]))
            .collect();
        crate::outln!("  By status:   {}", status_line.join("  "));
    }

    // Duration stats
    let dur = store.query(
        "SELECT cast(round(avg(duration_ms::DOUBLE), 0) as INTEGER), \
                cast(min(duration_ms) as INTEGER), \
                cast(max(duration_ms) as INTEGER) \
         FROM experiments",
    )?;
    if !dur.is_empty() && !dur[0][0].is_empty() && dur[0][0] != "NULL" {
        crate::outln!(
            "  Duration:    avg={}ms  min={}ms  max={}ms",
            dur[0][0],
            dur[0][1],
            dur[0][2]
        );
    }

    // Load tests
    let load = store.query(
        "SELECT count(*), round(avg(latency_p95_ms), 1), round(avg(error_rate), 4) \
         FROM load_results",
    )?;
    if !load.is_empty() && load[0][0] != "0" {
        crate::outln!(
            "  Load tests:  {} (avg p95={}ms, avg error_rate={})",
            load[0][0],
            load[0][1],
            load[0][2]
        );
    }

    // Top 5 longest experiments
    let top = store.query(
        "SELECT duration_ms, title, status \
         FROM experiments ORDER BY duration_ms DESC LIMIT 5",
    )?;
    if !top.is_empty() {
        crate::outln!("\nTop 5 by duration:");
        for row in &top {
            let dur_s = row[0].parse::<f64>().unwrap_or(0.0) / 1000.0;
            crate::outln!("  {dur_s:>7.1}s  {} ({})", row[1], row[2]);
        }
    }

    // Recent experiments
    let recent = store.query(
        "SELECT title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT 5",
    )?;
    if !recent.is_empty() {
        crate::outln!("\nLast 5 experiments:");
        for row in &recent {
            let status_icon = match row[1].as_str() {
                "completed" => "PASS",
                "deviated" => "DEV ",
                "aborted" => "ABRT",
                "failed" => "FAIL",
                _ => &row[1],
            };
            crate::outln!("  [{status_icon}] {}ms  {}", row[2], row[0]);
        }
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if the analytics store cannot be opened or the query fails.
#[must_use = "callers must handle analytics errors"]
pub fn cmd_analyze(
    journals_path: Option<&Path>,
    query: Option<&str>,
    last: Option<usize>,
    all: bool,
) -> Result<()> {
    use tumult_core::journal::read_journal;
    use tumult_lake::AnalyticsStore;

    let (store, count) = if let Some(path) = journals_path {
        let store = AnalyticsStore::in_memory()?;
        let mut count = 0;

        if path.is_file() {
            let journal = read_journal(path)
                .with_context(|| format!("failed to read journal: {}", path.display()))?;
            store.ingest_journal(&journal)?;
            count = 1;
        } else if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry_path = entry?.path();
                if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                    match read_journal(&entry_path) {
                        Ok(journal) => {
                            store.ingest_journal(&journal)?;
                            count += 1;
                        }
                        Err(e) => {
                            // Experiments and journals share the `.toon` extension. A
                            // file that parses as an experiment definition simply isn't
                            // a journal — skip it silently instead of warning about a
                            // "missing" journal field. Only genuinely malformed files
                            // (neither a valid journal nor a valid experiment) warn.
                            let is_experiment = std::fs::read_to_string(&entry_path)
                                .ok()
                                .and_then(|c| tumult_core::engine::parse_experiment(&c).ok())
                                .is_some();
                            if !is_experiment {
                                crate::errln!("warning: skipping {}: {}", entry_path.display(), e);
                            }
                        }
                    }
                }
            }
        } else {
            bail!("path does not exist: {}", path.display());
        }
        (store, count)
    } else {
        // Use persistent store
        let db_path =
            AnalyticsStore::default_path().context("failed to determine analytics store path")?;
        if !db_path.exists() {
            bail!(
                "no persistent store found at {}. Run experiments first or specify a journals path.",
                db_path.display()
            );
        }
        // Every subpath of this command (`--query`, `--all`, the default
        // summary) only reads: open the store read-only so a raw `--query`
        // can never write to the store, and so the command can coexist with
        // another process (e.g. the MCP server) holding the write lock.
        let store = AnalyticsStore::open_read_only(&db_path)?;
        let count = store.experiment_count()?;
        (store, count)
    };

    crate::outln!("Loaded {count} journal(s) into analytics store\n");

    if let Some(sql) = query {
        tumult_mcp::tools::validate_select_only(sql)?;
        let columns = store.query_columns(sql)?;
        let rows = store.query(sql)?;
        crate::outln!("{}", columns.join("\t"));
        crate::outln!(
            "{}",
            columns
                .iter()
                .map(|c| "-".repeat(c.len().max(8)))
                .collect::<Vec<_>>()
                .join("\t")
        );
        for row in &rows {
            crate::outln!("{}", row.join("\t"));
        }
        crate::outln!("\n{} row(s)", rows.len());
    } else if all {
        print_store_aggregate(&store)?;
    } else {
        print_experiment_summary(&store, last.unwrap_or(1))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::testing::capture;

    // ── cmd_analyze --query against an in-memory journal store ──

    /// Write `journal` as `name.toon` into a fresh tempdir.
    fn journal_dir(journal: &tumult_core::types::Journal) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("run-1.toon");
        tumult_core::journal::write_journal(journal, &path).unwrap();
        dir
    }

    fn sample_journal() -> tumult_core::types::Journal {
        tumult_core::types::Journal {
            target_version: None,
            target: None,
            experiment_title: "analyze test".into(),
            experiment_id: "exp-analyze-1".into(),
            status: tumult_core::types::ExperimentStatus::Completed,
            started_at_ns: 1,
            ended_at_ns: 2,
            duration_ms: 1,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            kind: tumult_core::types::TestKind::default(),
            objectives: None,
            probe_samples: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    #[test]
    fn query_runs_normal_select() {
        let dir = journal_dir(&sample_journal());

        let (result, printed) = capture(|| {
            cmd_analyze(
                Some(dir.path()),
                Some("SELECT experiment_id, title FROM experiments"),
                None,
                false,
            )
        });

        result.unwrap();
        assert_eq!(
            printed.stdout,
            "Loaded 1 journal(s) into analytics store\n\n\
             experiment_id\ttitle\n\
             -------------\t--------\n\
             exp-analyze-1\tanalyze test\n\n\
             1 row(s)\n"
        );
    }

    #[test]
    fn query_rejects_delete() {
        let dir = journal_dir(&sample_journal());
        let err = cmd_analyze(
            Some(dir.path()),
            Some("DELETE FROM experiments"),
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("only SELECT/WITH"), "{err}");
    }

    #[test]
    fn query_rejects_read_text() {
        let dir = journal_dir(&sample_journal());
        let err = cmd_analyze(
            Some(dir.path()),
            Some("SELECT * FROM read_text('/etc/passwd')"),
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("forbidden keyword"), "{err}");
    }

    // ── cmd_analyze summary/aggregate rendering ───────────────

    use tumult_core::types::{
        ActivityResult, ActivityStatus, ActivityType, ExperimentStatus, HypothesisResult, Journal,
        LoadResult, LoadTool, SpanId, TraceId,
    };

    fn activity(
        name: &str,
        activity_type: ActivityType,
        status: ActivityStatus,
        output: Option<String>,
    ) -> ActivityResult {
        ActivityResult {
            name: name.into(),
            activity_type,
            status,
            started_at_ns: 1,
            duration_ms: 250,
            output,
            error: None,
            trace_id: TraceId::empty(),
            span_id: SpanId::empty(),
        }
    }

    /// A journal exercising every timeline branch: hypothesis before/after,
    /// a failed probe, a long output that must be truncated, a rollback
    /// phase, and a load result block.
    fn rich_journal(id: &str, started_at_ns: i64) -> Journal {
        Journal {
            target_version: None,
            target: None,
            experiment_title: format!("rich {id}"),
            experiment_id: id.into(),
            status: ExperimentStatus::Deviated,
            started_at_ns,
            ended_at_ns: started_at_ns + 60_000_000_000,
            duration_ms: 60_000,
            steady_state_before: Some(HypothesisResult {
                title: "steady before".into(),
                met: true,
                probe_results: vec![activity(
                    "health",
                    ActivityType::Probe,
                    ActivityStatus::Succeeded,
                    Some("ok".into()),
                )],
            }),
            steady_state_after: Some(HypothesisResult {
                title: "steady after".into(),
                met: false,
                probe_results: vec![activity(
                    "health",
                    ActivityType::Probe,
                    ActivityStatus::Failed,
                    None,
                )],
            }),
            method_results: vec![
                activity(
                    "inject",
                    ActivityType::Action,
                    ActivityStatus::Succeeded,
                    // >60 chars so the preview truncation branch runs.
                    Some("x".repeat(120)),
                ),
                activity("verify", ActivityType::Probe, ActivityStatus::Failed, None),
            ],
            rollback_results: vec![activity(
                "undo",
                ActivityType::Action,
                ActivityStatus::Succeeded,
                None,
            )],
            rollback_failures: 0,
            kind: tumult_core::types::TestKind::default(),
            objectives: None,
            probe_samples: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: Some(LoadResult {
                tool: LoadTool::K6,
                started_at_ns,
                ended_at_ns: started_at_ns + 30_000_000_000,
                duration_s: 30.0,
                vus: 10,
                throughput_rps: 100.0,
                latency_p50_ms: 12.0,
                latency_p95_ms: 45.0,
                latency_p99_ms: 80.0,
                error_rate: 0.01,
                total_requests: 3000,
                thresholds_met: true,
                samples: vec![],
                execution: tumult_core::types::LoadExecution::default(),
            }),
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    #[test]
    fn summary_renders_full_timeline_and_load_block() {
        let dir = tempfile::TempDir::new().unwrap();
        tumult_core::journal::write_journal(
            &rich_journal("rich-1", 1_774_980_000_000_000_000),
            &dir.path().join("run-1.toon"),
        )
        .unwrap();
        tumult_core::journal::write_journal(
            &rich_journal("rich-2", 1_774_980_060_000_000_000),
            &dir.path().join("run-2.toon"),
        )
        .unwrap();

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, Some(5), false));

        result.unwrap();
        let out = printed.stdout;
        // Newest first.
        let newer = out.find("Experiment: rich rich-2\n").expect("rich-2");
        let older = out.find("Experiment: rich rich-1\n").expect("rich-1");
        assert!(newer < older, "{out}");
        assert!(out.contains("Status:     DEVIATED (60000ms)\n"), "{out}");
        assert!(
            out.contains("health (probe) (hypothesis before)  250ms  → ok\n"),
            "{out}"
        );
        assert!(
            out.contains("health (probe) (hypothesis after)  250ms FAILED\n"),
            "{out}"
        );
        // A long output is cut to 57 characters and an ellipsis.
        let preview = format!("inject (action)  250ms  → {}…\n", "x".repeat(57));
        assert!(out.contains(&preview), "{out}");
        assert!(
            out.contains("└─ undo (action) (rollback)  250ms\n"),
            "{out}"
        );
        assert!(out.contains("\nLoad Test (k6):\n"), "{out}");
        assert!(
            out.contains("  Latency: p50=12.0000ms  p95=45.0000ms  p99=80.0000ms\n"),
            "{out}"
        );
        assert!(out.contains("  Thresholds: PASS\n"), "{out}");
        // last > 1 with several experiments also prints the store aggregate.
        assert!(
            out.contains("Store: 2 experiments, 0 completed, avg "),
            "{out}"
        );
    }

    #[test]
    fn a_summary_of_one_experiment_has_no_store_aggregate() {
        let dir = journal_dir(&rich_journal("only", 1_774_980_000_000_000_000));

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, Some(5), false));

        result.unwrap();
        let out = printed.stdout;
        assert!(out.contains("Experiment: rich only\n"), "{out}");
        assert!(!out.contains("Store: "), "{out}");
    }

    #[test]
    fn summary_on_empty_store_reports_no_experiments() {
        let dir = tempfile::TempDir::new().unwrap();

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, None, false));

        result.unwrap();
        assert_eq!(
            printed.stdout,
            "Loaded 0 journal(s) into analytics store\n\nNo experiments found.\n"
        );
    }

    #[test]
    fn aggregate_renders_store_wide_stats() {
        let dir = journal_dir(&rich_journal("agg-1", 1_774_980_000_000_000_000));

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, None, true));

        result.unwrap();
        let out = printed.stdout;
        assert!(out.contains("Analytics Store Summary\n"), "{out}");
        assert!(out.contains("  Experiments: 1\n"), "{out}");
        // Two hypothesis probes, two method steps and one rollback.
        assert!(out.contains("  Activities:  5\n"), "{out}");
        assert!(out.contains("  By status:   deviated=1\n"), "{out}");
        assert!(
            out.contains("  Duration:    avg=60000ms  min=60000ms  max=60000ms\n"),
            "{out}"
        );
        assert!(out.contains("  Load tests:  1 (avg p95=45"), "{out}");
        assert!(out.contains("   60.0s  rich agg-1 (deviated)\n"), "{out}");
        assert!(out.contains("  [DEV ] 60000ms  rich agg-1\n"), "{out}");
    }

    #[test]
    fn aggregate_on_empty_store_reports_absence() {
        let dir = tempfile::TempDir::new().unwrap();

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, None, true));

        result.unwrap();
        let out = printed.stdout;
        assert!(out.contains("  No experiments recorded yet.\n"), "{out}");
        // No aggregate over nothing: that would print NULLs.
        assert!(!out.contains("Duration:"), "{out}");
        assert!(!out.contains("NULL"), "{out}");
    }

    #[test]
    fn analyze_accepts_a_single_journal_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("single.toon");
        tumult_core::journal::write_journal(&sample_journal(), &path).unwrap();

        let (result, printed) = capture(|| cmd_analyze(Some(&path), None, None, false));

        result.unwrap();
        assert_eq!(
            printed.stdout,
            "Loaded 1 journal(s) into analytics store\n\n\
             Experiment: analyze test\n\
             Status:     PASS (1ms)\n"
        );
    }

    #[test]
    fn analyze_missing_path_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = cmd_analyze(Some(&dir.path().join("nope")), None, None, false).unwrap_err();
        assert!(err.to_string().contains("path does not exist"), "{err}");
    }

    #[test]
    fn analyze_skips_experiment_definitions_and_warns_on_malformed_toon() {
        let dir = tempfile::TempDir::new().unwrap();
        tumult_core::journal::write_journal(&sample_journal(), &dir.path().join("run.toon"))
            .unwrap();
        // A valid experiment definition: skipped silently (not a journal).
        std::fs::write(
            dir.path().join("experiment.toon"),
            toon_format::encode_default(&tumult_core::types::Experiment::default()).unwrap(),
        )
        .unwrap();
        // Neither journal nor experiment: skipped with a warning.
        let broken = dir.path().join("broken.toon");
        std::fs::write(&broken, "not valid {{{").unwrap();

        let (result, printed) = capture(|| cmd_analyze(Some(dir.path()), None, None, false));

        result.unwrap();
        assert!(
            printed
                .stdout
                .starts_with("Loaded 1 journal(s) into analytics store\n"),
            "{}",
            printed.stdout
        );
        let warnings: Vec<&str> = printed.stderr.lines().collect();
        assert_eq!(warnings.len(), 1, "{}", printed.stderr);
        assert!(
            warnings[0].starts_with(&format!("warning: skipping {}: ", broken.display())),
            "{}",
            printed.stderr
        );
    }
}
