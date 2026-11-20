//! Analytics-store query tools: ad-hoc journal analysis and persistent-store stats.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::reporting::journal_skipped;
use crate::tools::validation::validate_select_only;
use crate::tools::StructuredReport;

/// Loads one journal of a directory walk into `store`. A journal that cannot
/// be read or ingested is left out and reported, never fatal.
fn ingest_or_skip(store: &tumult_lake::AnalyticsStore, path: &Path) {
    let journal = match tumult_core::journal::read_journal(path) {
        Ok(journal) => journal,
        Err(error) => return journal_skipped(path, &error),
    };
    if let Err(error) = store.ingest_journal(&journal) {
        journal_skipped(path, &error);
    }
}

/// Run the caller's `query` on `store` and render the rows as tab-separated
/// text. External access is turned off first, so the query reads the tables
/// of the store and nothing else of the host.
fn answer(store: &tumult_lake::AnalyticsStore, query: &str) -> Result<String, ToolError> {
    store
        .deny_external_access()
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

/// Analyze journals with a SQL query via `DuckDB`.
///
/// # Errors
///
/// Returns a [`ToolError`] if the query is not a SELECT/WITH, the store cannot
/// be created, a journal cannot be read or ingested, or the query fails,
/// which includes any query that reads a host file.
pub fn analyze(journals_path: &str, query: &str) -> Result<String, ToolError> {
    use tumult_core::journal::read_journal;

    validate_select_only(query)?;

    let store =
        tumult_lake::AnalyticsStore::in_memory().map_err(|e| ToolError::Store(e.to_string()))?;

    let path = Path::new(journals_path);
    if path.is_file() {
        let journal = read_journal(path).map_err(|e| ToolError::Parse(e.to_string()))?;
        store
            .ingest_journal(&journal)
            .map_err(|e| ToolError::Store(e.to_string()))?;
    } else if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry_path = entry?.path();
            if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                ingest_or_skip(&store, &entry_path);
            }
        }
    }

    answer(&store, query)
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

    let store = tumult_lake::AnalyticsStore::open_read_only(&path)
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
/// be opened, or the query fails, which includes any query that reads a
/// host file.
pub fn analyze_persistent(store_path: &str, query: &str) -> Result<String, ToolError> {
    validate_select_only(query)?;

    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }

    let store = tumult_lake::AnalyticsStore::open_read_only(&path)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    answer(&store, query)
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
        let store = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
        // Read the schema version back from the store itself so this test
        // tracks migrations instead of hardcoding the current number.
        let schema_version = store.schema_version().unwrap();
        drop(store);

        let result = store_stats(db_path.to_str().unwrap());
        assert!(result.is_ok());
        let report = result.unwrap();
        assert!(report.text.contains("experiments: 0"));
        assert!(report
            .text
            .contains(&format!("schema_version: {schema_version}")));
        assert_eq!(report.structured["experiments"], 0);
        assert_eq!(report.structured["schema_version"], schema_version);
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
        let store = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
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
            let store = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
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

    // ── host files stay out of reach ─────────────────────────

    /// The value the host files hold. No answer may carry it.
    const HOST_SECRET: &str = "host-secret-7f3a";

    /// Writes a CSV and a JSON file that hold [`HOST_SECRET`] into `dir`,
    /// and returns queries that read them: by table function and by a
    /// quoted path in `FROM`. None of them names a function of the deny
    /// list, so only the connection itself can refuse them.
    fn host_file_queries(dir: &Path) -> Vec<String> {
        let csv = dir.join("secret.csv");
        std::fs::write(&csv, format!("value\n{HOST_SECRET}\n")).unwrap();
        let json = dir.join("secret.json");
        std::fs::write(&json, format!("{{\"value\": \"{HOST_SECRET}\"}}\n")).unwrap();
        vec![
            format!("SELECT * FROM read_csv_auto('{}')", csv.display()),
            format!("SELECT * FROM read_json_auto('{}')", json.display()),
            format!("SELECT * FROM '{}'", csv.display()),
        ]
    }

    /// `None` when the connection refused the query and the secret did not
    /// come back in the error either; otherwise what went wrong.
    fn leak(result: Result<String, ToolError>, sql: &str) -> Option<String> {
        match result {
            Ok(output) => Some(format!("{sql} was answered: {output}")),
            Err(error) => {
                let error = error.to_string();
                let refused = error.contains("Permission Error") && !error.contains(HOST_SECRET);
                (!refused).then(|| format!("{sql} failed otherwise: {error}"))
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_cannot_read_a_host_file_through_sql() {
        let run = TempDir::new().unwrap();
        let journal = write_run_journal(run.path());
        let host = TempDir::new().unwrap();

        let leaks: Vec<String> = host_file_queries(host.path())
            .iter()
            .filter_map(|sql| leak(analyze(journal.to_str().unwrap(), sql), sql))
            .collect();
        assert!(leaks.is_empty(), "{leaks:#?}");
    }

    #[test]
    fn analyze_store_cannot_read_a_host_file_through_sql() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");
        drop(tumult_lake::AnalyticsStore::open(&db_path).unwrap());
        let host = TempDir::new().unwrap();

        let leaks: Vec<String> = host_file_queries(host.path())
            .iter()
            .filter_map(|sql| leak(analyze_persistent(db_path.to_str().unwrap(), sql), sql))
            .collect();
        assert!(leaks.is_empty(), "{leaks:#?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_reports_journals_it_could_not_load_from_a_directory() {
        // The run leaves its experiment definition beside the journal, so
        // the journal is copied to a directory of its own.
        let run = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        std::fs::copy(write_run_journal(run.path()), dir.path().join("run.toon")).unwrap();
        let corrupt = dir.path().join("corrupt.toon");
        std::fs::write(&corrupt, "{{{{ not a journal").unwrap();

        let capture = tumult_otel::logging::testing::LogCapture::start(tracing::Level::INFO);
        let output = analyze(
            dir.path().to_str().unwrap(),
            "SELECT count(*) FROM experiments",
        )
        .unwrap();
        let logs = capture.snapshot();

        assert!(output.contains("1 row(s)"), "{output}");
        let skipped = logs.named("mcp.journal.skipped");
        assert_eq!(skipped.len(), 1, "{}", logs.raw);
        assert_eq!(skipped[0]["file.path"], corrupt.to_str().unwrap());
        assert!(skipped[0]["error.message"].is_string(), "{}", logs.raw);
    }
}
