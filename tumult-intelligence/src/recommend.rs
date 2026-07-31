//! Top-level recommendation entry point and draft validation.

use tumult_core::engine::validate_experiment;

use crate::context::{build_context, heuristic_output};
use crate::error::RecommendError;
use crate::render::render_text;
use crate::types::{OutputFormat, RecommendOptions, RecommendationOutput};

/// Build AI-powered recommendations, falling back to deterministic heuristics.
///
/// # Errors
///
/// Returns an error only when JSON output serialization fails. Model and store
/// failures are represented as heuristic fallback output.
pub fn recommend(options: &RecommendOptions) -> Result<String, RecommendError> {
    render(&recommend_output(options), options.format)
}

/// Build the structured heuristic recommendation output (draft validated).
///
/// This is the same pipeline as [`recommend`] without rendering, for callers
/// that post-process the output — e.g. the CLI's agent enhancement flow.
#[must_use]
pub fn recommend_output(options: &RecommendOptions) -> RecommendationOutput {
    let context = build_context(&options.store_path);
    let mut result = heuristic_output(options, &context);
    validate_draft(&mut result);
    result
}

/// Render a recommendation output as text or pretty JSON.
///
/// # Errors
///
/// Returns an error only when JSON output serialization fails.
pub fn render(
    output: &RecommendationOutput,
    format: OutputFormat,
) -> Result<String, RecommendError> {
    match format {
        OutputFormat::Text => Ok(render_text(output)),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(output)?),
    }
}

pub(crate) fn validate_draft(output: &mut RecommendationOutput) {
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
