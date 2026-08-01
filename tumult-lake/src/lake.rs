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
#[doc(hidden)]
pub const AUDIT_TABLE: &str = "manual_experiment_audit";
const AUDIT_TS_COL: &str = "changed_at_ns";

/// The evidence register: full snapshot per run (records mutate through the
/// draft → submitted → verified lifecycle), never deleted.
#[doc(hidden)]
pub const MANUAL_TABLE: &str = "manual_experiments";

/// Journal-detail tables: incremental on `started_at_ns` and
/// retention-eligible under the same watermark guard as telemetry.
const JOURNAL_TABLES: [&str; 3] = ["experiments", "activity_results", "load_results"];
const JOURNAL_TS_COL: &str = "started_at_ns";

/// INSERT-ONLY autopilot history: snapshot-exported (rows are few and the
/// tables are event-sourced), retention-eligible ONLY while the current
/// fingerprint matches the last exported one — fingerprint equality proves
/// every hot row is already in the lake.
const AUTOPILOT_SNAPSHOT_TABLES: [(&str, &str); 3] = [
    ("autopilot_decisions", "decided_at_ns"),
    ("autopilot_events", "at_ns"),
    ("autopilot_change_events", "at_ns"),
];

/// Mutable or timestamp-less tables: snapshot-exported, never deleted.
/// `graph_edges` rows are rewritten with `ts = 0` by topology refreshes, so
/// a watermark would lose updates and retention would delete fresh rows;
/// the `agentic_*` tables have no timestamp column at all.
const SNAPSHOT_ONLY_TABLES: [&str; 6] = [
    "graph_nodes",
    "graph_edges",
    "agentic_runs",
    "agentic_contract_outcomes",
    "agentic_fault_applications",
    "agentic_replay_outcomes",
];

/// Content fingerprint for a snapshot table: md5 over the ordered per-row
/// hashes of the full row JSON. Manual evidence uses its cheaper, stable
/// `content_hash` column instead.
#[doc(hidden)]
pub fn fingerprint_sql(table: &str) -> String {
    if table == MANUAL_TABLE {
        format!(
            "SELECT md5(COALESCE(string_agg(content_hash, ',' ORDER BY id), '')) AS fp \
             FROM {table}"
        )
    } else {
        format!(
            "SELECT md5(COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp \
             FROM (SELECT md5(CAST(row_to_json(t) AS VARCHAR)) AS h FROM {table} t)"
        )
    }
}

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
    /// Legacy field: the pre-generalization fingerprint for
    /// `manual_experiments`. Seeded into `fingerprints` on read and never
    /// written again.
    #[serde(default, skip_serializing)]
    manual_fingerprint: Option<String>,
    /// Per-table content fingerprint for snapshot-exported tables; a
    /// snapshot is rewritten only when its fingerprint changes.
    #[serde(default)]
    fingerprints: BTreeMap<String, String>,
}

#[doc(hidden)]
pub fn meta_path(dir: &Path) -> PathBuf {
    dir.join("_meta.json")
}

fn read_meta(dir: &Path) -> Result<LakeMeta, StoreError> {
    let mut meta = match std::fs::read_to_string(meta_path(dir)) {
        Ok(raw) => serde_json::from_str::<LakeMeta>(&raw)
            .map_err(|e| StoreError::Internal(format!("corrupt lake watermark file: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LakeMeta::default(),
        Err(e) => return Err(StoreError::from(e)),
    };
    // Migrate the legacy manual-only fingerprint field into the generic map.
    if let Some(fp) = meta.manual_fingerprint.take() {
        meta.fingerprints
            .entry(MANUAL_TABLE.to_string())
            .or_insert(fp);
    }
    Ok(meta)
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

#[doc(hidden)]
pub fn now_ns() -> i64 {
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

/// Full-snapshot export for a mutable or event-sourced table: latest
/// snapshot wins; consumers take the newest file. Rewritten only when the
/// table's content fingerprint changes, so idempotent re-runs write nothing
/// here either.
fn export_snapshot(
    reader: &Reader,
    cfg: &LakeConfig,
    table: &str,
    run_ns: i64,
    prior_fingerprint: Option<&str>,
) -> Result<(TableExport, String), StoreError> {
    let fp = reader
        .query_json_rows(&fingerprint_sql(table))?
        .first()
        .and_then(|r| r.get("fp"))
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let unchanged = prior_fingerprint == Some(fp.as_str());
    let count = reader.query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))?;
    let n = count
        .first()
        .and_then(|r| r.get("n"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let mut files = Vec::new();
    let mut rows = 0;
    if n > 0 && !unchanged {
        let day = reader
            .query_json_rows(&format!(
                "SELECT strftime(to_timestamp({run_ns} / 1000000000.0), '%Y-%m-%d') AS d"
            ))?
            .first()
            .and_then(|r| r.get("d"))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        let rel = format!("{table}/date={day}/data-{run_ns}.parquet");
        let path = cfg.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        reader.execute_batch(&format!(
            "COPY (SELECT * FROM {table}) TO '{}' (FORMAT PARQUET)",
            path.display()
        ))?;
        files.push(rel);
        rows = n;
    }
    Ok((
        TableExport {
            name: table.to_string(),
            rows,
            watermark_ns: run_ns,
            files,
        },
        fp,
    ))
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

    for table in JOURNAL_TABLES {
        let wm = meta.tables.get(table).copied().unwrap_or(0);
        let t = export_incremental(reader, cfg, table, JOURNAL_TS_COL, wm, run_ns)?;
        meta.tables.insert(table.to_string(), t.watermark_ns);
        tables.push(t);
    }

    for table in std::iter::once(&MANUAL_TABLE)
        .chain(AUTOPILOT_SNAPSHOT_TABLES.iter().map(|(t, _)| t))
        .chain(SNAPSHOT_ONLY_TABLES.iter())
    {
        let (t, fp) = export_snapshot(
            reader,
            cfg,
            table,
            run_ns,
            meta.fingerprints.get(*table).map(String::as_str),
        )?;
        meta.tables.insert((*table).to_string(), t.watermark_ns);
        meta.fingerprints.insert((*table).to_string(), fp);
        tables.push(t);
    }

    meta.last_export_ns = Some(run_ns);
    write_meta(&cfg.dir, &meta)?;
    Ok(ExportReport {
        ran_at_ns: run_ns,
        lake_dir: cfg.dir.display().to_string(),
        retention_days: cfg.retention_days,
        tables,
    })
}

/// Delete hot-store rows older than `retention_days` that the lake provably
/// holds. Telemetry and journal-detail tables use the watermark guard (rows
/// above the watermark survive); autopilot snapshot tables are purged only
/// while the current fingerprint matches the last exported one (equality
/// proves every row is in the lake). Audit, manual-evidence, graph and
/// agentic tables are never touched (append-only compliance evidence, or
/// mutable/timestamp-less rows a watermark cannot protect). No-op when
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
    for table in JOURNAL_TABLES {
        let wm = meta.tables.get(table).copied().unwrap_or(0);
        let n = writer.execute(
            &format!(
                "DELETE FROM {table} WHERE {JOURNAL_TS_COL} < {cutoff} AND {JOURNAL_TS_COL} <= {wm}"
            ),
            [],
        )?;
        deleted.insert(table.to_string(), n as u64);
    }
    for (table, ts_col) in AUTOPILOT_SNAPSHOT_TABLES {
        let Some(exported_fp) = meta.fingerprints.get(table) else {
            // Never exported: nothing in the lake, nothing may be deleted.
            continue;
        };
        let current_fp = writer
            .query_json_rows(&fingerprint_sql(table))?
            .first()
            .and_then(|r| r.get("fp"))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if &current_fp != exported_fp {
            // Rows exist that are not in the lake yet: keep everything.
            continue;
        }
        let n = writer.execute(
            &format!("DELETE FROM {table} WHERE {ts_col} < {cutoff}"),
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
