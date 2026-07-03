use std::collections::BTreeMap;
use std::time::Duration;

use crate::model::{AgenticError, AgenticScenario};

use super::core::{elapsed_millis, trace_headers, AgentAdapter, AgentResponse, TraceContext};

#[derive(Debug, Clone)]
pub struct FakeHttpAgentAdapter {
    name: String,
    response: AgentResponse,
    delay: Duration,
    timeout: Duration,
    failure: Option<String>,
    trace_context: Option<TraceContext>,
}

impl FakeHttpAgentAdapter {
    #[must_use]
    pub fn new(name: impl Into<String>, response: AgentResponse) -> Self {
        Self {
            name: name.into(),
            response,
            delay: Duration::ZERO,
            timeout: Duration::from_secs(2),
            failure: None,
            trace_context: None,
        }
    }

    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_failure(mut self, failure: impl Into<String>) -> Self {
        self.failure = Some(failure.into());
        self
    }

    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    #[must_use]
    pub fn trace_headers(&self) -> BTreeMap<String, String> {
        self.trace_context
            .as_ref()
            .map(trace_headers)
            .unwrap_or_default()
    }
}

impl AgentAdapter for FakeHttpAgentAdapter {
    fn invoke(&self, scenario: &AgenticScenario) -> Result<AgentResponse, AgenticError> {
        if self.delay > self.timeout {
            return Err(AgenticError::Adapter(format!(
                "adapter=fake_http name={} scenario={} error=timeout timeout_ms={}",
                self.name,
                scenario.name,
                self.timeout.as_millis()
            )));
        }

        if let Some(failure) = &self.failure {
            return Err(AgenticError::Adapter(format!(
                "adapter=fake_http name={} scenario={} error=failure failure={failure}",
                self.name, scenario.name
            )));
        }

        let mut response = self.response.clone();
        response.latency_ms = response
            .latency_ms
            .saturating_add(elapsed_millis(self.delay));
        Ok(response)
    }
}
