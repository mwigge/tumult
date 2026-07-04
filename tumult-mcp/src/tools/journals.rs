//! Journal and plugin discovery tools: read/list journals, trace queries, plugins.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;

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

/// Discover plugins of both kinds and list their actions.
///
/// Script plugins are read from the filesystem search paths; native plugins
/// come from the server's composition-root registry (`crate::native`), so
/// the same plugins the runner can dispatch to are the ones reported here.
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
    let native = crate::native::registry();

    // (name, kind) pairs, merged and sorted by name.
    let mut plugins: Vec<(String, &str)> = registry
        .list_plugins()
        .into_iter()
        .map(|name| (name, "script"))
        .collect();
    plugins.extend(
        native
            .plugin_names()
            .into_iter()
            .map(|name| (name.to_string(), "native")),
    );
    plugins.sort();

    // Actions of both kinds, sorted for stable output.
    let mut actions: Vec<String> = registry
        .list_all_actions()
        .iter()
        .map(|(plugin, desc)| format!("{plugin}::{}", desc.name))
        .collect();
    actions.extend(native.qualified_functions());
    actions.sort();

    let mut output = format!("Plugins: {}\n", plugins.len());
    for (name, kind) in &plugins {
        let _ = writeln!(output, "  {name} ({kind})");
    }
    let _ = writeln!(output, "Actions: {}", actions.len());
    for action in &actions {
        let _ = writeln!(output, "  {action}");
    }
    output
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::experiment::run_experiment;
    use crate::tools::test_support::write_valid_experiment;
    use tempfile::TempDir;

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

    #[test]
    fn discover_includes_native_plugins_with_functions() {
        let output = discover_plugins();
        assert!(output.contains("tumult-kubernetes (native)"));
        assert!(output.contains("tumult-net (native)"));
        assert!(output.contains("tumult-ssh (native)"));
        assert!(output.contains("tumult-ssh::execute"));
        assert!(output.contains("tumult-kubernetes::delete_pod"));
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
}
