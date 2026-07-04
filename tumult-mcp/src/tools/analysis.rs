//! Analytics-store query tools: ad-hoc journal analysis and persistent-store stats.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::validation::validate_select_only;
use crate::tools::StructuredReport;

/// Analyze journals with a SQL query via `DuckDB`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the query is not a SELECT/WITH, the store cannot
/// be created, a journal cannot be read or ingested, or the query fails.
pub fn analyze(journals_path: &str, query: &str) -> Result<String, ToolError> {
    use tumult_core::journal::read_journal;

    validate_select_only(query)?;

    let store = tumult_analytics::AnalyticsStore::in_memory()
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let path = Path::new(journals_path);
    if path.is_file() {
        let journal = read_journal(path).map_err(|e| ToolError::Parse(e.to_string()))?;
        store
            .ingest_journal(&journal)
            .map_err(|e| ToolError::Store(e.to_string()))?;
    } else if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("toon") {
                if let Ok(journal) = read_journal(&entry.path()) {
                    let _ = store.ingest_journal(&journal);
                }
            }
        }
    }

    let columns = store
        .query_columns(query)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let rows = store
        .query(query)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut output = columns.join("\t") + "\n";
    for row in &rows {
        output += &row.join("\t");
        output += "\n";
    }
    let _ = write!(output, "{} row(s)", rows.len());
    Ok(output)
}

/// Query the persistent analytics store stats.
/// If `store_path` is empty, uses the default path.
///
/// The structured object contains `store`, `schema_version`, `experiments`,
/// `activities`, and (when file metadata is readable) `size_mb`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist, cannot be opened, or
/// the stats/schema-version query fails.
pub fn store_stats(store_path: &str) -> Result<StructuredReport, ToolError> {
    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }

    let store = tumult_analytics::AnalyticsStore::open(&path)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let stats = store.stats().map_err(|e| ToolError::Store(e.to_string()))?;
    let version = store
        .schema_version()
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut structured = serde_json::Map::new();
    structured.insert("store".into(), serde_json::json!(store_path));
    structured.insert("schema_version".into(), serde_json::json!(version));
    structured.insert(
        "experiments".into(),
        serde_json::json!(stats.experiment_count),
    );
    structured.insert("activities".into(), serde_json::json!(stats.activity_count));

    let mut output = format!("store: {store_path}\n");
    let _ = writeln!(output, "schema_version: {version}");
    let _ = writeln!(output, "experiments: {}", stats.experiment_count);
    let _ = writeln!(output, "activities: {}", stats.activity_count);

    if let Ok(meta) = std::fs::metadata(&path) {
        // u64 → f64: file sizes in megabytes; precision loss is acceptable for display.
        #[allow(clippy::cast_precision_loss)]
        let mb = meta.len() as f64 / (1024.0 * 1024.0);
        let _ = writeln!(output, "size_mb: {mb:.2}");
        structured.insert("size_mb".into(), serde_json::json!(mb));
    }

    Ok(StructuredReport {
        text: output,
        structured,
    })
}

/// Analyze using the persistent store directly (no journal loading).
///
/// # Errors
///
/// Returns a [`ToolError`] if the query is not a SELECT/WITH, the store cannot
/// be opened, or the query fails.
pub fn analyze_persistent(store_path: &str, query: &str) -> Result<String, ToolError> {
    validate_select_only(query)?;

    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }

    let store = tumult_analytics::AnalyticsStore::open(&path)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let columns = store
        .query_columns(query)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let rows = store
        .query(query)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut output = columns.join("\t") + "\n";
    for row in &rows {
        output += &row.join("\t");
        output += "\n";
    }
    let _ = write!(output, "{} row(s)", rows.len());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::write_run_journal;
    use tempfile::TempDir;

    // ── analyze ───────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_returns_query_results() {
        let dir = TempDir::new().unwrap();

        // First run the experiment to get a journal
        let journal_path = write_run_journal(dir.path());

        let result = analyze(
            journal_path.to_str().unwrap(),
            "SELECT experiment_id, status FROM experiments",
        );
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("1 row(s)"));
    }

    #[test]
    fn analyze_rejects_non_select_query() {
        let dir = TempDir::new().unwrap();
        let result = analyze(dir.path().to_str().unwrap(), "DROP TABLE experiments");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("only SELECT/WITH"));
    }

    // ── store_stats ──────────────────────────────────────────

    #[test]
    fn store_stats_with_temp_store() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");
        let store = tumult_analytics::AnalyticsStore::open(&db_path).unwrap();
        drop(store);

        let result = store_stats(db_path.to_str().unwrap());
        assert!(result.is_ok());
        let report = result.unwrap();
        assert!(report.text.contains("experiments: 0"));
        assert!(report.text.contains("schema_version: 3"));
        assert_eq!(report.structured["experiments"], 0);
        assert_eq!(report.structured["schema_version"], 3);
        assert!(report.structured.contains_key("store"));
        assert!(report.structured.contains_key("activities"));
    }

    #[test]
    fn store_stats_missing_store_returns_error() {
        let result = store_stats("/nonexistent/analytics.duckdb");
        assert!(result.is_err());
    }

    // ── analyze_persistent ───────────────────────────────────

    #[test]
    fn analyze_persistent_rejects_non_select_query() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");
        let store = tumult_analytics::AnalyticsStore::open(&db_path).unwrap();
        drop(store);

        let result = analyze_persistent(db_path.to_str().unwrap(), "DROP TABLE experiments");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("only SELECT/WITH"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_persistent_queries_store() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");

        // Pre-populate a persistent store
        {
            let store = tumult_analytics::AnalyticsStore::open(&db_path).unwrap();
            let journal_file = write_run_journal(dir.path());
            let journal = tumult_core::journal::read_journal(&journal_file).unwrap();
            store.ingest_journal(&journal).unwrap();
        }

        let result = analyze_persistent(
            db_path.to_str().unwrap(),
            "SELECT count(*) as n FROM experiments",
        );
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("1 row(s)"));
    }
}
