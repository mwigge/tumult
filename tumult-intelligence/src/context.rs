//! Assembly of the heuristic recommendation context and its fallback output.

use std::path::Path;

use crate::report::{heuristic_report, plugin_catalog};
use crate::types::{
    RecommendOptions, RecommendationContext, RecommendationDocument, RecommendationItem,
    RecommendationOutput,
};

pub(crate) fn build_context(store_path: &Path) -> RecommendationContext {
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

pub(crate) fn heuristic_output(
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
