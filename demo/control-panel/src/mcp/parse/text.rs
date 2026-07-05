//! Parsers for tools whose results are read from text content: tools/list,
//! run, discover, validate, analyze_store, and recommend.

use serde_json::Value;

use crate::mcp::protocol::McpError;

use super::outcomes::{
    DiscoverOutcome, RecommendOutcome, Recommendation, RunOutcome, TableOutcome, ToolInfo,
    ValidateOutcome,
};
use super::{content_text, labeled_count, number_before};

/// Parse a `tools/list` result into [`ToolInfo`]s, reading each tool's
/// `annotations` for the destructive/read-only hints.
#[must_use]
pub fn parse_tools_list(result: &Value) -> Vec<ToolInfo> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|t| {
                    let ann = t.get("annotations");
                    ToolInfo {
                        name: t
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        destructive: ann
                            .and_then(|a| a.get("destructiveHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        read_only: ann
                            .and_then(|a| a.get("readOnlyHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `tumult_run_experiment` `tools/call` result into a [`RunOutcome`].
///
/// Reads `structuredContent.journal.{status,duration_ms}`,
/// `structuredContent.journal_path`, and `structuredContent.ingestion`.
///
/// # Errors
/// Returns [`McpError::Protocol`] when the tool reported `isError: true` (the
/// error text is lifted from the `content` array) or when no journal status can
/// be found.
pub fn parse_run_result(result: &Value) -> Result<RunOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "experiment tool reported an error".to_string(),
        )));
    }

    let sc = result
        .get("structuredContent")
        .ok_or_else(|| McpError::Protocol("run result missing structuredContent".to_string()))?;
    let journal = sc.get("journal").unwrap_or(sc);

    let status = journal
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("journal missing status".to_string()))?
        .to_string();

    let duration_ms = journal.get("duration_ms").and_then(Value::as_u64);
    let journal_path = sc
        .get("journal_path")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let ingestion = sc
        .get("ingestion")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(RunOutcome {
        outcome: verdict_for(&status).to_string(),
        status,
        duration_ms,
        journal_path,
        ingestion,
    })
}

/// Map a raw journal status to the panel's verdict. `halted` (auto-halt guard)
/// gets its own verdict so the UI can badge it distinctly from an outright
/// failure.
#[must_use]
pub fn verdict_for(status: &str) -> &'static str {
    match status {
        "completed" => "passed",
        "deviated" => "deviated",
        "halted" => "halted",
        _ => "failed",
    }
}

/// Parse a `tumult_discover` `tools/call` result into a [`DiscoverOutcome`].
///
/// Discover advertises no structured schema, so we read the text content and
/// pull the `Plugins: N` / `Actions: M` header counts.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or when the counts
/// cannot be located.
pub fn parse_discover_result(result: &Value) -> Result<DiscoverOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "discover tool reported an error".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("discover result had no text content".to_string()))?;
    let plugins = labeled_count(&text, "Plugins:")
        .ok_or_else(|| McpError::Protocol("discover output missing plugin count".to_string()))?;
    let actions = labeled_count(&text, "Actions:")
        .ok_or_else(|| McpError::Protocol("discover output missing action count".to_string()))?;
    Ok(DiscoverOutcome { plugins, actions })
}

/// Parse a `tumult_validate` `tools/call` result into a [`ValidateOutcome`].
///
/// A failed validation surfaces as `isError: true`; we lift its text into
/// [`McpError::Protocol`] so the loop marks the step failed.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_validate_result(result: &Value) -> Result<ValidateOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "experiment failed validation".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("validate result had no text content".to_string()))?;
    let trimmed = text.trim();
    let valid = trimmed.starts_with("Valid");
    let title = trimmed
        .split_once('\'')
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(t, _)| t.to_string());
    let method_steps = number_before(trimmed, "method step").unwrap_or(0);
    let rollbacks = number_before(trimmed, "rollback").unwrap_or(0);
    Ok(ValidateOutcome {
        valid,
        title,
        method_steps,
        rollbacks,
        summary: trimmed.to_string(),
    })
}

/// Parse a `tumult_analyze_store` `tools/call` result into a [`TableOutcome`].
///
/// The tool returns tab-separated text: a header row, one row per record, then
/// a trailing `N row(s)` line.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_analyze_store_result(result: &Value) -> Result<TableOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "analyze_store tool reported an error".to_string(),
        )));
    }
    let text = content_text(result).ok_or_else(|| {
        McpError::Protocol("analyze_store result had no text content".to_string())
    })?;

    let mut lines = text.lines();
    let columns: Vec<String> = lines
        .next()
        .map(|h| h.split('\t').map(str::to_string).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for line in lines {
        // The trailing "N row(s)" summary line is not a data row.
        if line.trim_end().ends_with("row(s)") {
            continue;
        }
        if line.is_empty() {
            continue;
        }
        rows.push(line.split('\t').map(str::to_string).collect::<Vec<_>>());
    }
    let row_count = rows.len();
    Ok(TableOutcome {
        columns,
        rows,
        row_count,
    })
}

/// Parse a `tumult_recommend` `tools/call` result into a [`RecommendOutcome`].
///
/// Reads `structuredContent`: either a `message` (no store yet) or a
/// `recommendations` array of `{rank, title, rationale}`. Falls back to the
/// text content when no structured content is present.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error.
pub fn parse_recommend_result(result: &Value) -> Result<RecommendOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "recommend tool reported an error".to_string()),
        ));
    }
    if let Some(sc) = result.get("structuredContent") {
        if let Some(msg) = sc.get("message").and_then(Value::as_str) {
            return Ok(RecommendOutcome {
                message: Some(msg.to_string()),
                recommendations: Vec::new(),
            });
        }
        let recommendations = sc
            .get("recommendations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|r| Recommendation {
                        rank: r.get("rank").and_then(Value::as_i64).unwrap_or_default(),
                        title: r
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        rationale: r
                            .get("rationale")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(RecommendOutcome {
            message: None,
            recommendations,
        });
    }
    // No structured content — fall back to the raw text summary.
    Ok(RecommendOutcome {
        message: content_text(result),
        recommendations: Vec::new(),
    })
}
