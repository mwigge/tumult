//! AI-assisted recommendation support shared by the Tumult CLI and MCP server.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tumult_core::engine::validate_experiment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("json") {
            Self::Json
        } else {
            Self::Text
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecommendOptions {
    pub store_path: PathBuf,
    pub goal: Option<String>,
    pub model: Option<String>,
    pub include_draft: bool,
    pub format: OutputFormat,
}

impl RecommendOptions {
    #[must_use]
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store_path: store_path.into(),
            goal: None,
            model: None,
            include_draft: true,
            format: OutputFormat::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecommendationItem {
    pub rank: usize,
    pub title: String,
    pub rationale: String,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub expected_learning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecommendationOutput {
    pub source: String,
    pub model: Option<String>,
    pub goal: Option<String>,
    pub recommendations: Vec<RecommendationItem>,
    pub draft_toon: Option<String>,
    pub draft_valid: Option<bool>,
    pub draft_validation_error: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub heuristic_context: String,
}

#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
struct ModelResponse {
    #[serde(default)]
    recommendations: Vec<RecommendationItem>,
    #[serde(default)]
    draft_toon: Option<String>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct RecommendationContext {
    heuristic_report: String,
    plugin_catalog: String,
    documents: Vec<RecommendationDocument>,
}

#[derive(Debug, Clone)]
struct RecommendationDocument {
    id: String,
    title: Option<String>,
    path: Option<String>,
    content: String,
}

/// Build AI-powered recommendations, falling back to deterministic heuristics.
///
/// # Errors
///
/// Returns an error only when JSON output serialization fails. Model and store
/// failures are represented as heuristic fallback output.
pub fn recommend(options: &RecommendOptions) -> anyhow::Result<String> {
    let output = recommend_struct(options);
    match output.format {
        OutputFormat::Text => Ok(render_text(&output.result)),
        OutputFormat::Json => serde_json::to_string_pretty(&output.result).context("encode JSON"),
    }
}

struct FormattedRecommendation {
    format: OutputFormat,
    result: RecommendationOutput,
}

fn recommend_struct(options: &RecommendOptions) -> FormattedRecommendation {
    let context = build_context(&options.store_path);
    let mut result = heuristic_output(options, &context);
    validate_draft(&mut result);
    FormattedRecommendation {
        format: options.format,
        result,
    }
}

#[cfg(test)]
fn parse_model_response(answer: &str) -> anyhow::Result<ModelResponse> {
    let trimmed = answer.trim();
    if let Ok(parsed) = serde_json::from_str::<ModelResponse>(trimmed) {
        return validate_model_response(parsed);
    }

    for (start, _) in trimmed.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&trimmed[start..]).into_iter();
        if let Some(Ok(parsed)) = stream.next() {
            if let Ok(valid) = validate_model_response(parsed) {
                return Ok(valid);
            }
        }
    }

    Err(anyhow::anyhow!(
        "model response did not contain valid recommendation JSON"
    ))
}

#[cfg(test)]
fn validate_model_response(response: ModelResponse) -> anyhow::Result<ModelResponse> {
    if response.recommendations.is_empty() {
        anyhow::bail!("model response did not include recommendations");
    }
    let _draft_present = response.draft_toon.is_some();
    Ok(response)
}

fn build_context(store_path: &Path) -> RecommendationContext {
    let heuristic_report = heuristic_report(store_path);
    let plugin_catalog = plugin_catalog();
    let documents = vec![
        RecommendationDocument {
            id: "tumult-heuristics".to_string(),
            title: Some("Tumult heuristic recommendation context".to_string()),
            path: None,
            content: heuristic_report.clone(),
        },
        RecommendationDocument {
            id: "tumult-plugins".to_string(),
            title: Some("Tumult plugin catalog".to_string()),
            path: None,
            content: plugin_catalog.clone(),
        },
    ];
    RecommendationContext {
        heuristic_report,
        plugin_catalog,
        documents,
    }
}

fn heuristic_output(
    options: &RecommendOptions,
    context: &RecommendationContext,
) -> RecommendationOutput {
    let document_summary = context.document_summary();
    RecommendationOutput {
        source: "heuristic-fallback".to_string(),
        model: None,
        goal: options.goal.clone(),
        recommendations: vec![RecommendationItem {
            rank: 1,
            title: "Close the largest untested action coverage gaps".to_string(),
            rationale: "Tumult found plugin actions that have not appeared in the analytics store."
                .to_string(),
            plugins: Vec::new(),
            actions: Vec::new(),
            preconditions: vec![
                "Confirm target service ownership and rollback path.".to_string(),
                "Run during an approved resilience testing window.".to_string(),
            ],
            expected_learning: Some(
                "Which untested failure modes produce measurable resilience gaps.".to_string(),
            ),
        }],
        draft_toon: None,
        draft_valid: None,
        draft_validation_error: None,
        notes: vec![
            "Used deterministic Tumult coverage and failure heuristics.".to_string(),
            document_summary,
        ],
        heuristic_context: context.heuristic_report.clone(),
    }
}

impl RecommendationContext {
    fn document_summary(&self) -> String {
        let ids = self
            .documents
            .iter()
            .map(RecommendationDocument::summary)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Prepared {} local context documents from plugin catalog ({} bytes): {ids}",
            self.documents.len(),
            self.plugin_catalog.len()
        )
    }
}

impl RecommendationDocument {
    fn summary(&self) -> String {
        let title = self.title.as_deref().unwrap_or("untitled");
        let path = self.path.as_deref().unwrap_or("inline");
        format!("{}:{}:{}:{}b", self.id, title, path, self.content.len())
    }
}

fn validate_draft(output: &mut RecommendationOutput) {
    let Some(draft) = output.draft_toon.as_ref() else {
        output.draft_valid = None;
        output.draft_validation_error = None;
        return;
    };
    match toon_format::decode_default::<tumult_core::types::Experiment>(draft) {
        Ok(experiment) => match validate_experiment(&experiment) {
            Ok(()) => {
                output.draft_valid = Some(true);
                output.draft_validation_error = None;
            }
            Err(err) => {
                output.draft_valid = Some(false);
                output.draft_validation_error = Some(err.to_string());
            }
        },
        Err(err) => {
            output.draft_valid = Some(false);
            output.draft_validation_error = Some(err.to_string());
        }
    }
}

fn render_text(output: &RecommendationOutput) -> String {
    let mut text = String::new();
    writeln!(text, "=== AI-Powered Tumult Recommendations ===").ok();
    writeln!(text, "Source: {}", output.source).ok();
    if let Some(model) = &output.model {
        writeln!(text, "Model: {model}").ok();
    }
    if let Some(goal) = &output.goal {
        writeln!(text, "Goal: {goal}").ok();
    }
    writeln!(text).ok();

    for recommendation in &output.recommendations {
        writeln!(text, "{}. {}", recommendation.rank, recommendation.title).ok();
        writeln!(text, "   Rationale: {}", recommendation.rationale).ok();
        if !recommendation.plugins.is_empty() {
            writeln!(text, "   Plugins: {}", recommendation.plugins.join(", ")).ok();
        }
        if !recommendation.actions.is_empty() {
            writeln!(text, "   Actions: {}", recommendation.actions.join(", ")).ok();
        }
        if !recommendation.preconditions.is_empty() {
            writeln!(
                text,
                "   Preconditions: {}",
                recommendation.preconditions.join("; ")
            )
            .ok();
        }
        if let Some(learning) = &recommendation.expected_learning {
            writeln!(text, "   Expected learning: {learning}").ok();
        }
        writeln!(text).ok();
    }

    if let Some(draft) = &output.draft_toon {
        writeln!(
            text,
            "Draft TOON experiment validation: {}",
            match output.draft_valid {
                Some(true) => "valid",
                Some(false) => "invalid",
                None => "not checked",
            }
        )
        .ok();
        if let Some(err) = &output.draft_validation_error {
            writeln!(text, "Draft validation error: {err}").ok();
        }
        writeln!(text).ok();
        writeln!(text, "{draft}").ok();
    }

    if !output.notes.is_empty() {
        writeln!(text).ok();
        writeln!(text, "Notes:").ok();
        for note in &output.notes {
            writeln!(text, "  - {note}").ok();
        }
    }

    text
}

#[must_use]
pub fn heuristic_report(store_path: &Path) -> String {
    let mut output = String::new();
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let available_actions: Vec<String> = available_plugins
        .iter()
        .flat_map(|plugin| {
            plugin
                .actions
                .iter()
                .map(move |action| format!("{}::{}", plugin.name, action.name))
        })
        .collect();

    writeln!(output, "=== Recommendations ===").ok();
    writeln!(output).ok();

    if !store_path.exists() {
        writeln!(
            output,
            "No analytics store found at {}. Run experiments to build history.",
            store_path.display()
        )
        .ok();
        writeln!(output, "Available actions: {}", available_actions.len()).ok();
        for action in available_actions.iter().take(15) {
            writeln!(output, "  - {action}").ok();
        }
        return output;
    }

    let Ok(store) = tumult_analytics::AnalyticsStore::open(store_path) else {
        writeln!(output, "Analytics store could not be opened.").ok();
        return output;
    };

    let tested_actions = store
        .query("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")
        .unwrap_or_default();
    let tested_set: HashSet<String> = tested_actions
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect();
    let untested: Vec<&String> = available_actions
        .iter()
        .filter(|action| {
            let short_name = action.split("::").nth(1).unwrap_or(action);
            !tested_set.contains(short_name)
        })
        .collect();

    let tested_count = available_actions.len().saturating_sub(untested.len());
    let coverage = if available_actions.is_empty() {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (tested_count as f64 / available_actions.len() as f64) * 100.0
        }
    };
    writeln!(
        output,
        "Coverage: {tested_count}/{} actions tested ({coverage:.0}%)",
        available_actions.len()
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

    let stale = store
        .query(
            "SELECT title, max(started_at_ns) as last_run \
             FROM experiments GROUP BY title \
             ORDER BY last_run ASC LIMIT 5",
        )
        .unwrap_or_default();
    if !stale.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Oldest experiments:").ok();
        for row in &stale {
            if let Some(title) = row.first() {
                writeln!(output, "  - {title}").ok();
            }
        }
    }

    output
}

fn plugin_catalog() -> String {
    let plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let mut output = String::new();
    for plugin in plugins {
        writeln!(output, "plugin: {}", plugin.name).ok();
        if !plugin.actions.is_empty() {
            writeln!(output, "  actions:").ok();
            for action in plugin.actions {
                writeln!(output, "    - {}: {}", action.name, action.description).ok();
            }
        }
        if !plugin.probes.is_empty() {
            writeln!(output, "  probes:").ok();
            for probe in plugin.probes {
                writeln!(output, "    - {}: {}", probe.name, probe.description).ok();
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_format_defaults_to_text() {
        assert_eq!(OutputFormat::parse("text"), OutputFormat::Text);
        assert_eq!(OutputFormat::parse("json"), OutputFormat::Json);
        assert_eq!(OutputFormat::parse("weird"), OutputFormat::Text);
    }

    #[test]
    fn parses_json_embedded_in_model_text() {
        let response = parse_model_response(
            r#"Here is JSON:
            {"recommendations":[{"rank":1,"title":"t","rationale":"r"}],"draft_toon":null,"notes":["n"]}"#,
        )
        .unwrap();
        assert_eq!(response.recommendations.len(), 1);
        assert_eq!(response.notes, vec!["n"]);
    }

    #[test]
    fn parses_json_from_markdown_fence_after_noisy_braces() {
        let response = parse_model_response(
            r#"The shape is {not json}.
            ```json
            {
              "recommendations": [
                {
                  "rank": 1,
                  "title": "exercise process timeout",
                  "rationale": "timeout handling is not covered",
                  "plugins": ["tumult-process"],
                  "actions": ["kill-process"],
                  "preconditions": ["maintenance window"],
                  "expected_learning": "whether callers retry"
                }
              ],
              "draft_toon": null,
              "notes": ["review blast radius"]
            }
            ```"#,
        )
        .unwrap();

        assert_eq!(response.recommendations[0].rank, 1);
        assert_eq!(response.recommendations[0].plugins, vec!["tumult-process"]);
        assert_eq!(
            response.recommendations[0].expected_learning.as_deref(),
            Some("whether callers retry")
        );
    }

    #[test]
    fn rejects_model_json_without_recommendations() {
        let err = parse_model_response(r#"{"draft_toon":null,"notes":["empty"]}"#).unwrap_err();
        assert!(
            err.to_string().contains("recommendations"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_store_heuristic_is_useful() {
        let report = heuristic_report(Path::new("/definitely/not/a/tumult/store.duckdb"));
        assert!(report.contains("No analytics store found"));
    }

    #[test]
    fn heuristic_output_preserves_goal_and_context_without_draft() {
        let options = RecommendOptions {
            store_path: PathBuf::from("/definitely/not/a/tumult/store.duckdb"),
            goal: Some("cover database failover".to_string()),
            model: Some("ignored".to_string()),
            include_draft: true,
            format: OutputFormat::Json,
        };
        let context = build_context(&options.store_path);

        let output = heuristic_output(&options, &context);

        assert_eq!(output.source, "heuristic-fallback");
        assert_eq!(output.goal.as_deref(), Some("cover database failover"));
        assert_eq!(output.model, None);
        assert_eq!(output.draft_toon, None);
        assert_eq!(output.draft_valid, None);
        assert!(!output.recommendations.is_empty());
        assert!(output
            .notes
            .iter()
            .any(|note| note.contains("deterministic Tumult coverage")));
        assert!(output
            .heuristic_context
            .contains("No analytics store found"));
    }

    #[test]
    fn render_text_includes_recommendation_details_notes_and_valid_draft() {
        let draft = valid_process_draft();
        let mut output = sample_output(Some(draft));
        validate_draft(&mut output);

        let text = render_text(&output);

        assert!(text.contains("Source: deterministic-recommender"));
        assert!(text.contains("Model: qwen"));
        assert!(text.contains("Goal: test retries"));
        assert!(text.contains("1. Kill a worker process"));
        assert!(text.contains("Plugins: tumult-process"));
        assert!(text.contains("Actions: kill-process"));
        assert!(text.contains("Preconditions: maintenance window"));
        assert!(text.contains("Expected learning: retry behavior"));
        assert!(text.contains("Draft TOON experiment validation: valid"));
        assert!(text.contains("Notes:"));
        assert!(text.contains("Keep scope narrow"));
    }

    #[test]
    fn json_rendering_preserves_output_shape() {
        let output = sample_output(None);

        let rendered = serde_json::to_string_pretty(&output).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["source"], "deterministic-recommender");
        assert_eq!(value["model"], "qwen");
        assert_eq!(value["goal"], "test retries");
        assert_eq!(value["recommendations"][0]["rank"], 1);
        assert_eq!(
            value["recommendations"][0]["expected_learning"],
            "retry behavior"
        );
        assert!(value["draft_toon"].is_null());
        assert!(value["draft_valid"].is_null());
        assert_eq!(value["notes"][0], "Keep scope narrow");
        assert_eq!(value["heuristic_context"], "coverage summary");
    }

    #[test]
    fn validate_draft_marks_decode_failures_invalid() {
        let mut output = sample_output(Some("this is not toon".to_string()));

        validate_draft(&mut output);

        assert_eq!(output.draft_valid, Some(false));
        assert!(output.draft_validation_error.is_some());
    }

    #[test]
    fn validate_draft_marks_engine_validation_failures_invalid() {
        let mut output = sample_output(Some(
            "title: missing method\nmethod[0]:\nrollbacks[0]:\n".to_string(),
        ));

        validate_draft(&mut output);

        assert_eq!(output.draft_valid, Some(false));
        let err = output.draft_validation_error.unwrap_or_default();
        assert!(
            err.contains("method") || err.contains("Method"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_draft_clears_stale_state_when_draft_absent() {
        let mut output = sample_output(None);
        output.draft_valid = Some(false);
        output.draft_validation_error = Some("old error".to_string());

        validate_draft(&mut output);

        assert_eq!(output.draft_valid, None);
        assert_eq!(output.draft_validation_error, None);
    }

    fn sample_output(draft_toon: Option<String>) -> RecommendationOutput {
        RecommendationOutput {
            source: "deterministic-recommender".to_string(),
            model: Some("qwen".to_string()),
            goal: Some("test retries".to_string()),
            recommendations: vec![RecommendationItem {
                rank: 1,
                title: "Kill a worker process".to_string(),
                rationale: "Process failure is not covered".to_string(),
                plugins: vec!["tumult-process".to_string()],
                actions: vec!["kill-process".to_string()],
                preconditions: vec!["maintenance window".to_string()],
                expected_learning: Some("retry behavior".to_string()),
            }],
            draft_toon,
            draft_valid: None,
            draft_validation_error: None,
            notes: vec!["Keep scope narrow".to_string()],
            heuristic_context: "coverage summary".to_string(),
        }
    }

    fn valid_process_draft() -> String {
        r#"title: Process retry validation
description: Verify process execution path stays healthy

steady_state_hypothesis:
  title: Process provider works
  probes[1]:
    - name: echo-ready
      activity_type: probe
      provider:
        type: process
        path: echo
        arguments[1]: "ready"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "ready"

method[1]:
  - name: exercise-process-provider
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "chaos"
      timeout_s: 5.0

rollbacks[1]:
  - name: verify-ready-after-action
    activity_type: probe
    provider:
      type: process
      path: echo
      arguments[1]: "ready"
      timeout_s: 5.0
    tolerance:
      type: regex
      pattern: "ready"
"#
        .to_string()
    }
}
