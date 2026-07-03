use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::model::{AgenticError, AgenticScenario};

use super::core::{elapsed_millis, AgentResponse, TraceContext};

#[derive(Debug, Clone)]
pub struct HttpAgentAdapter {
    endpoint: String,
    timeout: Duration,
    allowlist: Vec<String>,
    trace_context: Option<TraceContext>,
}

#[derive(Debug, Clone, Serialize)]
struct HttpAgentRequest<'a> {
    scenario: &'a str,
    input_length: usize,
    expected_behavior: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
struct HttpAgentWireResponse {
    body: String,
    #[serde(default)]
    latency_ms: u64,
    #[serde(default)]
    tool_calls: u32,
    #[serde(default)]
    retry_count: u32,
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    fallback_used: bool,
}

impl HttpAgentAdapter {
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(2),
            allowlist: Vec::new(),
            trace_context: None,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.allowlist = allowlist;
        self
    }

    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    /// Invokes a local or allowlisted HTTP agent endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`AgenticError`] when the endpoint is not allowlisted, times out,
    /// returns a non-success status, or returns an invalid adapter response.
    pub async fn invoke(&self, scenario: &AgenticScenario) -> Result<AgentResponse, AgenticError> {
        self.validate_endpoint()?;

        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|err| {
                AgenticError::Adapter(format!("adapter=http error=client_build cause={err}"))
            })?;
        let request = HttpAgentRequest {
            scenario: &scenario.name,
            input_length: scenario.input.len(),
            expected_behavior: scenario.expected_behavior.as_deref(),
        };
        let mut builder = client.post(&self.endpoint).json(&request);
        if let Some(trace_context) = &self.trace_context {
            builder = builder.header("traceparent", trace_context.traceparent.as_str());
        }

        let started = Instant::now();
        let response = builder.send().await.map_err(|err| {
            if err.is_timeout() {
                AgenticError::Adapter(format!(
                    "adapter=http scenario={} error=timeout timeout_ms={}",
                    scenario.name,
                    self.timeout.as_millis()
                ))
            } else {
                AgenticError::Adapter(format!(
                    "adapter=http scenario={} error=request cause={err}",
                    scenario.name
                ))
            }
        })?;

        if !response.status().is_success() {
            return Err(AgenticError::Adapter(format!(
                "adapter=http scenario={} error=status status={}",
                scenario.name,
                response.status()
            )));
        }

        let elapsed_ms = elapsed_millis(started.elapsed());
        let wire = response
            .json::<HttpAgentWireResponse>()
            .await
            .map_err(|err| {
                AgenticError::Adapter(format!(
                    "adapter=http scenario={} error=decode cause={err}",
                    scenario.name
                ))
            })?;

        Ok(AgentResponse {
            body: wire.body,
            latency_ms: wire.latency_ms.max(elapsed_ms),
            tool_calls: wire.tool_calls,
            retry_count: wire.retry_count,
            input_tokens: wire.input_tokens,
            output_tokens: wire.output_tokens,
            fallback_used: wire.fallback_used,
        })
    }

    fn validate_endpoint(&self) -> Result<(), AgenticError> {
        if self.allowlist.is_empty()
            || self
                .allowlist
                .iter()
                .any(|allowed| self.endpoint.starts_with(allowed))
        {
            Ok(())
        } else {
            Err(AgenticError::TargetNotAllowed(self.endpoint.clone()))
        }
    }
}
