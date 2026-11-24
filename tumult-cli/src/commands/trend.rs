//! Journal export and metric-trend command handlers.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use tumult_lake::{RecoveryMetric, RecoveryTrend};

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

/// The metrics a trend can be of: a column of each run, then the
/// recovery metrics (see [`RecoveryMetric`]).
const TREND_METRICS: [&str; 8] = [
    "resilience_score",
    "duration_ms",
    "estimate_accuracy",
    "method_step_count",
    "rto_measured_s",
    "rto_margin_s",
    "rpo_measured_s",
    "rpo_margin_s",
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

/// What a trend is of.
enum Series {
    /// A column of each run: the queries of [`trend_queries`].
    Column(&'static str, &'static str),
    /// The RTO or the RPO each run measured, or its margin.
    Recovery(RecoveryMetric),
}

/// The series of `metric`.
fn series(metric: &str) -> Result<Series> {
    if let Some(recovery) = RecoveryMetric::from_name(metric) {
        return Ok(Series::Recovery(recovery));
    }
    let Some((points_sql, summary_sql)) = trend_queries(metric) else {
        bail!(
            "unknown metric: {}. Valid: {}",
            metric,
            TREND_METRICS.join(", ")
        );
    };
    Ok(Series::Column(points_sql, summary_sql))
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

/// The number of runs, the smallest, the largest and the mean of `values`.
/// `None` when there is no value.
fn summary(values: &[f64]) -> Option<(usize, f64, f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let (min, max, sum) = values.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY, 0.0_f64),
        |(min, max, sum), v| (min.min(*v), max.max(*v), sum + v),
    );
    // A count of runs is far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let avg = sum / values.len() as f64;
    Some((values.len(), min, max, avg))
}

/// The values a recovery trend measured: its points without a bound.
fn measured_values(trend: &RecoveryTrend) -> Vec<f64> {
    trend
        .points
        .iter()
        .filter(|point| point.bound.is_none())
        .map(|point| point.value)
        .collect()
}

/// The lines that count the runs a recovery trend of `metric` left out.
fn left_out_lines(metric: RecoveryMetric, trend: &RecoveryTrend) -> Vec<String> {
    let objective = if metric.is_rto() { "RTO" } else { "RPO" };
    let mut lines = Vec::new();
    if trend.no_outage > 0 {
        lines.push(format!("{} run(s) saw no outage", trend.no_outage));
    }
    if trend.not_measured > 0 {
        lines.push(format!(
            "{} run(s) measured no {objective}",
            trend.not_measured
        ));
    }
    lines
}

/// Print the trend of a column of each run: a table of its points, oldest
/// first, and their summary.
fn print_column_trend(
    store: &tumult_lake::AnalyticsStore,
    metric: &str,
    points_sql: &str,
    summary_sql: &str,
) -> Result<()> {
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

/// Print a recovery trend: a table of its points, oldest first, the
/// summary of the values they measured, and the runs it left out.
fn print_recovery_trend(metric: RecoveryMetric, trend: &RecoveryTrend) {
    print_recovery_points(metric, trend);
    for line in left_out_lines(metric, trend) {
        crate::outln!("{line}");
    }
}

/// Print the points of a recovery trend and the summary of the values they
/// measured. A run that did not recover shows its bound, says so, and is
/// left out of the summary.
fn print_recovery_points(metric: RecoveryMetric, trend: &RecoveryTrend) {
    let name = metric.name();
    if trend.points.is_empty() {
        crate::outln!("No data points for metric: {name}");
        return;
    }
    crate::outln!("Trend: {name} ({} data points)\n", trend.points.len());
    let columns = [
        "experiment_id",
        "title",
        "status",
        name,
        "started_at_ns",
        "note",
    ];
    crate::outln!("{}", table_line(&columns.map(String::from)));
    crate::outln!("{}", "-".repeat(columns.len() * 20));
    for point in &trend.points {
        let (value, note) = match point.bound {
            Some(bound) => (format!("{bound} {:.4}", point.value), "not recovered"),
            None => (format!("{:.4}", point.value), ""),
        };
        let cells = [
            point.experiment_id.clone(),
            point.title.clone(),
            point.status.clone(),
            value,
            point.started_at_ns.to_string(),
            note.to_string(),
        ];
        crate::outln!("{}", table_line(&cells));
    }
    crate::outln!();
    let measured = measured_values(trend);
    if let Some((runs, min, max, avg)) = summary(&measured) {
        crate::outln!("Summary: {runs} runs, min={min:.4}, max={max:.4}, avg={avg:.4}");
    }
    let unrecovered = trend.points.len() - measured.len();
    if unrecovered > 0 {
        crate::outln!(
            "Not recovered: {unrecovered} run(s), each shown as a bound and left out of the \
             summary"
        );
    }
}

/// A point of a trend as JSON: its run and its value.
fn json_point(
    experiment_id: &str,
    title: &str,
    status: &str,
    started_at_ns: i64,
    value: f64,
) -> serde_json::Map<String, serde_json::Value> {
    let mut point = serde_json::Map::new();
    point.insert("experiment_id".into(), experiment_id.into());
    point.insert("title".into(), title.into());
    point.insert("status".into(), status.into());
    point.insert("started_at_ns".into(), started_at_ns.into());
    point.insert("value".into(), value.into());
    point
}

/// The summary of `values` as JSON: `null` when there is no value.
fn json_summary(values: &[f64]) -> serde_json::Value {
    summary(values).map_or(serde_json::Value::Null, |(runs, min, max, avg)| {
        serde_json::json!({ "runs": runs, "min": min, "max": max, "avg": avg })
    })
}

/// The points and the summary of a trend of a column of each run, as
/// JSON: the rows of its table.
fn json_column_trend(
    store: &tumult_lake::AnalyticsStore,
    metric: &str,
    points_sql: &str,
    trend: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let mut points = Vec::new();
    let mut values = Vec::new();
    for row in store.query(points_sql)? {
        let value: f64 = row[3]
            .parse()
            .with_context(|| format!("the {metric} of {} is no number: {}", row[0], row[3]))?;
        let started_at_ns: i64 = row[4]
            .parse()
            .with_context(|| format!("the start of {} is no number: {}", row[0], row[4]))?;
        points.push(json_point(&row[0], &row[1], &row[2], started_at_ns, value).into());
        values.push(value);
    }
    trend.insert("points".into(), serde_json::Value::Array(points));
    trend.insert("summary".into(), json_summary(&values));
    Ok(())
}

/// The points, the summary and the runs left out of a recovery trend, as
/// JSON. A point says whether its run recovered (the RTO only), and the
/// bound of a run that did not.
fn json_recovery_trend(
    recovery: &RecoveryTrend,
    trend: &mut serde_json::Map<String, serde_json::Value>,
) {
    let points = recovery.points.iter().map(|p| {
        let mut point = json_point(
            &p.experiment_id,
            &p.title,
            &p.status,
            p.started_at_ns,
            p.value,
        );
        if let Some(recovered) = p.recovered {
            point.insert("recovered".into(), recovered.into());
        }
        if let Some(bound) = p.bound {
            point.insert("bound".into(), bound.into());
        }
        serde_json::Value::Object(point)
    });
    trend.insert("points".into(), points.collect());
    trend.insert("summary".into(), json_summary(&measured_values(recovery)));
    trend.insert("runs_without_outage".into(), recovery.no_outage.into());
    trend.insert("runs_not_measured".into(), recovery.not_measured.into());
}

/// Print the trend of `series` over the runs in `scope` as JSON: what it
/// is of, how many journals it read, its points with their flags, their
/// summary and, for a recovery trend, the runs it left out.
fn print_json_trend(
    store: &tumult_lake::AnalyticsStore,
    series: &Series,
    metric: &str,
    journals: usize,
    scope: &tumult_core::journal::JournalScope,
    target: Option<&str>,
) -> Result<()> {
    let mut trend = serde_json::Map::new();
    trend.insert("metric".into(), metric.into());
    trend.insert("target".into(), target.into());
    trend.insert("journals".into(), journals.into());
    trend.insert(
        "journals_without_target".into(),
        scope.without_target().into(),
    );
    match series {
        Series::Column(points_sql, _) => {
            json_column_trend(store, metric, points_sql, &mut trend)?;
        }
        Series::Recovery(recovery) => {
            json_recovery_trend(&store.recovery_trend(*recovery)?, &mut trend);
        }
    }
    crate::outln!("{}", serde_json::to_string_pretty(&trend)?);
    Ok(())
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
    json: bool,
) -> Result<()> {
    use tumult_core::journal::JournalScope;
    use tumult_lake::AnalyticsStore;

    let series = series(metric)?;
    let mut scope = JournalScope::new(window_start_ns(last)?, target);
    let store = AnalyticsStore::in_memory()?;
    let count = load_in_scope(journals_path, &mut scope, &store)?;

    if json {
        return print_json_trend(&store, &series, metric, count, &scope, target);
    }

    crate::outln!("Loaded {count} journal(s)");
    if let Some(note) = scope.left_out_note() {
        crate::outln!("{note}");
    }
    crate::outln!();

    match series {
        Series::Column(points_sql, summary_sql) => {
            print_column_trend(&store, metric, points_sql, summary_sql)
        }
        Series::Recovery(recovery) => {
            print_recovery_trend(recovery, &store.recovery_trend(recovery)?);
            Ok(())
        }
    }
}
