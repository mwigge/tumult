use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::model::{AgenticError, AgenticScenario, AgenticTarget, PrivacyConfig};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    pub traceparent: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterSmokeExpectation {
    pub adapter: String,
    pub scenario: String,
    pub fault: String,
    pub contract: String,
    pub expected: String,
    pub actual: String,
    pub next_diagnostic_command: String,
}

impl AdapterSmokeExpectation {
    #[must_use]
    pub fn failure_message(&self) -> String {
        format!(
            "adapter={} scenario={} fault={} contract={} expected={} actual={} next_diagnostic_command={}",
            self.adapter,
            self.scenario,
            self.fault,
            self.contract,
            self.expected,
            self.actual,
            self.next_diagnostic_command
        )
    }
}

#[must_use]
pub fn adapter_failure_expectation(
    adapter: impl Into<String>,
    scenario: impl Into<String>,
    fault: impl Into<String>,
    contract: impl Into<String>,
    expected: impl Into<String>,
    actual: impl Into<String>,
    next_diagnostic_command: impl Into<String>,
) -> AdapterSmokeExpectation {
    AdapterSmokeExpectation {
        adapter: adapter.into(),
        scenario: scenario.into(),
        fault: fault.into(),
        contract: contract.into(),
        expected: expected.into(),
        actual: actual.into(),
        next_diagnostic_command: next_diagnostic_command.into(),
    }
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
pub fn target_type(target: &AgenticTarget) -> &'static str {
    match target {
        AgenticTarget::Http { .. } => "http",
        AgenticTarget::Mcp { .. } => "mcp",
        AgenticTarget::Replay { .. } => "replay",
    }
}

/// # Errors
///
/// Returns [`AgenticError::TargetNotAllowed`] when the target endpoint,
/// server, or fixture is not covered by the configured allowlist.
pub fn validate_target(
    target: &AgenticTarget,
    privacy: &PrivacyConfig,
) -> Result<(), AgenticError> {
    let value = match target {
        AgenticTarget::Http { endpoint } => endpoint,
        AgenticTarget::Mcp { server, .. } => server,
        AgenticTarget::Replay { fixture } => fixture,
    };

    if privacy.target_allowlist.is_empty()
        || privacy
            .target_allowlist
            .iter()
            .any(|allowed| value.starts_with(allowed))
    {
        return Ok(());
    }

    Err(AgenticError::TargetNotAllowed(value.clone()))
}

#[must_use]
pub fn trace_headers(trace_context: &TraceContext) -> BTreeMap<String, String> {
    BTreeMap::from([("traceparent".to_string(), trace_context.traceparent.clone())])
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
