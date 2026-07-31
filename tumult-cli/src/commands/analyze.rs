//! Analytics summary rendering and the `analyze` command handler.

use std::path::Path;

use anyhow::{bail, Context, Result};

// ── Select-only query validation ──────────────────────────────
//
// Ported from `tumult-mcp/src/tools/validation.rs` (the MCP cypher/graph
// query guard) — a deliberate small duplication: the CLI must not depend on
// the MCP crate for this, and the two implementations must be kept in sync
// manually. The token scan is deliberately quote-insensitive: a forbidden
// token inside a string literal also rejects the query, trading a rare
// false positive for never missing a smuggled call.

/// Keywords that introduce a write or schema/configuration change. Rejected
/// as standalone tokens anywhere in the query, since `DuckDB` allows DML/DDL
/// after a leading `WITH` CTE (e.g. `WITH x AS (SELECT 1) INSERT INTO ...`).
const FORBIDDEN_SQL_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "ATTACH", "DETACH", "COPY", "EXPORT",
    "IMPORT", "INSTALL", "LOAD", "PRAGMA", "SET", "CALL", "VACUUM", "TRUNCATE",
];

/// `DuckDB` table functions and extension entry points that reach the host
/// filesystem, the network, or another database. A plain `SELECT` stays
/// read-only against the store, but `SELECT * FROM read_text('/etc/passwd')`
/// reads arbitrary host files and `INSTALL httpfs` loads remote extensions —
/// so these tokens are rejected exactly like the DML/DDL keywords above.
const FORBIDDEN_SQL_FUNCTIONS: &[&str] = &[
    "READ_TEXT",
    "READ_CSV",
    "READ_PARQUET",
    "READ_JSON",
    "READ_BLOB",
    "GLOB",
    "SQLITE_SCAN",
    "PARQUET_SCAN",
    "CSV_SCAN",
    "JSON_SCAN",
    "HTTPFS",
    "EXPORT_DATABASE",
    "IMPORT_DATABASE",
];

/// Validate that a `--query` SQL string is read-only (SELECT or WITH only,
/// single statement, no write keywords or filesystem table functions).
///
/// # Errors
///
/// Returns an error if the query does not start with `SELECT` or `WITH`,
/// contains more than one statement, or contains a forbidden keyword or
/// table function.
fn validate_select_only(query: &str) -> Result<()> {
    let trimmed = query.trim();
    let normalized = trimmed.to_uppercase();
    if !(normalized.starts_with("SELECT") || normalized.starts_with("WITH")) {
        bail!(
            "only SELECT/WITH queries are allowed, got: {}",
            normalized.split_whitespace().next().unwrap_or("(empty)")
        );
    }

    let without_trailing_semicolons =
        trimmed.trim_end_matches(|c: char| c.is_whitespace() || c == ';');
    if without_trailing_semicolons.contains(';') {
        bail!("only a single statement is allowed (no `;`-separated statements)");
    }

    for token in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if FORBIDDEN_SQL_KEYWORDS.contains(&token) || FORBIDDEN_SQL_FUNCTIONS.contains(&token) {
            bail!("query contains a forbidden keyword: {token}");
        }
    }

    Ok(())
}

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
        println!("No experiments found.");
        return Ok(());
    }

    for (i, exp) in experiments.iter().enumerate() {
        let exp_id = &exp[0];
        let title = &exp[1];
        let status = &exp[2];
        let duration_ms = &exp[3];

        if i > 0 {
            println!("\n{}", "─".repeat(60));
        }

        let status_marker = match status.as_str() {
            "completed" => "PASS",
            "deviated" => "DEVIATED",
            "aborted" => "ABORTED",
            "failed" => "FAIL",
            _ => status.as_str(),
        };

        println!("Experiment: {title}");
        println!("Status:     {status_marker} ({duration_ms}ms)");

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
            println!("\nTimeline:");
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

                println!(
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
            println!("\nLoad Test ({}):", lr[0]);
            println!(
                "  VUs: {}  Duration: {}s  Requests: {}",
                lr[1], lr[9], lr[7]
            );
            println!(
                "  Latency: p50={}ms  p95={}ms  p99={}ms",
                lr[3], lr[4], lr[5]
            );
            println!("  Throughput: {} req/s  Error rate: {}", lr[2], lr[6]);
            let met = if lr[8] == "true" { "PASS" } else { "FAIL" };
            println!("  Thresholds: {met}");
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
            println!("\n{}", "═".repeat(60));
            println!(
                "Store: {} experiments, {} completed, avg {}ms",
                agg[0][0], agg[0][1], agg[0][2]
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
        println!("Analytics Store Summary");
        println!("{}", "═".repeat(60));
        println!("  No experiments recorded yet.");
        println!("  Run an experiment (e.g. `tumult run experiment.toon`) to populate the store.");
        return Ok(());
    }

    let act_rows = store.query("SELECT count(*) FROM activity_results")?;
    let activities = act_rows
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    println!("Analytics Store Summary");
    println!("{}", "═".repeat(60));
    println!("  Experiments: {total}");
    println!("  Activities:  {activities}");

    // Status breakdown
    let statuses = store.query(
        "SELECT status, count(*) as cnt FROM experiments GROUP BY status ORDER BY cnt DESC",
    )?;
    if !statuses.is_empty() {
        let status_line: Vec<String> = statuses
            .iter()
            .map(|r| format!("{}={}", r[0], r[1]))
            .collect();
        println!("  By status:   {}", status_line.join("  "));
    }

    // Duration stats
    let dur = store.query(
        "SELECT cast(round(avg(duration_ms::DOUBLE), 0) as INTEGER), \
                cast(min(duration_ms) as INTEGER), \
                cast(max(duration_ms) as INTEGER) \
         FROM experiments",
    )?;
    if !dur.is_empty() && !dur[0][0].is_empty() && dur[0][0] != "NULL" {
        println!(
            "  Duration:    avg={}ms  min={}ms  max={}ms",
            dur[0][0], dur[0][1], dur[0][2]
        );
    }

    // Load tests
    let load = store.query(
        "SELECT count(*), round(avg(latency_p95_ms), 1), round(avg(error_rate), 4) \
         FROM load_results",
    )?;
    if !load.is_empty() && load[0][0] != "0" {
        println!(
            "  Load tests:  {} (avg p95={}ms, avg error_rate={})",
            load[0][0], load[0][1], load[0][2]
        );
    }

    // Top 5 longest experiments
    let top = store.query(
        "SELECT duration_ms, title, status \
         FROM experiments ORDER BY duration_ms DESC LIMIT 5",
    )?;
    if !top.is_empty() {
        println!("\nTop 5 by duration:");
        for row in &top {
            let dur_s = row[0].parse::<f64>().unwrap_or(0.0) / 1000.0;
            println!("  {dur_s:>7.1}s  {} ({})", row[1], row[2]);
        }
    }

    // Recent experiments
    let recent = store.query(
        "SELECT title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT 5",
    )?;
    if !recent.is_empty() {
        println!("\nLast 5 experiments:");
        for row in &recent {
            let status_icon = match row[1].as_str() {
                "completed" => "PASS",
                "deviated" => "DEV ",
                "aborted" => "ABRT",
                "failed" => "FAIL",
                _ => &row[1],
            };
            println!("  [{status_icon}] {}ms  {}", row[2], row[0]);
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
                                eprintln!("warning: skipping {}: {}", entry_path.display(), e);
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

    println!("Loaded {count} journal(s) into analytics store\n");

    if let Some(sql) = query {
        validate_select_only(sql)?;
        let columns = store.query_columns(sql)?;
        let rows = store.query(sql)?;
        println!("{}", columns.join("\t"));
        println!(
            "{}",
            columns
                .iter()
                .map(|c| "-".repeat(c.len().max(8)))
                .collect::<Vec<_>>()
                .join("\t")
        );
        for row in &rows {
            println!("{}", row.join("\t"));
        }
        println!("\n{} row(s)", rows.len());
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

    // ── validate_select_only ──────────────────────────────────

    #[test]
    fn select_only_allows_plain_select() {
        validate_select_only("SELECT experiment_id, title FROM experiments").unwrap();
        validate_select_only("  select count(*) from activity_results").unwrap();
        validate_select_only("WITH cte AS (SELECT 1) SELECT * FROM cte").unwrap();
        // A single trailing semicolon is tolerated.
        validate_select_only("SELECT * FROM experiments;").unwrap();
        // Identifiers merely containing a forbidden token stay allowed.
        validate_select_only("SELECT alter_ego, globe_count, loader FROM experiments").unwrap();
    }

    #[test]
    fn select_only_rejects_write_statements() {
        for sql in [
            "DELETE FROM experiments",
            "INSERT INTO experiments VALUES (1)",
            "UPDATE experiments SET title = 'x'",
            "DROP TABLE experiments",
            "CREATE TABLE foo (id int)",
            "TRUNCATE TABLE experiments",
        ] {
            let err = validate_select_only(sql).unwrap_err();
            assert!(
                err.to_string().contains("only SELECT/WITH")
                    || err.to_string().contains("forbidden keyword"),
                "{sql} must be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn select_only_rejects_stacked_statements() {
        let err = validate_select_only("SELECT 1; DROP TABLE experiments").unwrap_err();
        assert!(err.to_string().contains("single statement"), "{err}");
    }

    #[test]
    fn select_only_rejects_dml_after_cte() {
        let err = validate_select_only("WITH x AS (SELECT 1) DELETE FROM experiments").unwrap_err();
        assert!(err.to_string().contains("forbidden keyword"), "{err}");
    }

    #[test]
    fn select_only_rejects_filesystem_table_functions() {
        for sql in [
            "SELECT * FROM read_text('/etc/passwd')",
            "SELECT * FROM read_csv('data.csv')",
            "SELECT * FROM read_parquet('s3://bucket/x.parquet')",
            "SELECT * FROM read_json('/var/log/app.json')",
            "SELECT * FROM read_blob('/etc/shadow')",
            "SELECT * FROM glob('/home/*/.ssh/*')",
            "SELECT * FROM parquet_scan('x.parquet')",
            "SELECT * FROM csv_scan('x.csv')",
            "SELECT * FROM json_scan('x.json')",
            "SELECT * FROM sqlite_scan('/tmp/other.db', 'users')",
            "SELECT export_database('/tmp/out')",
            "SELECT import_database('/tmp/in')",
            "SELECT * FROM t ATTACH 'evil.db'",
            "SELECT 1 FROM t COPY TO '/tmp/out.csv'",
            "SELECT * FROM (INSTALL httpfs)",
            "WITH x AS (LOAD httpfs) SELECT 1",
        ] {
            let err = validate_select_only(sql).unwrap_err();
            assert!(
                err.to_string().contains("forbidden keyword"),
                "{sql} must be rejected, got: {err}"
            );
        }
    }

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
        cmd_analyze(
            Some(dir.path()),
            Some("SELECT experiment_id, title FROM experiments"),
            None,
            false,
        )
        .unwrap();
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
}
