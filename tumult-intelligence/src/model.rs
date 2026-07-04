//! Parsing of raw model responses into structured recommendations.
//!
//! These helpers are exercised exclusively by the test suite today.

use crate::types::ModelResponse;

pub(crate) fn parse_model_response(answer: &str) -> anyhow::Result<ModelResponse> {
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

pub(crate) fn validate_model_response(response: ModelResponse) -> anyhow::Result<ModelResponse> {
    if response.recommendations.is_empty() {
        anyhow::bail!("model response did not include recommendations");
    }
    let _draft_present = response.draft_toon.is_some();
    Ok(response)
}
