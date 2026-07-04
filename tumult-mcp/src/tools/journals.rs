//! Journal and plugin discovery tools: read/list journals, trace queries, plugins.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// Read a TOON journal file and return it as structured data.
///
/// The structured object always contains `summary` (title, id, status,
/// timing, and result counts) and — unless `summary_only` — `journal`, the
/// full journal as JSON. The text content is that object as pretty JSON
/// (`format: "json"`, default) or the raw TOON file (`format: "toon"`,
/// only when the full journal is requested); text is capped at 512 KiB.
///
/// # Errors
///
/// Returns a [`ToolError`] if the format is invalid, the file cannot be
/// read, or its contents do not decode as a journal.
pub fn read_journal(
    journal_path: &str,
    format: &str,
    summary_only: bool,
) -> Result<StructuredReport, ToolError> {
    use tumult_core::types::Journal;

    if format != "json" && format != "toon" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported format '{format}'; expected json or toon"
        )));
    }

    let raw = std::fs::read_to_string(journal_path).map_err(ToolError::Io)?;
    let journal: Journal = toon_format::decode_default(&raw)
        .map_err(|e| ToolError::Parse(format!("not a valid journal file: {e}")))?;

    let summary = serde_json::json!({
        "experiment_title": journal.experiment_title,
        "experiment_id": journal.experiment_id,
        "status": journal.status,
        "started_at_ns": journal.started_at_ns,
        "duration_ms": journal.duration_ms,
        "method_count": journal.method_results.len(),
        "rollback_count": journal.rollback_results.len(),
        "rollback_failures": journal.rollback_failures,
    });

    let mut structured = serde_json::Map::new();
    structured.insert("summary".into(), summary);
    if !summary_only {
        structured.insert(
            "journal".into(),
            serde_json::to_value(&journal).map_err(|e| ToolError::Execution(e.to_string()))?,
        );
    }

    let text = if format == "toon" && !summary_only {
        raw
    } else {
        serde_json::to_string_pretty(&serde_json::Value::Object(structured.clone()))
            .map_err(|e| ToolError::Execution(e.to_string()))?
    };

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "pass summary=true for a compact view"),
        structured,
    })
}

/// List .toon journal files in a directory, sorted by path.
///
/// Returns one page of `limit` entries starting at `offset`. The structured
/// object is `{items, total, offset, limit}`; the text content keeps the
/// legacy newline-separated path lines (for the returned page only).
///
/// # Errors
///
/// Returns [`ToolError::Io`] if the directory cannot be read.
pub fn list_journals(
    directory: &str,
    limit: usize,
    offset: usize,
) -> Result<StructuredReport, ToolError> {
    let mut journals = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("toon") {
            journals.push(entry.path().display().to_string());
        }
    }
    journals.sort();
    let total = journals.len();
    let items: Vec<String> = journals.into_iter().skip(offset).take(limit).collect();

    let text = items.join("\n");
    let mut structured = serde_json::Map::new();
    structured.insert("items".into(), serde_json::json!(items));
    structured.insert("total".into(), serde_json::json!(total));
    structured.insert("offset".into(), serde_json::json!(offset));
    structured.insert("limit".into(), serde_json::json!(limit));
    Ok(StructuredReport { text, structured })
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
    use crate::tools::test_support::write_run_journal;
    use tempfile::TempDir;

    // ── read_journal ──────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_journal_json_returns_full_journal_and_summary() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = read_journal(journal_path.to_str().unwrap(), "json", false).unwrap();

        // Structured content: summary always, journal when not summary-only.
        let summary = result.structured.get("summary").unwrap();
        assert_eq!(summary["experiment_title"], "MCP test experiment");
        assert_eq!(summary["method_count"], 1);
        let journal = result.structured.get("journal").unwrap();
        assert_eq!(journal["experiment_title"], "MCP test experiment");

        // Text is the same object as pretty JSON.
        let parsed: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(parsed["summary"], *summary);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_journal_summary_only_omits_full_journal() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = read_journal(journal_path.to_str().unwrap(), "json", true).unwrap();
        assert!(result.structured.contains_key("summary"));
        assert!(
            !result.structured.contains_key("journal"),
            "summary mode must omit the full journal"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_journal_toon_returns_raw_content_with_structure() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());
        let raw = std::fs::read_to_string(&journal_path).unwrap();

        let result = read_journal(journal_path.to_str().unwrap(), "toon", false).unwrap();
        assert_eq!(result.text, raw, "toon format must return the raw file");
        assert!(result.structured.contains_key("journal"));
        assert!(result.structured.contains_key("summary"));
    }

    #[test]
    fn read_journal_rejects_unknown_format() {
        let result = read_journal("/nonexistent.toon", "yaml", false);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected json or toon"));
    }

    #[test]
    fn read_journal_rejects_non_journal_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.toon");
        std::fs::write(&path, "test content").unwrap();
        let result = read_journal(path.to_str().unwrap(), "json", false);
        assert!(
            result.is_err(),
            "content that is not a journal must be rejected"
        );
    }

    #[test]
    fn read_journal_nonexistent_returns_error() {
        let result = read_journal("/nonexistent.toon", "json", false);
        assert!(result.is_err());
    }

    // ── list_journals ─────────────────────────────────────────

    #[test]
    fn list_journals_finds_toon_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.toon"), "").unwrap();
        std::fs::write(dir.path().join("b.toon"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let report = list_journals(dir.path().to_str().unwrap(), 100, 0).unwrap();
        assert_eq!(report.structured["total"], 2);
        assert_eq!(report.structured["items"].as_array().unwrap().len(), 2);
        assert!(report.text.contains("a.toon") && report.text.contains("b.toon"));
        assert!(!report.text.contains("c.txt"));
    }

    #[test]
    fn list_journals_empty_dir() {
        let dir = TempDir::new().unwrap();
        let report = list_journals(dir.path().to_str().unwrap(), 100, 0).unwrap();
        assert_eq!(report.structured["total"], 0);
        assert!(report.structured["items"].as_array().unwrap().is_empty());
        assert!(report.text.is_empty());
    }

    #[test]
    fn list_journals_pages_sorted_entries() {
        let dir = TempDir::new().unwrap();
        for name in ["c.toon", "a.toon", "b.toon"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        // Page of 1 at offset 1 of the sorted set: exactly b.toon.
        let report = list_journals(dir.path().to_str().unwrap(), 1, 1).unwrap();
        assert_eq!(report.structured["total"], 3);
        assert_eq!(report.structured["offset"], 1);
        assert_eq!(report.structured["limit"], 1);
        let items = report.structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].as_str().unwrap().ends_with("b.toon"));
        assert!(report.text.ends_with("b.toon"));

        // Offset past the end yields an empty page but the true total.
        let report = list_journals(dir.path().to_str().unwrap(), 10, 10).unwrap();
        assert_eq!(report.structured["total"], 3);
        assert!(report.structured["items"].as_array().unwrap().is_empty());
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
        // Run experiment to generate a journal
        let journal_path = write_run_journal(dir.path());

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
