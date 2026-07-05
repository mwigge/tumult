//! Reporting tools: journal reports, compliance summaries, and metric trends.

mod compliance;
mod trend;

pub use compliance::compliance;
pub use trend::trend;

use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

use tumult_core::journal::read_journal;
use tumult_core::types::Journal;

/// Render a journal as a `json` or `junit` report (mirrors `tumult report`;
/// HTML/PDF remain CLI-only).
///
/// With `output_path` (already resolved/contained by the caller) the report
/// is written to disk and the structured object carries `output_path`;
/// otherwise the content is returned inline in both the text and the
/// structured `content` field, capped at 512 KiB.
///
/// # Errors
///
/// Returns a [`ToolError`] if the format is unknown, the journal cannot be
/// read or serialized, or the output file cannot be written.
pub fn report(
    journal_path: &str,
    format: &str,
    output_path: Option<&Path>,
) -> Result<StructuredReport, ToolError> {
    if format != "json" && format != "junit" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported report format '{format}'; valid values: json, junit \
             (html/pdf are only available via the tumult CLI)"
        )));
    }

    let journal = read_journal(Path::new(journal_path))
        .map_err(|e| ToolError::Parse(format!("failed to read journal: {e}")))?;

    let content = if format == "junit" {
        tumult_core::report::junit_report(&journal)
    } else {
        tumult_core::report::json_report(&journal)
            .map_err(|e| ToolError::Execution(e.to_string()))?
    };

    let mut structured = serde_json::Map::new();
    structured.insert("format".into(), serde_json::json!(format));

    if let Some(out) = output_path {
        std::fs::write(out, &content)
            .map_err(|e| ToolError::Execution(format!("failed to write report: {e}")))?;
        let out_str = out.display().to_string();
        structured.insert("output_path".into(), serde_json::json!(out_str));
        return Ok(StructuredReport {
            text: format!("Report generated: {out_str}"),
            structured,
        });
    }

    let capped = crate::tools::cap_text(content, "pass output_path to persist the full report");
    structured.insert("content".into(), serde_json::json!(capped));
    Ok(StructuredReport {
        text: capped,
        structured,
    })
}

/// Read every journal under `journals_path` (a `.toon` file or a directory
/// of them, non-recursive — CLI parity), invoking `on_journal` per journal.
/// Returns `(loaded, skipped)` counts; unreadable files are skipped.
pub(super) fn for_each_journal(
    journals_path: &str,
    mut on_journal: impl FnMut(Journal),
) -> Result<(usize, usize), ToolError> {
    let path = Path::new(journals_path);
    let mut loaded = 0usize;
    let mut skipped = 0usize;

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry_path = entry?.path();
            if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&entry_path) {
                    Ok(journal) => {
                        on_journal(journal);
                        loaded += 1;
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
    } else if path.is_file() {
        let journal = read_journal(path)
            .map_err(|e| ToolError::Parse(format!("failed to read journal: {e}")))?;
        on_journal(journal);
        loaded = 1;
    } else {
        return Err(ToolError::NotFound(format!(
            "path does not exist: {journals_path}"
        )));
    }

    Ok((loaded, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::write_run_journal;
    use tempfile::TempDir;

    #[test]
    fn report_rejects_unknown_format() {
        let err = report("journal.toon", "html", None).expect_err("html must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("html"), "must name the bad value: {msg}");
        assert!(
            msg.contains("json") && msg.contains("junit"),
            "must list valid values: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_json_inline_returns_journal_json() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = report(journal_path.to_str().unwrap(), "json", None).unwrap();
        assert_eq!(result.structured["format"], "json");
        let content = result.structured["content"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(parsed["experiment_title"], "MCP test experiment");
        assert_eq!(result.text, content);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_junit_writes_output_file() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());
        let out = dir.path().join("report.xml");

        let result = report(journal_path.to_str().unwrap(), "junit", Some(&out)).unwrap();
        assert!(out.exists(), "report file must be written");
        assert_eq!(
            result.structured["output_path"],
            out.display().to_string().as_str()
        );
        assert!(!result.structured.contains_key("content"));
        let xml = std::fs::read_to_string(&out).unwrap();
        assert!(xml.contains("<testsuite name=\"MCP test experiment\""));
    }
}
