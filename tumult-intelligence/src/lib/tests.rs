//! Test suite for the intelligence recommendation pipeline.

use std::path::{Path, PathBuf};

use crate::context::{build_context, heuristic_output};
use crate::model::parse_model_response;
use crate::recommend::validate_draft;
use crate::render::render_text;
use crate::report::heuristic_report;
use crate::types::{OutputFormat, RecommendOptions, RecommendationItem, RecommendationOutput};

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
