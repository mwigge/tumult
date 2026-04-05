//! Tool implementations for the Tumult MCP server.
//!
//! Each function handles a single MCP tool call and returns
//! structured text content.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::ToolError;

/// Validate that a SQL query is read-only (SELECT or WITH only).
///
/// Prevents SQL injection by rejecting any query that does not start
/// with SELECT or WITH (e.g., DROP, INSERT, UPDATE, DELETE, CREATE).
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if the query does not start with
/// `SELECT` or `WITH`.
pub fn validate_select_only(query: &str) -> Result<(), ToolError> {
    let normalized = query.trim().to_uppercase();
    if normalized.starts_with("SELECT") || normalized.starts_with("WITH") {
        Ok(())
    } else {
        Err(ToolError::InvalidInput(format!(
            "only SELECT/WITH queries are allowed, got: {}",
            normalized.split_whitespace().next().unwrap_or("(empty)")
        )))
    }
}

/// Validate that an action or probe name contains only safe characters.
///
/// Allowed characters: ASCII alphanumerics, hyphens (`-`), underscores (`_`),
/// and dots (`.`).  This whitelist prevents SQL injection when the name is
/// interpolated into a query string (e.g., in the `coverage` tool).
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if the name is empty or contains any
/// character outside the allowed set.
pub fn validate_action_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidInput(
            "action name must not be empty".into(),
        ));
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        Ok(())
    } else {
        Err(ToolError::InvalidInput(format!(
            "action name contains invalid characters: {name:?}"
        )))
    }
}

/// Resolve a user-supplied path safely within a base directory.
///
/// Joins `base` with `user_path`, canonicalizes the result, and verifies
/// the resolved path is still within `base`. This prevents directory
/// traversal attacks (e.g., `../../etc/passwd`).
///
/// # Errors
///
/// Returns [`ToolError::Path`] if the path cannot be canonicalized or if the
/// resolved path escapes the base directory.
pub fn safe_resolve_path(base: &Path, user_path: &str) -> Result<PathBuf, ToolError> {
    let candidate = base.join(user_path);
    let resolved = candidate
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("path resolution error: {e}")))?;
    let base_canonical = base
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("base path resolution error: {e}")))?;
    if resolved.starts_with(&base_canonical) {
        Ok(resolved)
    } else {
        Err(ToolError::Path(format!(
            "path traversal detected: resolved path {} is outside base {}",
            resolved.display(),
            base_canonical.display()
        )))
    }
}

/// Validate an experiment file. Returns a summary string on success.
///
/// # Errors
///
/// Returns a [`ToolError`] if the file cannot be read, parsed, or fails
/// validation.
pub fn validate_experiment(experiment_path: &str) -> Result<String, ToolError> {
    use tumult_core::engine::{parse_experiment, validate_experiment};

    let content = std::fs::read_to_string(Path::new(experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    Ok(format!(
        "Valid: '{}' — {} method steps, {} rollbacks",
        experiment.title,
        experiment.method.len(),
        experiment.rollbacks.len()
    ))
}

/// Run an experiment and return the journal as TOON.
///
/// `parent_context` is an optional `OTel` context to link the root
/// `resilience.experiment` span to an upstream caller (e.g. an MCP tool span).
///
/// # Errors
///
/// Returns a [`ToolError`] if the file cannot be read, parsed, validated,
/// executed, or encoded.
pub fn run_experiment(
    experiment_path: &str,
    rollback_strategy: &str,
    parent_context: Option<opentelemetry::Context>,
) -> Result<String, ToolError> {
    use std::sync::Arc;
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::{parse_experiment, validate_experiment};
    use tumult_core::execution::RollbackStrategy;
    use tumult_core::journal::encode_journal;
    use tumult_core::runner::{run_experiment as run, ActivityExecutor, RunConfig};

    let content = std::fs::read_to_string(Path::new(experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    let strategy = match rollback_strategy {
        "always" => RollbackStrategy::Always,
        "never" => RollbackStrategy::Never,
        _ => RollbackStrategy::OnDeviation,
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(crate::handler::ProcessExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig {
        rollback_strategy: strategy,
        cancellation_token: None,
        parent_context,
        load_executor: None,
    };

    let journal = run(&experiment, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    encode_journal(&journal).map_err(|e| ToolError::Execution(e.to_string()))
}

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

/// Read a TOON journal file.
///
/// # Errors
///
/// Returns [`ToolError::Io`] if the file cannot be read.
pub fn read_journal(journal_path: &str) -> Result<String, ToolError> {
    std::fs::read_to_string(journal_path).map_err(ToolError::Io)
}

/// List .toon journal files in a directory.
///
/// # Errors
///
/// Returns [`ToolError::Io`] if the directory cannot be read.
pub fn list_journals(directory: &str) -> Result<Vec<String>, ToolError> {
    let mut journals = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("toon") {
            journals.push(entry.path().display().to_string());
        }
    }
    Ok(journals)
}

/// Discover plugins and list their actions.
#[must_use]
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
        let _ = writeln!(output, "  {name}");
    }
    let _ = writeln!(output, "Actions: {}", actions.len());
    for (plugin, desc) in &actions {
        let _ = writeln!(output, "  {}::{}", plugin, desc.name);
    }
    output
}

/// Create an experiment file from a template.
///
/// # Errors
///
/// Returns [`ToolError::AlreadyExists`] if the file already exists, or
/// [`ToolError::Io`] if the file cannot be written.
pub fn create_experiment(output_path: &str, plugin: Option<&str>) -> Result<String, ToolError> {
    let path = Path::new(output_path);
    if path.exists() {
        return Err(ToolError::AlreadyExists(format!(
            "{output_path} already exists"
        )));
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

    std::fs::write(path, &template)?;
    Ok(format!("Created {output_path}"))
}

/// Query trace data from a journal — returns activity spans with trace/span IDs.
///
/// This is Option B: MCP observability resource. Agents can query past
/// experiment traces to understand execution timelines and correlate
/// with external observability systems.
///
/// # Errors
///
/// Returns a [`ToolError`] if the journal file cannot be read or decoded.
pub fn query_traces(journal_path: &str) -> Result<String, ToolError> {
    use tumult_core::journal::read_journal;

    let journal =
        read_journal(Path::new(journal_path)).map_err(|e| ToolError::Parse(e.to_string()))?;

    let mut output = format!(
        "Experiment: {} ({})\nStatus: {:?}\nTrace data:\n\n",
        journal.experiment_title, journal.experiment_id, journal.status
    );

    // Hypothesis before
    if let Some(ref hyp) = journal.steady_state_before {
        let _ = writeln!(output, "Hypothesis Before: {}", hyp.title);
        for probe in &hyp.probe_results {
            let _ = writeln!(
                output,
                "  {} [{:?}] trace={} span={} {}ms",
                probe.name,
                probe.status,
                if probe.trace_id.is_empty() {
                    "(none)"
                } else {
                    probe.trace_id.as_str()
                },
                if probe.span_id.is_empty() {
                    "(none)"
                } else {
                    probe.span_id.as_str()
                },
                probe.duration_ms,
            );
        }
    }

    // Method
    output += "\nMethod:\n";
    for result in &journal.method_results {
        let _ = writeln!(
            output,
            "  {} [{:?}] trace={} span={} {}ms",
            result.name,
            result.status,
            if result.trace_id.is_empty() {
                "(none)"
            } else {
                result.trace_id.as_str()
            },
            if result.span_id.is_empty() {
                "(none)"
            } else {
                result.span_id.as_str()
            },
            result.duration_ms,
        );
    }

    // Hypothesis after
    if let Some(ref hyp) = journal.steady_state_after {
        output += "\n";
        let _ = writeln!(output, "Hypothesis After: {}", hyp.title);
        for probe in &hyp.probe_results {
            let _ = writeln!(
                output,
                "  {} [{:?}] trace={} span={} {}ms",
                probe.name, probe.status, probe.trace_id, probe.span_id, probe.duration_ms,
            );
        }
    }

    // Rollbacks
    if !journal.rollback_results.is_empty() {
        output += "\nRollbacks:\n";
        for result in &journal.rollback_results {
            let _ = writeln!(
                output,
                "  {} [{:?}] trace={} span={} {}ms",
                result.name, result.status, result.trace_id, result.span_id, result.duration_ms,
            );
        }
    }

    Ok(output)
}

/// Query the persistent analytics store stats.
/// If `store_path` is empty, uses the default path.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store does not exist, cannot be opened, or
/// the stats/schema-version query fails.
pub fn store_stats(store_path: &str) -> Result<String, ToolError> {
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

    let mut output = format!("store: {store_path}\n");
    let _ = writeln!(output, "schema_version: {version}");
    let _ = writeln!(output, "experiments: {}", stats.experiment_count);
    let _ = writeln!(output, "activities: {}", stats.activity_count);

    if let Ok(meta) = std::fs::metadata(&path) {
        // u64 → f64: file sizes in megabytes; precision loss is acceptable for display.
        #[allow(clippy::cast_precision_loss)]
        let mb = meta.len() as f64 / (1024.0 * 1024.0);
        let _ = writeln!(output, "size_mb: {mb:.2}");
    }

    Ok(output)
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

/// List all `.toon` experiment files found recursively under `search_root`.
///
/// Each result line contains the file name, relative path, and the `title`
/// field parsed from the experiment. Files that cannot be parsed are skipped.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `search_root` directory cannot be read.
pub fn list_experiments(search_root: &str) -> Result<String, ToolError> {
    let root = Path::new(search_root);
    let mut results: Vec<String> = Vec::new();

    collect_toon_files(root, root, &mut results)?;

    if results.is_empty() {
        return Ok("No experiment files found.".to_string());
    }

    let count = results.len();
    let mut output = format!("Experiments: {count}\n");
    for line in &results {
        output += line;
        output += "\n";
    }
    Ok(output)
}

/// Recursively collect `.toon` experiment entries under `dir`.
fn collect_toon_files(base: &Path, dir: &Path, results: &mut Vec<String>) -> Result<(), ToolError> {
    let read_dir = std::fs::read_dir(dir).map_err(ToolError::Io)?;

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse, but ignore errors from subdirectories (permissions etc.)
            let _ = collect_toon_files(base, &path, results);
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("toon") {
            continue;
        }

        // Try to extract the title field; skip files that aren't experiments.
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        // Quick parse: look for `title:` line (TOON format) or JSON/YAML title key.
        let title = extract_title(&content);
        let Some(title) = title else { continue };

        let rel = path
            .strip_prefix(base)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        results.push(format!("  name={name}  path={rel}  title={title}"));
    }

    Ok(())
}

/// Extract the `title` field from a TOON file's raw text content.
///
/// Supports both `title: value` (TOON/YAML) and `"title": "value"` (JSON) formats.
/// Returns `None` if no title field is found or the value is empty.
fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        // TOON / YAML style: `title: My experiment`
        if let Some(rest) = trimmed.strip_prefix("title:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        // JSON style: `"title": "My experiment"`
        if let Some(rest) = trimmed.strip_prefix("\"title\":") {
            let value = rest
                .trim()
                .trim_matches('"')
                .trim_matches(',')
                .trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ── GameDay tools ─────────────────────────────────────────────

/// Runs a `GameDay` — all experiments under shared load.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `GameDay` cannot be read, parsed,
/// or any experiment fails to execute.
#[allow(clippy::too_many_lines)] // GameDay orchestration spans load setup, multi-experiment execution, and result aggregation
pub fn gameday_run(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::parse_experiment;
    use tumult_core::runner::{run_gameday, RunConfig};
    use tumult_core::types::GameDay;

    let path = Path::new(gameday_path);
    let content = std::fs::read_to_string(path)?;

    let gameday: GameDay = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse gameday: {e}")))?;

    let gameday_dir = path.parent().unwrap_or(Path::new("."));

    let mut experiments = Vec::new();
    for gd_exp in &gameday.experiments {
        let exp_path = if gd_exp.path.is_absolute() {
            gd_exp.path.clone()
        } else {
            gameday_dir.join(&gd_exp.path)
        };
        let exp_content = std::fs::read_to_string(&exp_path)?;
        let experiment = parse_experiment(&exp_content).map_err(|e| {
            ToolError::Parse(format!("failed to parse {}: {e}", exp_path.display()))
        })?;
        experiments.push(experiment);
    }

    let executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(super::handler::ProcessExecutor);
    let controls = std::sync::Arc::new(ControlRegistry::new());
    let config = RunConfig::default();

    let journal = run_gameday(&gameday, &experiments, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(format!("gameday failed: {e}")))?;

    // Write journal
    let journal_path = path.with_extension("journal.toon");
    let toon_out = toon_format::encode_default(&journal)
        .map_err(|e| ToolError::Execution(format!("failed to encode journal: {e}")))?;
    std::fs::write(&journal_path, &toon_out)?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(
        output,
        "Resilience Score: {:.2}",
        journal.resilience_score.overall
    )
    .ok();
    writeln!(
        output,
        "Experiments: {}/{} passed",
        journal
            .experiment_journals
            .iter()
            .filter(|j| j.status == tumult_core::types::ExperimentStatus::Completed)
            .count(),
        journal.experiment_journals.len()
    )
    .ok();
    writeln!(output, "Journal: {}", journal_path.display()).ok();

    Ok(output)
}

/// Analyzes a completed `GameDay` journal.
///
/// # Errors
///
/// Returns a [`ToolError`] if the journal cannot be read or parsed.
pub fn gameday_analyze(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::types::GameDayJournal;

    let path = Path::new(gameday_path);
    let journal_path = path.with_extension("journal.toon");
    let content = std::fs::read_to_string(&journal_path)?;

    let journal: GameDayJournal = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse: {e}")))?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(output, "Score: {:.2}", journal.resilience_score.overall).ok();
    writeln!(
        output,
        "  Pass rate: {:.2}",
        journal.resilience_score.pass_rate
    )
    .ok();
    writeln!(
        output,
        "  Recovery: {:.2}",
        journal.resilience_score.recovery_compliance
    )
    .ok();
    writeln!(
        output,
        "  Load impact: {:.2}",
        journal.resilience_score.load_impact_tolerance
    )
    .ok();
    writeln!(
        output,
        "  Compliance: {:.2}",
        journal.resilience_score.compliance_coverage
    )
    .ok();

    for (i, ej) in journal.experiment_journals.iter().enumerate() {
        let icon = if ej.status == tumult_core::types::ExperimentStatus::Completed {
            "PASS"
        } else {
            "FAIL"
        };
        writeln!(
            output,
            "  #{} [{}] {} ({}ms)",
            i + 1,
            icon,
            ej.experiment_title,
            ej.duration_ms
        )
        .ok();
    }

    Ok(output)
}

/// Lists `.gameday.toon` files found recursively under `search_root`.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if `search_root` is not a directory.
pub fn gameday_list(search_root: &str) -> Result<String, ToolError> {
    let root = Path::new(search_root);
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(format!(
            "not a directory: {search_root}"
        )));
    }

    let mut entries = Vec::new();
    collect_gameday_files(root, &mut entries);

    if entries.is_empty() {
        return Ok("No .gameday.toon files found.".to_string());
    }

    let mut output = String::new();
    for (path, title) in &entries {
        writeln!(output, "{title}  ({path})").ok();
    }
    Ok(output)
}

fn collect_gameday_files(dir: &Path, entries: &mut Vec<(String, String)>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gameday_files(&path, entries);
        } else if path.extension().and_then(|e| e.to_str()) == Some("toon")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".gameday"))
        {
            let title = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| extract_title(&c))
                .unwrap_or_else(|| "(untitled)".to_string());
            entries.push((path.display().to_string(), title));
        }
    }
}

// ── Intelligence tools (agent reasoning) ────────────────────────

/// Returns recommendations for what to test next, based on coverage gaps
/// in the persistent analytics store.
///
/// Analyzes which plugins, actions, and fault types have been tested
/// vs available, which experiments fail most, and which targets lack
/// recent testing.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store cannot be opened or queried.
#[allow(clippy::too_many_lines)] // Recommendation logic covers multiple metrics and formatting stages; splitting would not reduce complexity
pub fn recommend(store_path: &str) -> Result<String, ToolError> {
    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Ok("No analytics store found. Run some experiments first.".to_string());
    }

    let store = tumult_analytics::AnalyticsStore::open(&path)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut output = String::new();

    // 1. Available plugins vs tested
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let available_actions: Vec<String> = available_plugins
        .iter()
        .flat_map(|p| {
            p.actions
                .iter()
                .map(move |a| format!("{}::{}", p.name, a.name))
        })
        .collect();

    // 2. Which actions have been executed (from activity_results)?
    let tested_actions = store
        .query("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")
        .unwrap_or_default();
    let tested_set: std::collections::HashSet<String> = tested_actions
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect();

    // 3. Find untested actions
    let untested: Vec<&String> = available_actions
        .iter()
        .filter(|a| {
            let short_name = a.split("::").nth(1).unwrap_or(a);
            !tested_set.contains(short_name)
        })
        .collect();

    writeln!(output, "=== Recommendations ===").ok();
    writeln!(output).ok();

    // Coverage gaps
    writeln!(
        output,
        "Coverage: {}/{} actions tested ({:.0}%)",
        available_actions.len() - untested.len(),
        available_actions.len(),
        if available_actions.is_empty() {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                ((available_actions.len() - untested.len()) as f64 / available_actions.len() as f64)
                    * 100.0
            }
        }
    )
    .ok();

    if !untested.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Untested actions ({}):", untested.len()).ok();
        for action in untested.iter().take(15) {
            writeln!(output, "  - {action}").ok();
        }
        if untested.len() > 15 {
            writeln!(output, "  ... and {} more", untested.len() - 15).ok();
        }
    }

    // 4. Most failing experiments
    let failures = store
        .query(
            "SELECT title, count(*) as fails FROM experiments \
             WHERE status != 'completed' GROUP BY title \
             ORDER BY fails DESC LIMIT 5",
        )
        .unwrap_or_default();

    if !failures.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Most failing experiments:").ok();
        for row in &failures {
            if row.len() >= 2 {
                writeln!(output, "  {} ({} failures)", row[0], row[1]).ok();
            }
        }
    }

    // 5. Stale experiments (not run recently)
    let stale = store
        .query(
            "SELECT title, max(started_at_ns) as last_run \
             FROM experiments GROUP BY title \
             ORDER BY last_run ASC LIMIT 5",
        )
        .unwrap_or_default();

    if !stale.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Oldest experiments (consider re-running):").ok();
        for row in &stale {
            if !row.is_empty() {
                writeln!(output, "  - {}", row[0]).ok();
            }
        }
    }

    // 6. Suggested next steps
    writeln!(output).ok();
    writeln!(output, "Suggested next steps:").ok();
    if !untested.is_empty() {
        writeln!(
            output,
            "  1. Test untested actions — {} actions have never been executed",
            untested.len()
        )
        .ok();
    }
    if !failures.is_empty() {
        writeln!(
            output,
            "  2. Investigate failures — {} experiment types have non-passing runs",
            failures.len()
        )
        .ok();
    }
    writeln!(
        output,
        "  3. Run a GameDay to validate end-to-end resilience with compliance scoring"
    )
    .ok();

    Ok(output)
}

/// Returns a coverage report — which plugins, targets, and fault types
/// have been tested vs what is available.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store cannot be opened or queried.
pub fn coverage(store_path: &str) -> Result<String, ToolError> {
    let path = std::path::PathBuf::from(store_path);

    // Available capabilities
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let mut output = String::new();

    writeln!(output, "=== Coverage Report ===").ok();
    writeln!(output).ok();

    // Plugin-level coverage
    writeln!(output, "Plugin coverage:").ok();

    let store = if path.exists() {
        tumult_analytics::AnalyticsStore::open(&path).ok()
    } else {
        None
    };

    for plugin in &available_plugins {
        let action_count = plugin.actions.len();
        let probe_count = plugin.probes.len();

        let tested_count = if let Some(ref s) = store {
            // Count distinct action names from this plugin that appear in results
            let action_names: Vec<String> = plugin.actions.iter().map(|a| a.name.clone()).collect();
            let mut count = 0;
            for name in &action_names {
                // Validate the name before interpolating into a query to
                // prevent SQL injection via a crafted plugin manifest.
                if validate_action_name(name).is_err() {
                    continue;
                }
                let q =
                    format!("SELECT count(*) FROM activity_results WHERE name = '{name}' LIMIT 1");
                if let Ok(rows) = s.query(&q) {
                    if let Some(row) = rows.first() {
                        if let Some(val) = row.first() {
                            if val != "0" {
                                count += 1;
                            }
                        }
                    }
                }
            }
            count
        } else {
            0
        };

        let status = if tested_count == action_count && action_count > 0 {
            "FULL"
        } else if tested_count > 0 {
            "PARTIAL"
        } else {
            "NONE"
        };

        writeln!(
            output,
            "  {:<25} {tested_count}/{action_count} actions tested, {probe_count} probes  [{status}]",
            plugin.name
        )
        .ok();
    }

    // Summary stats from store
    if let Some(ref s) = store {
        writeln!(output).ok();
        writeln!(output, "Store summary:").ok();

        let experiment_count = s
            .query("SELECT count(*) FROM experiments")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());
        let activity_count = s
            .query("SELECT count(*) FROM activity_results")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());
        let pass_count = s
            .query("SELECT count(*) FROM experiments WHERE status = 'completed'")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());

        writeln!(output, "  Experiments: {experiment_count}").ok();
        writeln!(output, "  Activities: {activity_count}").ok();
        writeln!(output, "  Pass rate: {pass_count}/{experiment_count}").ok();

        // Distinct targets
        let targets = s
            .query("SELECT DISTINCT title FROM experiments ORDER BY title")
            .unwrap_or_default();
        writeln!(output, "  Distinct experiment types: {}", targets.len()).ok();
    } else {
        writeln!(output).ok();
        writeln!(
            output,
            "No analytics store found. Run experiments to build coverage data."
        )
        .ok();
    }

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
                label_selector: None,
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_valid_experiment_returns_journal() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let result = run_experiment(&path, "on-deviation", None);
        assert!(result.is_ok());
        let journal = result.unwrap();
        assert!(journal.contains("MCP test experiment"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_nonexistent_returns_error() {
        let result = run_experiment("/nonexistent.toon", "always", None);
        assert!(result.is_err());
    }

    // ── analyze ───────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_returns_query_results() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());

        // First run the experiment to get a journal
        let journal_toon = run_experiment(&path, "always", None).unwrap();
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn query_traces_returns_activity_spans() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());

        // Run experiment to generate a journal
        let journal_toon = run_experiment(&exp_path, "always", None).unwrap();
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

    // ── validate_select_only ─────────────────────────────────

    #[test]
    fn validate_select_only_allows_select() {
        assert!(validate_select_only("SELECT * FROM experiments").is_ok());
    }

    #[test]
    fn validate_select_only_allows_with() {
        assert!(validate_select_only("WITH cte AS (SELECT 1) SELECT * FROM cte").is_ok());
    }

    #[test]
    fn validate_select_only_allows_lowercase() {
        assert!(validate_select_only("select count(*) from experiments").is_ok());
    }

    #[test]
    fn validate_select_only_allows_whitespace_prefix() {
        assert!(validate_select_only("  SELECT 1").is_ok());
    }

    #[test]
    fn validate_select_only_rejects_drop() {
        let result = validate_select_only("DROP TABLE experiments");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("only SELECT/WITH"));
    }

    #[test]
    fn validate_select_only_rejects_insert() {
        assert!(validate_select_only("INSERT INTO experiments VALUES (1)").is_err());
    }

    #[test]
    fn validate_select_only_rejects_update() {
        assert!(validate_select_only("UPDATE experiments SET x=1").is_err());
    }

    #[test]
    fn validate_select_only_rejects_delete() {
        assert!(validate_select_only("DELETE FROM experiments").is_err());
    }

    #[test]
    fn validate_select_only_rejects_create() {
        assert!(validate_select_only("CREATE TABLE foo (id int)").is_err());
    }

    #[test]
    fn validate_select_only_rejects_empty() {
        assert!(validate_select_only("").is_err());
    }

    // ── validate_action_name ─────────────────────────────────

    #[test]
    fn validate_action_name_allows_simple_name() {
        assert!(validate_action_name("kill-process").is_ok());
    }

    #[test]
    fn validate_action_name_allows_underscores_and_dots() {
        assert!(validate_action_name("cpu_stress.v2").is_ok());
    }

    #[test]
    fn validate_action_name_rejects_single_quote() {
        assert!(validate_action_name("name' OR '1'='1").is_err());
    }

    #[test]
    fn validate_action_name_rejects_semicolon() {
        assert!(validate_action_name("name; DROP TABLE activity_results --").is_err());
    }

    #[test]
    fn validate_action_name_rejects_empty() {
        assert!(validate_action_name("").is_err());
    }

    #[test]
    fn analyze_rejects_non_select_query() {
        let dir = TempDir::new().unwrap();
        let result = analyze(dir.path().to_str().unwrap(), "DROP TABLE experiments");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("only SELECT/WITH"));
    }

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

    // ── safe_resolve_path ────────────────────────────────────

    #[test]
    fn safe_resolve_path_allows_file_within_base() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.toon");
        std::fs::write(&file, "content").unwrap();
        let result = safe_resolve_path(dir.path(), "test.toon");
        assert!(result.is_ok());
    }

    #[test]
    fn safe_resolve_path_allows_subdirectory() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("test.toon");
        std::fs::write(&file, "content").unwrap();
        let result = safe_resolve_path(dir.path(), "sub/test.toon");
        assert!(result.is_ok());
    }

    #[test]
    fn safe_resolve_path_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        let result = safe_resolve_path(dir.path(), "../../etc/passwd");
        // Either path resolution error (file doesn't exist) or traversal detected
        assert!(result.is_err());
    }

    #[test]
    fn safe_resolve_path_rejects_absolute_escape() {
        let dir = TempDir::new().unwrap();
        // An absolute path that's outside the base
        let result = safe_resolve_path(dir.path(), "/etc/hosts");
        assert!(result.is_err());
    }

    // ── analyze with persistent store ────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn analyze_persistent_queries_store() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("analytics.duckdb");

        // Pre-populate a persistent store
        {
            let store = tumult_analytics::AnalyticsStore::open(&db_path).unwrap();
            let exp_path = write_valid_experiment(dir.path());
            let journal_toon = run_experiment(&exp_path, "always", None).unwrap();
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

    // ── list_experiments ─────────────────────────────────────

    #[test]
    fn list_experiments_finds_toon_files() {
        let dir = TempDir::new().unwrap();

        // Write two experiment files with title fields.
        let exp1 = "title: First Experiment\nmethod[0]:\n";
        let exp2 = "title: Second Experiment\nmethod[0]:\n";
        // A journal file — no title field so it should NOT appear.
        let not_exp = "status: completed\n";
        // A non-.toon file — must be ignored.
        let not_toon = "title: ignored\n";

        std::fs::write(dir.path().join("first.toon"), exp1).unwrap();
        std::fs::write(dir.path().join("second.toon"), exp2).unwrap();
        std::fs::write(dir.path().join("journal.toon"), not_exp).unwrap();
        std::fs::write(dir.path().join("readme.md"), not_toon).unwrap();

        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok(), "list_experiments should succeed");
        let output = result.unwrap();

        assert!(output.contains("First Experiment"), "must include first");
        assert!(output.contains("Second Experiment"), "must include second");
        assert!(
            !output.contains("readme.md"),
            "non-.toon file must be excluded"
        );
        // Count: exactly 2 experiments found.
        assert!(output.contains("Experiments: 2"), "count must be 2");
    }

    #[test]
    fn list_experiments_empty_dir() {
        let dir = TempDir::new().unwrap();
        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("No experiment files found."));
    }

    #[test]
    fn list_experiments_skips_toon_without_title() {
        let dir = TempDir::new().unwrap();
        // File with no title field is skipped.
        std::fs::write(dir.path().join("no_title.toon"), "status: done\n").unwrap();
        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No experiment files found."));
    }

    #[test]
    fn list_experiments_recurses_subdirectories() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.toon"), "title: Deep Experiment\n").unwrap();

        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("Deep Experiment"),
            "must recurse into subdirectory"
        );
    }
}
