use std::time::Duration;

use crate::model::{AgenticError, AgenticScenario};

use super::core::{AgentAdapter, AgentResponse, TraceContext};

#[derive(Debug, Clone)]
pub struct McpToolInvocation {
    pub input: serde_json::Value,
    pub required_fields: Vec<String>,
    pub trace_context: Option<TraceContext>,
}

#[derive(Debug, Clone)]
pub struct FakeMcpAdapter {
    server: String,
    tool: String,
    timeout: Duration,
    response: AgentResponse,
    delay: Duration,
    failure: Option<String>,
}

impl FakeMcpAdapter {
    #[must_use]
    pub fn new(
        server: impl Into<String>,
        tool: impl Into<String>,
        response: AgentResponse,
    ) -> Self {
        Self {
            server: server.into(),
            tool: tool.into(),
            timeout: Duration::from_secs(2),
            response,
            delay: Duration::ZERO,
            failure: None,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    #[must_use]
    pub fn with_failure(mut self, failure: impl Into<String>) -> Self {
        self.failure = Some(failure.into());
        self
    }

    /// Invokes a deterministic fake MCP tool with schema-like field checks.
    ///
    /// # Errors
    ///
    /// Returns [`AgenticError`] when required tool input is missing, the fake
    /// target is configured to fail, or the simulated delay exceeds the timeout.
    pub fn invoke_tool(
        &self,
        invocation: &McpToolInvocation,
    ) -> Result<AgentResponse, AgenticError> {
        let Some(input) = invocation.input.as_object() else {
            return Err(AgenticError::Adapter(format!(
                "adapter=mcp server={} tool={} error=invalid_input expected=object",
                self.server, self.tool
            )));
        };

        for field in &invocation.required_fields {
            if !input.contains_key(field) {
                return Err(AgenticError::Adapter(format!(
                    "adapter=mcp server={} tool={} error=missing_required_field field={field}",
                    self.server, self.tool
                )));
            }
        }

        if self.delay > self.timeout {
            return Err(AgenticError::Adapter(format!(
                "adapter=mcp server={} tool={} error=timeout timeout_ms={}",
                self.server,
                self.tool,
                self.timeout.as_millis()
            )));
        }

        if let Some(failure) = &self.failure {
            return Err(AgenticError::Adapter(format!(
                "adapter=mcp server={} tool={} error=tool_failure failure={failure}",
                self.server, self.tool
            )));
        }

        Ok(self.response.clone())
    }
}

impl AgentAdapter for FakeMcpAdapter {
    fn invoke(&self, scenario: &AgenticScenario) -> Result<AgentResponse, AgenticError> {
        let input = serde_json::json!({
            "scenario": scenario.name,
            "input_length": scenario.input.len()
        });
        let invocation = McpToolInvocation {
            input,
            required_fields: vec!["scenario".to_string()],
            trace_context: None,
        };
        self.invoke_tool(&invocation)
    }
}
