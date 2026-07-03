use std::collections::BTreeMap;
use std::time::Duration;

use crate::model::{AgenticError, AgenticScenario};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponse {
    pub body: String,
    pub latency_ms: u64,
    pub tool_calls: u32,
    pub retry_count: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub fallback_used: bool,
}

pub trait AgentAdapter {
    /// # Errors
    ///
    /// Returns an error if the adapter cannot invoke the scenario, the target
    /// times out, validation fails, or the fake adapter is configured to fail.
    fn invoke(&self, scenario: &AgenticScenario) -> Result<AgentResponse, AgenticError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    pub traceparent: String,
}

#[must_use]
pub fn fixture_response(body: impl Into<String>) -> AgentResponse {
    AgentResponse {
        body: body.into(),
        latency_ms: 1,
        tool_calls: 0,
        retry_count: 0,
        input_tokens: 1,
        output_tokens: 1,
        fallback_used: false,
    }
}

#[must_use]
pub fn trace_headers(trace_context: &TraceContext) -> BTreeMap<String, String> {
    BTreeMap::from([("traceparent".to_string(), trace_context.traceparent.clone())])
}

pub(crate) fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
