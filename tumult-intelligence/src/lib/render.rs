//! Human-readable rendering of recommendation output.

use std::fmt::Write as _;

use crate::types::RecommendationOutput;

pub(crate) fn render_text(output: &RecommendationOutput) -> String {
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
