mod core;
mod fake_http;
mod http;
mod mcp;
mod smoke;

pub use core::{fixture_response, trace_headers, AgentAdapter, AgentResponse, TraceContext};
pub use fake_http::FakeHttpAgentAdapter;
pub use http::HttpAgentAdapter;
pub use mcp::{FakeMcpAdapter, McpToolInvocation};
pub use smoke::{
    adapter_failure_expectation, target_type, validate_target, AdapterSmokeExpectation,
};
