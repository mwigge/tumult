//! Top-level recommendation entry point and draft validation.

use anyhow::Context as _;
use tumult_core::engine::validate_experiment;

use crate::context::{build_context, heuristic_output};
use crate::render::render_text;
use crate::types::{OutputFormat, RecommendOptions, RecommendationOutput};

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
