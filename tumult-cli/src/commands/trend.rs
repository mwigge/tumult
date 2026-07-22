//! Journal export and metric-trend command handlers.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::ExportFormat;

// ── Export command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the journal cannot be read or the export operation fails.
#[must_use = "callers must handle export errors"]
pub fn cmd_export(journal_path: &Path, format: ExportFormat) -> Result<()> {
    use tumult_analytics::arrow_convert::journal_to_record_batch;
    use tumult_analytics::export::{export_arrow_ipc, export_csv, export_parquet};
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let ext = match format {
        ExportFormat::Parquet => "parquet",
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
        ExportFormat::Arrow => "arrow",
    };
    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("journal");
    let out_path = std::path::PathBuf::from(format!("{stem}.{ext}"));

    match format {
        ExportFormat::Parquet | ExportFormat::Csv | ExportFormat::Arrow => {
            let (exp_batch, _) = journal_to_record_batch(std::slice::from_ref(&journal))?;
            match format {
                ExportFormat::Parquet => export_parquet(&exp_batch, &out_path)?,
                ExportFormat::Csv => export_csv(&exp_batch, &out_path)?,
                ExportFormat::Arrow => export_arrow_ipc(&exp_batch, &out_path)?,
                ExportFormat::Json => unreachable!(),
            }
        }
        ExportFormat::Json => {
            let json = serde_json::to_string_pretty(&journal)?;
            std::fs::write(&out_path, json)?;
        }
    }
    println!("Exported to: {}", out_path.display());
    Ok(())
}

// ── Trend command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)] // Multi-probe trend analysis output requires verbose formatting across multiple metric types
#[must_use = "callers must handle trend analysis errors"]
pub fn cmd_trend(
    journals_path: &Path,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    println!("Loaded {count} journal(s)\n");

    let valid_metrics = [
        "resilience_score",
        "duration_ms",
        "estimate_accuracy",
        "method_step_count",
    ];
    if !valid_metrics.contains(&metric) {
        bail!(
            "unknown metric: {}. Valid: {}",
            metric,
            valid_metrics.join(", ")
        );
    }

    // Parse --last flag into nanosecond cutoff
    let time_filter = if let Some(window) = last {
        let days: i64 = window.trim_end_matches('d').parse().with_context(|| {
            format!("--last must be a number of days (e.g., 30d), got: {window}")
        })?;
        let cutoff_ns =
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) - (days * 86400 * 1_000_000_000);
        format!(" AND started_at_ns >= {cutoff_ns}")
    } else {
        String::new()
    };

    // Pre-built queries keyed by metric — no format! interpolation (DB-03)
    let base_sql = match metric {
        "resilience_score" => "SELECT experiment_id, title, status, resilience_score, started_at_ns FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT experiment_id, title, status, duration_ms, started_at_ns FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT experiment_id, title, status, estimate_accuracy, started_at_ns FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT experiment_id, title, status, method_step_count, started_at_ns FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let target_filter = if target.is_some() {
        // Bind the LIKE pattern as a query parameter to prevent SQL injection.
        " AND lower(title) LIKE ?"
    } else {
        ""
    };
    let sql = format!("{base_sql}{time_filter}{target_filter} ORDER BY started_at_ns");

    let (columns, rows) = if let Some(t) = target {
        let like_pattern = format!("%{}%", t.to_lowercase());
        // Fetch column names from the base SQL (schema is identical regardless of filter).
        let columns = store.query_columns(base_sql)?;
        let rows = store.query_with_param(&sql, &like_pattern)?;
        (columns, rows)
    } else {
        let columns = store.query_columns(&sql)?;
        let rows = store.query(&sql)?;
        (columns, rows)
    };

    if rows.is_empty() {
        println!("No data points for metric: {metric}");
        return Ok(());
    }

    println!("Trend: {} ({} data points)\n", metric, rows.len());
    println!(
        "{}",
        columns.iter().fold(String::new(), |mut s, c| {
            let _ = write!(s, "{c:<20}");
            s
        })
    );
    println!("{}", "-".repeat(columns.len() * 20));
    for row in &rows {
        println!(
            "{}",
            row.iter().fold(String::new(), |mut s, v| {
                let _ = write!(s, "{v:<20}");
                s
            })
        );
    }

    // Summary stats — pre-built per metric
    let stats_sql = match metric {
        "resilience_score" => "SELECT count(*) as runs, min(resilience_score) as min, max(resilience_score) as max, avg(resilience_score) as avg FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT count(*) as runs, min(duration_ms) as min, max(duration_ms) as max, avg(duration_ms) as avg FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT count(*) as runs, min(estimate_accuracy) as min, max(estimate_accuracy) as max, avg(estimate_accuracy) as avg FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT count(*) as runs, min(method_step_count) as min, max(method_step_count) as max, avg(method_step_count) as avg FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let stats = store.query(stats_sql)?;
    if let Some(row) = stats.first() {
        println!(
            "\nSummary: {} runs, min={}, max={}, avg={}",
            row[0], row[1], row[2], row[3]
        );
    }

    Ok(())
}
