//! Journal export and metric-trend command handlers.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::ExportFormat;

// ── Export command ─────────────────────────────────────────────

/// The directory, in the working directory, of the files of steps. The runs
/// and the steps are two tables, so each has its own directory: a reader that
/// takes every file of a directory as one table gets the runs from the
/// working directory and the steps from this one.
const STEPS_DIR: &str = "activities";

/// The extension of the files `format` writes.
fn extension(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Parquet => "parquet",
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
        ExportFormat::Arrow => "arrow",
    }
}

/// Write the whole journal as JSON to `out_path`.
fn export_json(journal: &tumult_core::types::Journal, out_path: &Path) -> Result<()> {
    std::fs::write(out_path, serde_json::to_string_pretty(journal)?)?;
    crate::outln!("Exported to: {}", out_path.display());
    Ok(())
}

/// Write the journal of a run to the working directory as `<stem>.<ext>`.
///
/// JSON is the whole journal in one file. Parquet, CSV and Arrow are tables:
/// the run is one row of `<stem>.<ext>` and its steps are the rows of
/// `activities/<stem>.<ext>`, which is written also when the run has no
/// step.
///
/// # Errors
///
/// Returns an error if the journal cannot be read or the export operation fails.
#[must_use = "callers must handle export errors"]
pub fn cmd_export(journal_path: &Path, format: ExportFormat) -> Result<()> {
    use tumult_core::journal::read_journal;
    use tumult_lake::arrow_convert::journal_to_record_batch;
    use tumult_lake::export::{export_arrow_ipc, export_csv, export_parquet};

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;
    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("journal");
    let file = format!("{stem}.{}", extension(format));

    let write_table = match format {
        ExportFormat::Json => return export_json(&journal, Path::new(&file)),
        ExportFormat::Parquet => export_parquet,
        ExportFormat::Csv => export_csv,
        ExportFormat::Arrow => export_arrow_ipc,
    };
    let (run, steps) = journal_to_record_batch(std::slice::from_ref(&journal))?;
    std::fs::create_dir_all(STEPS_DIR)
        .with_context(|| format!("failed to create the directory {STEPS_DIR}"))?;
    let tables = [
        (run, std::path::PathBuf::from(&file)),
        (steps, Path::new(STEPS_DIR).join(&file)),
    ];
    for (table, out_path) in tables {
        write_table(&table, &out_path)?;
        crate::outln!("Exported to: {}", out_path.display());
    }
    Ok(())
}

// ── Trend command ─────────────────────────────────────────────

/// The metrics a trend can be of.
const TREND_METRICS: [&str; 4] = [
    "resilience_score",
    "duration_ms",
    "estimate_accuracy",
    "method_step_count",
];

/// The queries of a trend of `metric`: its points, oldest first, and their
/// summary. `None` for a metric a trend cannot be of.
///
/// They read every run of the store, which holds the journals of the scope
/// only (see [`load_in_scope`]): the points and their summary are over the
/// same runs.
fn trend_queries(metric: &str) -> Option<(&'static str, &'static str)> {
    // Pre-built queries keyed by metric — no format! interpolation (DB-03)
    Some(match metric {
        "resilience_score" => (
            "SELECT experiment_id, title, status, resilience_score, started_at_ns FROM experiments WHERE resilience_score IS NOT NULL ORDER BY started_at_ns",
            "SELECT count(*) as runs, min(resilience_score) as min, max(resilience_score) as max, avg(resilience_score) as avg FROM experiments WHERE resilience_score IS NOT NULL",
        ),
        "duration_ms" => (
            "SELECT experiment_id, title, status, duration_ms, started_at_ns FROM experiments WHERE duration_ms IS NOT NULL ORDER BY started_at_ns",
            "SELECT count(*) as runs, min(duration_ms) as min, max(duration_ms) as max, avg(duration_ms) as avg FROM experiments WHERE duration_ms IS NOT NULL",
        ),
        "estimate_accuracy" => (
            "SELECT experiment_id, title, status, estimate_accuracy, started_at_ns FROM experiments WHERE estimate_accuracy IS NOT NULL ORDER BY started_at_ns",
            "SELECT count(*) as runs, min(estimate_accuracy) as min, max(estimate_accuracy) as max, avg(estimate_accuracy) as avg FROM experiments WHERE estimate_accuracy IS NOT NULL",
        ),
        "method_step_count" => (
            "SELECT experiment_id, title, status, method_step_count, started_at_ns FROM experiments WHERE method_step_count IS NOT NULL ORDER BY started_at_ns",
            "SELECT count(*) as runs, min(method_step_count) as min, max(method_step_count) as max, avg(method_step_count) as avg FROM experiments WHERE method_step_count IS NOT NULL",
        ),
        _ => return None,
    })
}

/// The start of the window `--last` asks for, in epoch nanoseconds. `None`
/// when it asks for none.
fn window_start_ns(last: Option<&str>) -> Result<Option<i64>> {
    const DAY_NS: i64 = 86_400 * 1_000_000_000;

    let Some(window) = last else {
        return Ok(None);
    };
    let days: i64 = window
        .trim_end_matches('d')
        .parse()
        .with_context(|| format!("--last must be a number of days (e.g., 30d), got: {window}"))?;
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    Ok(Some(now_ns.saturating_sub(days.saturating_mul(DAY_NS))))
}

/// Read the journals at `journals_path`, one journal or a directory of
/// them, and put the ones in `scope` into `store`. The number of journals
/// read, in the scope or not.
fn load_in_scope(
    journals_path: &Path,
    scope: &mut tumult_core::journal::JournalScope,
    store: &tumult_lake::AnalyticsStore,
) -> Result<usize> {
    use tumult_core::journal::read_journal;

    if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        if scope.admit(&journal) {
            store.ingest_journal(&journal)?;
        }
        return Ok(1);
    }
    if !journals_path.is_dir() {
        bail!("path does not exist: {}", journals_path.display());
    }
    let mut count = 0;
    for entry in std::fs::read_dir(journals_path)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toon") {
            continue;
        }
        match read_journal(&path) {
            Ok(journal) => {
                count += 1;
                if scope.admit(&journal) {
                    store.ingest_journal(&journal)?;
                }
            }
            Err(e) => super::events::journal_skipped(&path, &e),
        }
    }
    Ok(count)
}

/// One line of a table: each cell in a column 20 wide.
fn table_line(cells: &[String]) -> String {
    cells.iter().fold(String::new(), |mut line, cell| {
        let _ = write!(line, "{cell:<20}");
        line
    })
}

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[must_use = "callers must handle trend analysis errors"]
pub fn cmd_trend(
    journals_path: &Path,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    use tumult_core::journal::JournalScope;
    use tumult_lake::AnalyticsStore;

    let mut scope = JournalScope::new(window_start_ns(last)?, target);
    let store = AnalyticsStore::in_memory()?;
    let count = load_in_scope(journals_path, &mut scope, &store)?;

    crate::outln!("Loaded {count} journal(s)");
    if let Some(note) = scope.left_out_note() {
        crate::outln!("{note}");
    }
    crate::outln!();

    let Some((points_sql, summary_sql)) = trend_queries(metric) else {
        bail!(
            "unknown metric: {}. Valid: {}",
            metric,
            TREND_METRICS.join(", ")
        );
    };
    let columns = store.query_columns(points_sql)?;
    let rows = store.query(points_sql)?;

    if rows.is_empty() {
        crate::outln!("No data points for metric: {metric}");
        return Ok(());
    }

    crate::outln!("Trend: {} ({} data points)\n", metric, rows.len());
    crate::outln!("{}", table_line(&columns));
    crate::outln!("{}", "-".repeat(columns.len() * 20));
    for row in &rows {
        crate::outln!("{}", table_line(row));
    }

    let summary = store.query(summary_sql)?;
    if let Some(row) = summary.first() {
        crate::outln!(
            "\nSummary: {} runs, min={}, max={}, avg={}",
            row[0],
            row[1],
            row[2],
            row[3]
        );
    }

    Ok(())
}
