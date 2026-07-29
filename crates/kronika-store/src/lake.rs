//! Parquet lake export + retention — clean-room, DuckDB-native.
//!
//! The hot DuckDB store is the fast recent-query tier; the lake is the
//! immutable long-term tier: per table, `COPY (SELECT …) TO
//! '<lake>/<table>/date=<d>/data-<run>.parquet' (FORMAT PARQUET)` writes one
//! file per day-partition directory. Exports are **incremental** against a
//! persisted watermark (`<lake>/_meta.json`), so re-runs are idempotent: no
//! new rows, no new files.
//!
//! Retention reclaims the hot store only for rows the watermark proves were
//! already exported (`ts_ns <= watermark`), and only when
//! `KRONIKA_RETENTION_DAYS > 0`. Two tables are exempt from deletion:
//! `manual_experiment_audit` (append-only compliance evidence) and
//! `manual_experiments` (the evidence register itself — exported as a full
//! snapshot per run instead of incrementally, since records mutate through
//! their review lifecycle).
//!
//! Two operational caveats, both inherited from event-time watermarking:
//!
//! * **Snapshot readers.** A read-only `DuckDB` connection pins its snapshot
//!   at open; open a *fresh* [`Reader`] per export run or it will not see
//!   rows committed since it was opened.
//! * **Backdated late arrivals.** The watermark is event time (`ts_ns`).
//!   Rows arriving with an event time at or below the current watermark are
//!   invisible to incremental export (tumult/smedja telemetry arrives in
//!   real time, so this only matters for hand-backfilled data — do full
//!   re-exports for those). Retention never deletes rows above the
//!   watermark, so unexported late rows at least cannot be reclaimed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Reader, StoreError, Writer};

/// Telemetry tables: `ts_ns` watermark column, incremental export, eligible
/// for retention deletes once exported.
const TELEMETRY_TABLES: [&str; 5] = [
    "spans",
    "logs",
    "metric_sums",
    "metric_gauges",
    "metric_histograms",
];

/// Append-only audit: incremental export on `changed_at_ns`, never deleted.
const AUDIT_TABLE: &str = "manual_experiment_audit";
const AUDIT_TS_COL: &str = "changed_at_ns";

/// The evidence register: full snapshot per run (records mutate through the
/// draft → submitted → verified lifecycle), never deleted.
const MANUAL_TABLE: &str = "manual_experiments";

/// Day-partition expression for a watermark column (UTC, `YYYY-MM-DD`).
fn day_expr(ts_col: &str) -> String {
    format!("strftime(to_timestamp({ts_col} / 1000000000.0), '%Y-%m-%d')")
}

/// Lake layout + retention policy.
#[derive(Clone, Debug)]
pub struct LakeConfig {
    /// Root of the parquet lake (created on first export).
    pub dir: PathBuf,
    /// Hot-store retention in days; `0` keeps rows forever (default).
    pub retention_days: u64,
}

impl LakeConfig {
    #[must_use]
    pub fn new(dir: PathBuf, retention_days: u64) -> Self {
        Self {
            dir,
            retention_days,
        }
    }

    /// From `KRONIKA_LAKE_DIR` (default `<db dir>/lake`) and
    /// `KRONIKA_RETENTION_DAYS` (default `0` = keep forever).
    #[must_use]
    pub fn from_env(db_path: &Path) -> Self {
        let dir = std::env::var_os("KRONIKA_LAKE_DIR").map_or_else(
            || {
                db_path
                    .parent()
                    .map_or_else(|| PathBuf::from("lake"), |d| d.join("lake"))
            },
            PathBuf::from,
        );
        let retention_days = std::env::var("KRONIKA_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        Self::new(dir, retention_days)
    }
}

/// One table's slice of an export run.
#[derive(Clone, Debug, Serialize)]
pub struct TableExport {
    pub name: String,
    /// Rows written this run (0 when the watermark was already current).
    pub rows: u64,
    /// Watermark after this run (rows up to and including this are exported).
    pub watermark_ns: i64,
    /// Parquet files written this run, relative to the lake dir.
    pub files: Vec<String>,
}

/// What one export run did.
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub ran_at_ns: i64,
    pub lake_dir: String,
    pub retention_days: u64,
    pub tables: Vec<TableExport>,
}

/// Point-in-time lake summary for `GET /api/lake/status`.
#[derive(Clone, Debug, Serialize)]
pub struct LakeStatus {
    pub lake_dir: String,
    pub retention_days: u64,
    pub last_export_ns: Option<i64>,
    /// Per-table export watermark (rows up to and including are in the lake).
    pub watermarks: BTreeMap<String, i64>,
    pub files: u64,
    pub bytes: u64,
}

/// Persisted watermark state (`<lake>/_meta.json`).
#[derive(Default, Serialize, Deserialize)]
struct LakeMeta {
    last_export_ns: Option<i64>,
    #[serde(default)]
    tables: BTreeMap<String, i64>,
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("_meta.json")
}

fn read_meta(dir: &Path) -> Result<LakeMeta, StoreError> {
    match std::fs::read_to_string(meta_path(dir)) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| StoreError::Internal(format!("corrupt lake watermark file: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LakeMeta::default()),
        Err(e) => Err(StoreError::from(e)),
    }
}

/// Write the watermark file atomically (tmp + rename) so a crash mid-export
/// never leaves a torn watermark claiming rows that were not exported.
fn write_meta(dir: &Path, meta: &LakeMeta) -> Result<(), StoreError> {
    let raw = serde_json::to_string_pretty(meta)
        .map_err(|e| StoreError::Internal(format!("serialize lake meta: {e}")))?;
    let tmp = dir.join("_meta.json.tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, meta_path(dir))?;
    Ok(())
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// Export one watermark-column table incrementally: one parquet file per
/// day-partition with rows newer than the stored watermark. Returns the
/// table report; advances nothing on its own (the caller persists meta).
fn export_incremental(
    reader: &Reader,
    cfg: &LakeConfig,
    table: &str,
    ts_col: &str,
    watermark: i64,
    run_ns: i64,
) -> Result<TableExport, StoreError> {
    let day = day_expr(ts_col);
    let days = reader.query_json_rows(&format!(
        "SELECT DISTINCT {day} AS d FROM {table} WHERE {ts_col} > {watermark} ORDER BY d"
    ))?;
    let mut files = Vec::new();
    let mut rows: u64 = 0;
    for d in days {
        let Some(day_str) = d.get("d").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let rel = format!("{table}/date={day_str}/data-{run_ns}.parquet");
        let path = cfg.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let count = reader.query_json_rows(&format!(
            "SELECT count(*) AS n FROM {table} \
             WHERE {ts_col} > {watermark} AND {day} = '{day_str}'"
        ))?;
        let n = count
            .first()
            .and_then(|r| r.get("n"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if n == 0 {
            continue;
        }
        reader.execute_batch(&format!(
            "COPY (SELECT * FROM {table} \
             WHERE {ts_col} > {watermark} AND {day} = '{day_str}') \
             TO '{}' (FORMAT PARQUET)",
            path.display()
        ))?;
        rows += n;
        files.push(rel);
    }
    let new_watermark = if rows == 0 {
        watermark
    } else {
        reader
            .query_json_rows(&format!("SELECT max({ts_col}) AS m FROM {table}"))?
            .first()
            .and_then(|r| r.get("m"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(watermark)
            .max(watermark)
    };
    Ok(TableExport {
        name: table.to_string(),
        rows,
        watermark_ns: new_watermark,
        files,
    })
}

/// Full-snapshot export for the mutable evidence register: latest snapshot
/// wins; consumers take the newest file.
fn export_snapshot(
    reader: &Reader,
    cfg: &LakeConfig,
    run_ns: i64,
) -> Result<TableExport, StoreError> {
    let day = reader
        .query_json_rows(&format!(
            "SELECT strftime(to_timestamp({run_ns} / 1000000000.0), '%Y-%m-%d') AS d"
        ))?
        .first()
        .and_then(|r| r.get("d"))
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let count = reader.query_json_rows(&format!("SELECT count(*) AS n FROM {MANUAL_TABLE}"))?;
    let n = count
        .first()
        .and_then(|r| r.get("n"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let mut files = Vec::new();
    if n > 0 {
        let rel = format!("{MANUAL_TABLE}/date={day}/data-{run_ns}.parquet");
        let path = cfg.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        reader.execute_batch(&format!(
            "COPY (SELECT * FROM {MANUAL_TABLE}) TO '{}' (FORMAT PARQUET)",
            path.display()
        ))?;
        files.push(rel);
    }
    Ok(TableExport {
        name: MANUAL_TABLE.to_string(),
        rows: n,
        watermark_ns: run_ns,
        files,
    })
}

/// Run one export pass over all tables. The watermark file is advanced only
/// after every table exported successfully, so a failed run is retried from
/// the last good watermark (idempotent).
///
/// # Errors
/// Returns an error if any table's export or the watermark write fails.
pub fn export(reader: &Reader, cfg: &LakeConfig) -> Result<ExportReport, StoreError> {
    std::fs::create_dir_all(&cfg.dir)?;
    let run_ns = now_ns();
    let mut meta = read_meta(&cfg.dir)?;
    let mut tables = Vec::new();

    for table in TELEMETRY_TABLES {
        let wm = meta.tables.get(table).copied().unwrap_or(0);
        let t = export_incremental(reader, cfg, table, "ts_ns", wm, run_ns)?;
        meta.tables.insert(table.to_string(), t.watermark_ns);
        tables.push(t);
    }
    let audit_wm = meta.tables.get(AUDIT_TABLE).copied().unwrap_or(0);
    let audit = export_incremental(reader, cfg, AUDIT_TABLE, AUDIT_TS_COL, audit_wm, run_ns)?;
    meta.tables
        .insert(AUDIT_TABLE.to_string(), audit.watermark_ns);
    tables.push(audit);

    let manual = export_snapshot(reader, cfg, run_ns)?;
    meta.tables
        .insert(MANUAL_TABLE.to_string(), manual.watermark_ns);
    tables.push(manual);

    meta.last_export_ns = Some(run_ns);
    write_meta(&cfg.dir, &meta)?;
    Ok(ExportReport {
        ran_at_ns: run_ns,
        lake_dir: cfg.dir.display().to_string(),
        retention_days: cfg.retention_days,
        tables,
    })
}

/// Delete hot-store telemetry rows older than `retention_days` that the
/// watermark proves were already exported. Audit and manual-evidence tables
/// are never touched (append-only compliance evidence). No-op when
/// `retention_days == 0`.
///
/// # Errors
/// Returns an error if the watermark file cannot be read or a delete fails.
pub fn enforce_retention(
    writer: &Writer,
    cfg: &LakeConfig,
) -> Result<BTreeMap<String, u64>, StoreError> {
    let mut deleted = BTreeMap::new();
    if cfg.retention_days == 0 {
        return Ok(deleted);
    }
    let meta = read_meta(&cfg.dir)?;
    let cutoff = now_ns() - (cfg.retention_days as i64) * 86_400 * 1_000_000_000;
    for table in TELEMETRY_TABLES {
        let wm = meta.tables.get(table).copied().unwrap_or(0);
        let n = writer.execute(
            &format!("DELETE FROM {table} WHERE ts_ns < {cutoff} AND ts_ns <= {wm}"),
            [],
        )?;
        deleted.insert(table.to_string(), n as u64);
    }
    Ok(deleted)
}

/// Lake summary for `GET /api/lake/status`: watermarks from the meta file
/// plus a recursive file/byte count over `*.parquet`.
///
/// # Errors
/// Returns an error if the meta file is corrupt or the dir cannot be read.
pub fn status(cfg: &LakeConfig) -> Result<LakeStatus, StoreError> {
    let meta = read_meta(&cfg.dir)?;
    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    let mut stack = vec![cfg.dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // lake not created yet
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "parquet") {
                files += 1;
                bytes += entry.metadata().map_or(0, |m| m.len());
            }
        }
    }
    Ok(LakeStatus {
        lake_dir: cfg.dir.display().to_string(),
        retention_days: cfg.retention_days,
        last_export_ns: meta.last_export_ns,
        watermarks: meta.tables,
        files,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LogRow, MetricSumRow, SpanRow, Store};

    const DAY_NS: i64 = 86_400 * 1_000_000_000;
    // Fixed base so test rows land on deterministic dates.
    const BASE_NS: i64 = 1_785_225_600_000_000_000; // 2026-07-23T00:00:00Z

    fn span(ts_ns: i64, name: &str) -> SpanRow {
        SpanRow {
            ts_ns,
            trace_id: format!("trace-{ts_ns}"),
            span_id: format!("span-{ts_ns}"),
            span_name: name.into(),
            duration_ns: 1_000_000,
            service_name: "tumult".into(),
            ..SpanRow::default()
        }
    }

    fn fixture() -> (tempfile::TempDir, Store, LakeConfig) {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let cfg = LakeConfig::new(d.path().join("lake"), 0);
        (d, store, cfg)
    }

    fn parquet_count(reader: &Reader, cfg: &LakeConfig, table: &str) -> i64 {
        let glob = cfg.dir.join(format!("{table}/date=*/*.parquet"));
        reader
            .query_json_rows(&format!(
                "SELECT count(*) AS n FROM read_parquet('{}')",
                glob.display()
            ))
            .unwrap()
            .first()
            .and_then(|r| r.get("n"))
            .and_then(serde_json::Value::as_i64)
            .unwrap()
    }

    #[test]
    fn export_creates_valid_parquet_with_matching_row_counts() {
        let (_d, store, cfg) = fixture();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                span(BASE_NS, "resilience.experiment"),
                span(BASE_NS + 1, "resilience.experiment"),
                span(BASE_NS + DAY_NS, "resilience.action"),
            ])
            .unwrap();
        writer
            .insert_logs(&[LogRow {
                ts_ns: BASE_NS,
                severity_text: "INFO".into(),
                body: "hello".into(),
                ..LogRow::default()
            }])
            .unwrap();
        writer
            .insert_metric_sums(&[MetricSumRow {
                ts_ns: BASE_NS,
                metric_name: "tumult.runs".into(),
                value: 1.0,
                ..MetricSumRow::default()
            }])
            .unwrap();

        let reader = store.read_only().unwrap();
        let report = export(&reader, &cfg).unwrap();

        let spans = report.tables.iter().find(|t| t.name == "spans").unwrap();
        assert_eq!(spans.rows, 3);
        assert_eq!(spans.files.len(), 2, "two day partitions"); // d0 and d1
        assert_eq!(spans.watermark_ns, BASE_NS + DAY_NS);
        // Files exist on disk and read back with the full row count.
        for rel in &spans.files {
            assert!(cfg.dir.join(rel).exists(), "{rel} missing");
        }
        assert_eq!(parquet_count(&reader, &cfg, "spans"), 3);
        assert_eq!(parquet_count(&reader, &cfg, "logs"), 1);
        assert_eq!(parquet_count(&reader, &cfg, "metric_sums"), 1);
    }

    #[test]
    fn export_is_incremental_and_idempotent() {
        let (_d, store, cfg) = fixture();
        let writer = store.writer().unwrap();
        writer.insert_spans(&[span(BASE_NS, "a")]).unwrap();
        let reader = store.read_only().unwrap();

        let first = export(&reader, &cfg).unwrap();
        assert_eq!(first.tables[0].rows, 1);

        // Re-run with no new rows: nothing written, watermark unchanged.
        let second = export(&reader, &cfg).unwrap();
        assert!(second
            .tables
            .iter()
            .all(|t| t.rows == 0 && t.files.is_empty()));
        let files_after_noop = status(&cfg).unwrap().files;

        // New row: exactly one new file, watermark advances, lake total grows.
        // (A read-only connection pins its snapshot at open; a fresh reader
        // per unit of work sees later commits — the scheduler opens one per
        // run for exactly this reason.)
        writer
            .insert_spans(&[span(BASE_NS + 2 * DAY_NS, "b")])
            .unwrap();
        let reader2 = store.read_only().unwrap();
        let third = export(&reader2, &cfg).unwrap();
        let spans = third.tables.iter().find(|t| t.name == "spans").unwrap();
        assert_eq!(spans.rows, 1);
        assert_eq!(spans.watermark_ns, BASE_NS + 2 * DAY_NS);
        assert_eq!(status(&cfg).unwrap().files, files_after_noop + 1);
        assert_eq!(parquet_count(&reader2, &cfg, "spans"), 2);
    }

    #[test]
    fn retention_deletes_only_exported_old_rows() {
        let (_d, store, mut cfg) = fixture();
        cfg.retention_days = 1;
        let writer = store.writer().unwrap();
        // "Old" relative to the test's own clock: 3 days before now.
        let now = now_ns();
        let old = now - 3 * DAY_NS;
        let fresh = now - 1_000_000_000;
        writer
            .insert_spans(&[span(old, "old"), span(fresh, "fresh")])
            .unwrap();

        let reader = store.read_only().unwrap();
        export(&reader, &cfg).unwrap();
        // Lands after the export: above the watermark, so NOT yet exported —
        // the watermark check must protect it even if it were old enough.
        writer.insert_spans(&[span(now, "late")]).unwrap();

        let deleted = enforce_retention(&writer, &cfg).unwrap();
        assert_eq!(deleted.get("spans"), Some(&1));
        // Fresh reader: the one above pinned its snapshot before the delete.
        let reader2 = store.read_only().unwrap();
        let remaining = reader2
            .query_json_rows("SELECT span_name FROM spans ORDER BY ts_ns")
            .unwrap();
        let names: Vec<&str> = remaining
            .iter()
            .filter_map(|r| r.get("span_name").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(names, ["fresh", "late"]);
    }

    #[test]
    fn audit_exports_but_is_never_deleted() {
        let (_d, store, mut cfg) = fixture();
        cfg.retention_days = 1;
        let writer = store.writer().unwrap();
        let old = now_ns() - 3 * DAY_NS;
        writer
            .execute(
                "INSERT INTO manual_experiment_audit VALUES \
                 ('a1', 'exp-1', 'alice', ?, 'create', NULL, NULL, 'hash1')",
                [old],
            )
            .unwrap();

        let reader = store.read_only().unwrap();
        let report = export(&reader, &cfg).unwrap();
        let audit = report
            .tables
            .iter()
            .find(|t| t.name == AUDIT_TABLE)
            .unwrap();
        assert_eq!(audit.rows, 1);
        assert_eq!(parquet_count(&reader, &cfg, AUDIT_TABLE), 1);

        let deleted = enforce_retention(&writer, &cfg).unwrap();
        assert!(!deleted.contains_key(AUDIT_TABLE));
        let n = reader
            .query_json_rows("SELECT count(*) AS n FROM manual_experiment_audit")
            .unwrap();
        assert_eq!(n[0]["n"], serde_json::json!(1));
    }
}
