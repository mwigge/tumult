//! Tool implementations for the Tumult MCP server.
//!
//! Each function handles a single MCP tool call and returns
//! structured text content.

use std::path::Path;

/// Validate an experiment file. Returns a summary string on success.
pub fn validate_experiment(experiment_path: &str) -> Result<String, String> {
    use tumult_core::engine::{parse_experiment, validate_experiment};

    let content = std::fs::read_to_string(Path::new(experiment_path))
        .map_err(|e| format!("read error: {}", e))?;
    let experiment = parse_experiment(&content).map_err(|e| format!("parse error: {}", e))?;
    validate_experiment(&experiment).map_err(|e| format!("validation error: {}", e))?;

    Ok(format!(
        "Valid: '{}' — {} method steps, {} rollbacks",
        experiment.title,
        experiment.method.len(),
        experiment.rollbacks.len()
    ))
}

/// Run an experiment and return the journal as TOON.
pub fn run_experiment(experiment_path: &str, rollback_strategy: &str) -> Result<String, String> {
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::{parse_experiment, validate_experiment};
    use tumult_core::execution::RollbackStrategy;
    use tumult_core::journal::encode_journal;
    use tumult_core::runner::{run_experiment as run, RunConfig};

    let content = std::fs::read_to_string(Path::new(experiment_path))
        .map_err(|e| format!("read error: {}", e))?;
    let experiment = parse_experiment(&content).map_err(|e| format!("parse error: {}", e))?;
    validate_experiment(&experiment).map_err(|e| format!("validation error: {}", e))?;

    let strategy = match rollback_strategy {
        "always" => RollbackStrategy::Always,
        "never" => RollbackStrategy::Never,
        _ => RollbackStrategy::OnDeviation,
    };

    let executor = crate::handler::ProcessExecutor;
    let controls = ControlRegistry::new();
    let config = RunConfig {
        rollback_strategy: strategy,
    };

    let journal =
        run(&experiment, &executor, &controls, &config).map_err(|e| format!("run error: {}", e))?;
    encode_journal(&journal).map_err(|e| format!("encode error: {}", e))
}

/// Analyze journals with a SQL query via DuckDB.
pub fn analyze(journals_path: &str, query: &str) -> Result<String, String> {
    use tumult_core::journal::read_journal;

    let store =
        tumult_analytics::AnalyticsStore::in_memory().map_err(|e| format!("store error: {}", e))?;

    let path = Path::new(journals_path);
    if path.is_file() {
        let journal = read_journal(path).map_err(|e| format!("read error: {}", e))?;
        store
            .ingest_journal(&journal)
            .map_err(|e| format!("ingest error: {}", e))?;
    } else if path.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| format!("dir error: {}", e))? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("toon") {
                if let Ok(journal) = read_journal(&entry.path()) {
                    let _ = store.ingest_journal(&journal);
                }
            }
        }
    }

    let columns = store
        .query_columns(query)
        .map_err(|e| format!("query error: {}", e))?;
    let rows = store
        .query(query)
        .map_err(|e| format!("query error: {}", e))?;

    let mut output = columns.join("\t") + "\n";
    for row in &rows {
        output += &row.join("\t");
        output += "\n";
    }
    output += &format!("{} row(s)", rows.len());
    Ok(output)
}

/// Read a TOON journal file.
pub fn read_journal(journal_path: &str) -> Result<String, String> {
    std::fs::read_to_string(journal_path).map_err(|e| format!("read error: {}", e))
}

/// List .toon journal files in a directory.
pub fn list_journals(directory: &str) -> Result<Vec<String>, String> {
    let mut journals = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| format!("dir error: {}", e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("toon") {
            journals.push(entry.path().display().to_string());
        }
    }
    Ok(journals)
}

/// Discover plugins and list their actions.
pub fn discover_plugins() -> String {
    use tumult_plugin::discovery::discover_all_plugins;
    use tumult_plugin::registry::PluginRegistry;

    let mut registry = PluginRegistry::new();
    if let Ok(manifests) = discover_all_plugins() {
        for manifest in manifests {
            registry.register_script(manifest);
        }
    }

    let plugins = registry.list_plugins();
    let actions = registry.list_all_actions();

    let mut output = format!("Plugins: {}\n", plugins.len());
    for name in &plugins {
        output += &format!("  {}\n", name);
    }
    output += &format!("Actions: {}\n", actions.len());
    for (plugin, desc) in &actions {
        output += &format!("  {}::{}\n", plugin, desc.name);
    }
    output
}

/// Create an experiment file from a template.
pub fn create_experiment(output_path: &str, plugin: Option<&str>) -> Result<String, String> {
    let path = Path::new(output_path);
    if path.exists() {
        return Err(format!("{} already exists", output_path));
    }

    let plugin_name = plugin.unwrap_or("tumult-example");
    let template = format!(
        r#"title: My chaos experiment
description: Describe what this experiment validates

tags[2]: resilience, testing

steady_state_hypothesis:
  title: System is reachable
  probes[1]:
    - name: system-check
      activity_type: probe
      provider:
        type: process
        path: uname
        arguments[1]: "-a"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "."

method[1]:
  - name: inject-fault
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "fault injected via {plugin_name}"
      timeout_s: 30.0
"#
    );

    std::fs::write(path, &template).map_err(|e| format!("write error: {}", e))?;
    Ok(format!("Created {}", output_path))
}

/// Query trace data from a journal — returns activity spans with trace/span IDs.
///
/// This is Option B: MCP observability resource. Agents can query past
/// experiment traces to understand execution timelines and correlate
/// with external observability systems.
pub fn query_traces(journal_path: &str) -> Result<String, String> {
    use tumult_core::journal::read_journal;

    let journal =
        read_journal(Path::new(journal_path)).map_err(|e| format!("read error: {}", e))?;

    let mut output = format!(
        "Experiment: {} ({})\nStatus: {:?}\nTrace data:\n\n",
        journal.experiment_title, journal.experiment_id, journal.status
    );

    // Hypothesis before
    if let Some(ref hyp) = journal.steady_state_before {
        output += &format!("Hypothesis Before: {}\n", hyp.title);
        for probe in &hyp.probe_results {
            output += &format!(
                "  {} [{:?}] trace={} span={} {}ms\n",
                probe.name,
                probe.status,
                if probe.trace_id.is_empty() {
                    "(none)"
                } else {
                    &probe.trace_id
                },
                if probe.span_id.is_empty() {
                    "(none)"
                } else {
                    &probe.span_id
                },
                probe.duration_ms,
            );
        }
    }

    // Method
    output += "\nMethod:\n";
    for result in &journal.method_results {
        output += &format!(
            "  {} [{:?}] trace={} span={} {}ms\n",
            result.name,
            result.status,
            if result.trace_id.is_empty() {
                "(none)"
            } else {
                &result.trace_id
            },
            if result.span_id.is_empty() {
                "(none)"
            } else {
                &result.span_id
            },
            result.duration_ms,
        );
    }

    // Hypothesis after
    if let Some(ref hyp) = journal.steady_state_after {
        output += &format!("\nHypothesis After: {}\n", hyp.title);
        for probe in &hyp.probe_results {
            output += &format!(
                "  {} [{:?}] trace={} span={} {}ms\n",
                probe.name, probe.status, probe.trace_id, probe.span_id, probe.duration_ms,
            );
        }
    }

    // Rollbacks
    if !journal.rollback_results.is_empty() {
        output += "\nRollbacks:\n";
        for result in &journal.rollback_results {
            output += &format!(
                "  {} [{:?}] trace={} span={} {}ms\n",
                result.name, result.status, result.trace_id, result.span_id, result.duration_ms,
            );
        }
    }

    Ok(output)
}

/// Query the persistent analytics store stats.
/// If `store_path` is empty, uses the default path.
pub fn store_stats(store_path: &str) -> Result<String, String> {
    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Err(format!("store not found: {}", store_path));
    }

    let store =
        tumult_analytics::AnalyticsStore::open(&path).map_err(|e| format!("open error: {}", e))?;
    let stats = store.stats().map_err(|e| format!("stats error: {}", e))?;
    let version = store
        .schema_version()
        .map_err(|e| format!("version error: {}", e))?;

    let mut output = format!("store: {}\n", store_path);
    output += &format!("schema_version: {}\n", version);
    output += &format!("experiments: {}\n", stats.experiment_count);
    output += &format!("activities: {}\n", stats.activity_count);

    if let Ok(meta) = std::fs::metadata(&path) {
        let mb = meta.len() as f64 / (1024.0 * 1024.0);
        output += &format!("size_mb: {:.2}\n", mb);
    }

    Ok(output)
}

/// Analyze using the persistent store directly (no journal loading).
pub fn analyze_persistent(store_path: &str, query: &str) -> Result<String, String> {
    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Err(format!("store not found: {}", store_path));
    }

    let store =
        tumult_analytics::AnalyticsStore::open(&path).map_err(|e| format!("open error: {}", e))?;

    let columns = store
        .query_columns(query)
        .map_err(|e| format!("query error: {}", e))?;
    let rows = store
        .query(query)
        .map_err(|e| format!("query error: {}", e))?;

    let mut output = columns.join("\t") + "\n";
    for row in &rows {
        output += &row.join("\t");
        output += "\n";
    }
    output += &format!("{} row(s)", rows.len());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_valid_experiment(dir: &std::path::Path) -> String {
        let exp = tumult_core::types::Experiment {
            title: "MCP test experiment".into(),
            method: vec![tumult_core::types::Activity {
                name: "echo-action".into(),
                activity_type: tumult_core::types::ActivityType::Action,
                provider: tumult_core::types::Provider::Process {
                    path: "echo".into(),
                    arguments: vec!["hello".into()],
                    env: std::collections::HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
            }],
            ..Default::default()
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        let path = dir.join("test.toon");
        std::fs::write(&path, toon).unwrap();
        path.to_str().unwrap().to_string()
    }

    // ── validate_experiment ───────────────────────────────────

    #[test]
    fn validate_valid_experiment_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let result = validate_experiment(&path);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("MCP test experiment"));
    }

    #[test]
    fn validate_nonexistent_file_returns_error() {
        let result = validate_experiment("/nonexistent/file.toon");
        assert!(result.is_err());
    }

    #[test]
    fn validate_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.toon");
        std::fs::write(&path, "not valid toon {{{").unwrap();
        let result = validate_experiment(path.to_str().unwrap());
        assert!(result.is_err());
    }

    // ── run_experiment ────────────────────────────────────────

    #[test]
    fn run_valid_experiment_returns_journal() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let result = run_experiment(&path, "on-deviation");
        assert!(result.is_ok());
        let journal = result.unwrap();
        assert!(journal.contains("MCP test experiment"));
    }

    #[test]
    fn run_nonexistent_returns_error() {
        let result = run_experiment("/nonexistent.toon", "always");
        assert!(result.is_err());
    }

    // ── analyze ───────────────────────────────────────────────

    #[test]
    fn analyze_returns_query_results() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());

        // First run the experiment to get a journal
        let journal_toon = run_experiment(&path, "always").unwrap();
        let journal_path = dir.path().join("journal.toon");
        std::fs::write(&journal_path, journal_toon).unwrap();

        let result = analyze(
            journal_path.to_str().unwrap(),
            "SELECT experiment_id, status FROM experiments",
        );
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("1 row(s)"));
    }

    // ── read_journal ──────────────────────────────────────────

    #[test]
    fn read_journal_returns_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.toon");
        std::fs::write(&path, "test content").unwrap();
        let result = read_journal(path.to_str().unwrap());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test content");
    }

    #[test]
    fn read_journal_nonexistent_returns_error() {
        let result = read_journal("/nonexistent.toon");
        assert!(result.is_err());
    }

    // ── list_journals ─────────────────────────────────────────

    #[test]
    fn list_journals_finds_toon_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.toon"), "").unwrap();
        std::fs::write(dir.path().join("b.toon"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let result = list_journals(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn list_journals_empty_dir() {
        let dir = TempDir::new().unwrap();
        let result = list_journals(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ── discover_plugins ──────────────────────────────────────

    #[test]
    fn discover_returns_formatted_output() {
        let output = discover_plugins();
        assert!(output.contains("Plugins:"));
        assert!(output.contains("Actions:"));
    }

    // ── create_experiment ─────────────────────────────────────

    #[test]
    fn create_experiment_writes_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.toon");
        let result = create_experiment(path.to_str().unwrap(), None);
        assert!(result.is_ok());
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("title:"));
    }

    #[test]
    fn create_experiment_with_plugin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("kafka.toon");
        let result = create_experiment(path.to_str().unwrap(), Some("tumult-kafka"));
        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tumult-kafka"));
    }

    #[test]
    fn create_experiment_fails_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.toon");
        std::fs::write(&path, "existing").unwrap();
        let result = create_experiment(path.to_str().unwrap(), None);
        assert!(result.is_err());
    }

    // ── query_traces ──────────────────────────────────────────

    #[test]
    fn query_traces_returns_activity_spans() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());

        // Run experiment to generate a journal
        let journal_toon = run_experiment(&exp_path, "always").unwrap();
        let journal_path = dir.path().join("journal.toon");
        std::fs::write(&journal_path, journal_toon).unwrap();

        let result = query_traces(journal_path.to_str().unwrap());
        assert!(result.is_ok());
        let output = result.unwrap();

        // Should contain experiment info
        assert!(output.contains("MCP test experiment"));
        assert!(output.contains("Method:"));
        assert!(output.contains("echo-action"));
    }

    #[test]
    fn query_traces_nonexistent_returns_error() {
        let result = query_traces("/nonexistent/journal.toon");
        assert!(result.is_err());
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
        let output = result.unwrap();
        assert!(output.contains("experiments: 0"));
        assert!(output.contains("schema_version: 1"));
    }

    #[test]
    fn store_stats_missing_store_returns_error() {
        let result = store_stats("/nonexistent/analytics.duckdb");
        assert!(result.is_err());
    }

    // ── analyze with persistent store ────────────────────────

    #[test]
    fn analyze_persistent_queries_store() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");

        // Pre-populate a persistent store
        {
            let store = tumult_analytics::AnalyticsStore::open(&db_path).unwrap();
            let exp_path = write_valid_experiment(dir.path());
            let journal_toon = run_experiment(&exp_path, "always").unwrap();
            // Write journal to file, then read back via tumult_core
            let journal_file = dir.path().join("journal.toon");
            std::fs::write(&journal_file, &journal_toon).unwrap();
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
