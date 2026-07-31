//! OpenAI-compatible HTTP chat client for Tumult's analytics layer
//! (imported from `kronika-ai`; sibling module to [`crate::sql_guard`]).
//!
//! * [`Llm`] / [`OpenAiCompatClient`]: one OpenAI-compatible chat interface.
//!   Interactive paths can point at a direct API, LiteLLM or a local Ollama
//!   (the default); batch/digest workloads can delegate to smedja later.
//!
//! No live LLM calls happen anywhere in v1 — the client is only wired for
//! configuration and request serialisation.

// Imported from kronika (kronika-ai). Pedantic lints are scoped to
// tumult-native code; this module predates the pedantic gate.
#![allow(clippy::pedantic)]

use serde::{Deserialize, Serialize};

/// Errors from the LLM client.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("LLM response contained no choices")]
    EmptyResponse,

    #[error("{0}")]
    Config(String),
}

/// One chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// Chat roles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// The LLM interface. Implementations must be cheap to hold and share.
#[async_trait::async_trait]
pub trait Llm: Send + Sync {
    /// Send a chat conversation and return the assistant's reply text.
    async fn chat(&self, messages: &[Message]) -> Result<String, AiError>;
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Message,
}

/// An OpenAI-compatible chat-completions client (works against Ollama,
/// LiteLLM, vLLM, OpenAI itself, …).
///
/// Configuration from env:
/// * `KRONIKA_LLM_BASE_URL` (default `http://localhost:11434/v1` — Ollama)
/// * `KRONIKA_LLM_API_KEY` (default empty)
/// * `KRONIKA_LLM_MODEL` (default `qwen2.5:7b` — any local model tag)
pub struct OpenAiCompatClient {
    base_url: String,
    api_key: Option<String>,
    model: String,
    http: reqwest::Client,
}

impl OpenAiCompatClient {
    /// Build a client from `KRONIKA_LLM_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let base_url = std::env::var("KRONIKA_LLM_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:11434/v1".into());
        let api_key = std::env::var("KRONIKA_LLM_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        let model = std::env::var("KRONIKA_LLM_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());
        Self::new(base_url, api_key, model)
    }

    /// Build a client with explicit configuration.
    #[must_use]
    pub fn new(base_url: String, api_key: Option<String>, model: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            http: reqwest::Client::new(),
        }
    }

    /// The configured model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait::async_trait]
impl Llm for OpenAiCompatClient {
    async fn chat(&self, messages: &[Message]) -> Result<String, AiError> {
        let request = ChatRequest {
            model: &self.model,
            messages,
            stream: false,
        };
        let mut call = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&request);
        if let Some(key) = &self.api_key {
            call = call.bearer_auth(key);
        }
        let response: ChatResponse = call.send().await?.error_for_status()?.json().await?;
        response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or(AiError::EmptyResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_defaults_target_local_ollama() {
        std::env::remove_var("KRONIKA_LLM_BASE_URL");
        std::env::remove_var("KRONIKA_LLM_API_KEY");
        std::env::remove_var("KRONIKA_LLM_MODEL");
        let client = OpenAiCompatClient::from_env();
        assert_eq!(client.base_url, "http://localhost:11434/v1");
        assert_eq!(client.api_key, None);
        assert_eq!(client.model(), "qwen2.5:7b");
    }

    #[test]
    fn messages_serialise_openai_style() {
        let messages = vec![
            Message {
                role: Role::System,
                content: "You translate questions to SQL.".into(),
            },
            Message {
                role: Role::User,
                content: "What is the pass rate?".into(),
            },
        ];
        let json = serde_json::to_value(&messages).unwrap();
        assert_eq!(json[0]["role"], "system");
        assert_eq!(json[1]["role"], "user");
    }
}
