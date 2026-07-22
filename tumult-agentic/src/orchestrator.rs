//! Orchestrator mode: tumult drives a non-interactive agent as the trace root.
//!
//! tumult starts a `tumult.experiment` root span, mints a `traceparent` from it,
//! and runs the agent (e.g. `claude -p`) with that trace context plus telemetry
//! export and a base URL pointing at the fault-injecting proxy. The agent's own
//! spans then nest under tumult's experiment, and tumult evaluates its contracts
//! against the agent's response.
//!
//! The agent invocation is behind the [`AgentRunner`] trait (dependency
//! inversion) so the live subprocess can be stubbed in tests.

use crate::adapters::AgentResponse;
use crate::contracts::{evaluate_contract, ContractSpec};
use crate::model::{AgenticError, AgenticRunResult, ContractOutcome};
use crate::scoring::resilience_score;

/// Runs a non-interactive agent and returns its response text.
pub trait AgentRunner {
    /// Run the agent with `env` set and `prompt` supplied.
    ///
    /// # Errors
    ///
    /// Returns [`AgenticError::Adapter`] if the agent cannot be run or fails.
    fn run(&self, env: &[(String, String)], prompt: &str) -> Result<String, AgenticError>;
}

/// Runs an agent by spawning a command (e.g. `claude -p --output-format json`).
#[derive(Debug, Clone)]
pub struct CommandAgentRunner {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandAgentRunner {
    /// A runner for the Claude Code CLI in non-interactive JSON mode.
    #[must_use]
    pub fn claude() -> Self {
        Self {
            program: "claude".to_string(),
            args: vec![
                "-p".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ],
        }
    }
}

impl AgentRunner for CommandAgentRunner {
    fn run(&self, env: &[(String, String)], prompt: &str) -> Result<String, AgenticError> {
        use std::io::Write as _;

        let mut child = std::process::Command::new(&self.program)
            .args(&self.args)
            .envs(env.iter().map(|(key, value)| (key.clone(), value.clone())))
            // The prompt goes via stdin, never argv: command lines are
            // visible in `ps` to every local user, stdin is not. The agent
            // CLIs (`claude -p`, `codex`) read the prompt from stdin when no
            // positional prompt argument is given.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| AgenticError::Adapter(format!("spawn {}: {err}", self.program)))?;

        // Feed stdin from a separate thread so a child that never reads it
        // (or a prompt larger than the pipe buffer) cannot deadlock us.
        let mut stdin = child.stdin.take().ok_or_else(|| {
            AgenticError::Adapter(format!("{}: stdin pipe unavailable", self.program))
        })?;
        let prompt = prompt.to_string();
        let feeder = std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));

        let output = child
            .wait_with_output()
            .map_err(|err| AgenticError::Adapter(format!("wait on {}: {err}", self.program)))?;
        // A broken pipe just means the child exited before reading; the exit
        // status below carries the real outcome.
        let _ = feeder.join();
        if !output.status.success() {
            return Err(AgenticError::Adapter(format!(
                "{} exited with {}",
                self.program, output.status
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Build the environment for the agent subprocess: W3C trace context, telemetry
/// export, and the base URL pointing at the tumult proxy.
#[must_use]
pub fn agent_env(
    traceparent: &str,
    otlp_endpoint: Option<&str>,
    base_url: &str,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("TRACEPARENT".to_string(), traceparent.to_string()),
        (
            "CLAUDE_CODE_PROPAGATE_TRACEPARENT".to_string(),
            "1".to_string(),
        ),
        ("CLAUDE_CODE_ENABLE_TELEMETRY".to_string(), "1".to_string()),
        (
            "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA".to_string(),
            "1".to_string(),
        ),
        ("ANTHROPIC_BASE_URL".to_string(), base_url.to_string()),
    ];
    if let Some(endpoint) = otlp_endpoint {
        env.push(("OTEL_TRACES_EXPORTER".to_string(), "otlp".to_string()));
        env.push((
            "OTEL_EXPORTER_OTLP_ENDPOINT".to_string(),
            endpoint.to_string(),
        ));
    }
    env
}

/// Evaluate `contracts` against the agent's response body.
#[must_use]
pub fn evaluate_response(
    scenario: &str,
    contracts: &[ContractSpec],
    response_body: &str,
) -> Vec<ContractOutcome> {
    let response = AgentResponse {
        body: response_body.to_string(),
        latency_ms: 0,
        tool_calls: 0,
        retry_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        fallback_used: false,
    };
    contracts
        .iter()
        .map(|contract| evaluate_contract(scenario, contract, &response))
        .collect()
}

/// Inputs for an orchestrated live run.
#[derive(Debug, Clone)]
pub struct LiveRun<'a> {
    pub scenario: &'a str,
    pub client: &'a str,
    pub contracts: &'a [ContractSpec],
    pub prompt: &'a str,
    pub otlp_endpoint: Option<&'a str>,
    pub base_url: &'a str,
}

/// Orchestrate a live agent run with tumult as the trace root.
///
/// Starts a `tumult.experiment` root span, mints a `traceparent` for it, runs
/// the agent via `runner` (with telemetry + proxy base URL), evaluates the
/// pack's contracts against the agent's response, and emits the experiment span
/// nested under the root.
///
/// # Errors
///
/// Propagates [`AgenticError`] from the agent runner.
pub fn run_live(runner: &dyn AgentRunner, run: &LiveRun) -> Result<AgenticRunResult, AgenticError> {
    let root = tumult_otel::agentic_span::start_experiment_root(run.scenario, run.client);
    let traceparent =
        tumult_otel::propagation::current_traceparent(root.context()).unwrap_or_default();
    let env = agent_env(&traceparent, run.otlp_endpoint, run.base_url);

    let response = runner.run(&env, run.prompt)?;
    let outcomes = evaluate_response(run.scenario, run.contracts, &response);

    let result = AgenticRunResult {
        target_type: "live".to_string(),
        scenarios: vec![run.scenario.to_string()],
        faults: Vec::new(),
        resilience_score: resilience_score(&outcomes),
        contracts: outcomes,
        trace_id: None,
        replay_id: None,
    };

    emit_live_telemetry(run, &result, root.context());
    root.end();
    Ok(result)
}

fn emit_live_telemetry(run: &LiveRun, result: &AgenticRunResult, parent: &opentelemetry::Context) {
    use tumult_otel::agentic_span::{record_agentic_run, AgenticRunTelemetry, ContractRecord};
    let contracts: Vec<ContractRecord> = result
        .contracts
        .iter()
        .map(|contract| ContractRecord {
            contract_type: contract.contract_type.clone(),
            passed: contract.passed,
            reason: contract.reason.clone(),
            severity: contract.severity,
        })
        .collect();
    record_agentic_run(
        &AgenticRunTelemetry {
            scenario: run.scenario,
            target_type: "live",
            client: Some(run.client),
            resilience_score: result.resilience_score,
            faults: &[],
            contracts: &contracts,
        },
        Some(parent),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct StubRunner {
        response: String,
        captured_env: RefCell<Vec<(String, String)>>,
    }

    impl AgentRunner for StubRunner {
        fn run(&self, env: &[(String, String)], _prompt: &str) -> Result<String, AgenticError> {
            *self.captured_env.borrow_mut() = env.to_vec();
            Ok(self.response.clone())
        }
    }

    fn valid_json_contract() -> Vec<ContractSpec> {
        vec![ContractSpec::ValidJson {
            severity: Some(1.0),
        }]
    }

    #[test]
    fn agent_env_carries_trace_telemetry_and_base_url() {
        let env = agent_env(
            "00-aa-bb-01",
            Some("http://collector:4317"),
            "http://127.0.0.1:8080",
        );
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map["TRACEPARENT"], "00-aa-bb-01");
        assert_eq!(map["CLAUDE_CODE_PROPAGATE_TRACEPARENT"], "1");
        assert_eq!(map["ANTHROPIC_BASE_URL"], "http://127.0.0.1:8080");
        assert_eq!(map["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://collector:4317");
    }

    #[test]
    fn agent_env_omits_otlp_when_unset() {
        let env = agent_env("tp", None, "http://x");
        assert!(!env
            .iter()
            .any(|(key, _)| key == "OTEL_EXPORTER_OTLP_ENDPOINT"));
    }

    #[test]
    fn evaluate_response_flags_invalid_json() {
        let outcomes = evaluate_response("s", &valid_json_contract(), "{not json");
        assert!(!outcomes[0].passed);
        assert_eq!(outcomes[0].reason.as_deref(), Some("invalid_json"));
    }

    /// The prompt must travel via stdin: an argv element is visible in `ps`
    /// to other local users. With `sh -c SCRIPT`, a positional prompt would
    /// land in `$0`; the fixed code leaves `$0` as the shell name and pipes
    /// the prompt instead.
    #[cfg(unix)]
    #[test]
    fn command_runner_passes_prompt_via_stdin_not_argv() {
        let runner = CommandAgentRunner {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "if [ \"$0\" = secret-prompt ]; then exit 3; fi; cat".to_string(),
            ],
        };
        let out = runner
            .run(&[], "secret-prompt")
            .expect("prompt on stdin must be read and echoed");
        assert_eq!(out, "secret-prompt");
    }

    #[test]
    fn run_live_evaluates_contracts_and_wires_env() {
        let runner = StubRunner {
            response: r#"{"ok":true}"#.to_string(),
            captured_env: RefCell::new(Vec::new()),
        };
        let contracts = valid_json_contract();
        let run = LiveRun {
            scenario: "scenario",
            client: "claude-code",
            contracts: &contracts,
            prompt: "do a thing",
            otlp_endpoint: Some("http://collector:4317"),
            base_url: "http://127.0.0.1:8080",
        };
        let result = run_live(&runner, &run).expect("run_live");

        assert_eq!(result.contracts.len(), 1);
        assert!(result.contracts[0].passed, "valid json passes the contract");
        assert!(runner
            .captured_env
            .borrow()
            .iter()
            .any(|(key, value)| key == "ANTHROPIC_BASE_URL" && value == "http://127.0.0.1:8080"));
    }

    #[test]
    fn run_live_nests_experiment_under_tumult_experiment_root() {
        use opentelemetry::global;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        global::set_tracer_provider(provider.clone());

        let runner = StubRunner {
            response: r#"{"ok":true}"#.to_string(),
            captured_env: RefCell::new(Vec::new()),
        };
        let contracts = valid_json_contract();
        let run = LiveRun {
            scenario: "live-scenario",
            client: "claude-code",
            contracts: &contracts,
            prompt: "p",
            otlp_endpoint: None,
            base_url: "http://127.0.0.1:8080",
        };
        let _ = run_live(&runner, &run).expect("run_live");
        provider.force_flush().ok();

        let spans = exporter.get_finished_spans().expect("spans");
        let roots: Vec<_> = spans
            .iter()
            .filter(|span| span.name == tumult_otel::agentic_span::EXPERIMENT_ROOT_SPAN)
            .map(|span| span.span_context.span_id())
            .collect();
        let nested = spans
            .iter()
            .filter(|span| span.name == tumult_otel::agentic_span::EXPERIMENT_SPAN)
            .any(|span| roots.contains(&span.parent_span_id));
        assert!(
            nested,
            "the experiment span must nest under a tumult.experiment root span"
        );
    }
}
