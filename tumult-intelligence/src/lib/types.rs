//! Data types shared across the recommendation pipeline.

use serde::{Deserialize, Serialize};

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
    pub store_path: std::path::PathBuf,
    pub goal: Option<String>,
    pub model: Option<String>,
    pub include_draft: bool,
    pub format: OutputFormat,
}

impl RecommendOptions {
    #[must_use]
    pub fn new(store_path: impl Into<std::path::PathBuf>) -> Self {
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
pub(crate) struct ModelResponse {
    #[serde(default)]
    pub(crate) recommendations: Vec<RecommendationItem>,
    #[serde(default)]
    pub(crate) draft_toon: Option<String>,
    #[serde(default)]
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RecommendationContext {
    pub(crate) heuristic_report: String,
    pub(crate) plugin_catalog: String,
    pub(crate) documents: Vec<RecommendationDocument>,
}

#[derive(Debug, Clone)]
pub(crate) struct RecommendationDocument {
    pub(crate) id: String,
    pub(crate) title: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) content: String,
}

impl RecommendationContext {
    pub(crate) fn document_summary(&self) -> String {
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
    pub(crate) fn summary(&self) -> String {
        let title = self.title.as_deref().unwrap_or("untitled");
        let path = self.path.as_deref().unwrap_or("inline");
        format!("{}:{}:{}:{}b", self.id, title, path, self.content.len())
    }
}
