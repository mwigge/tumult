//! Experiment builder — turn a chosen fault action into a validated
//! [`Experiment`] and its TOON serialization.
//!
//! The builder is the single source of truth shared by the CLI `tumult new`
//! picker and the `tumult_scaffold_experiment` MCP tool. It constructs a
//! minimal, valid experiment: a steady-state hypothesis with one probe, a
//! method with the chosen action, and a rollback when the plugin has a
//! complementary undo action.

use std::collections::HashMap;

use indexmap::IndexMap;

use tumult_core::engine::validate_experiment;
use tumult_core::types::{Activity, ActivityType, Experiment, Hypothesis, Provider, Tolerance};

use crate::catalog::domain_for;

/// Errors raised while building or serializing an experiment.
#[derive(thiserror::Error, Debug)]
pub enum AuthoringError {
    /// The requested curated template does not exist.
    #[error("unknown template: {0}")]
    UnknownTemplate(String),
    /// A required argument was not supplied.
    #[error("missing required argument: {0}")]
    MissingArg(String),
    /// The core engine rejected the generated experiment.
    #[error("generated experiment failed validation: {0}")]
    Validation(String),
    /// TOON encoding failed.
    #[error("failed to encode experiment as TOON: {0}")]
    Encode(String),
    /// Template variable substitution failed.
    #[error("template instantiation failed: {0}")]
    Instantiate(String),
    /// A `--set key=value` override was malformed.
    #[error("invalid override (expected key=value): {0}")]
    BadOverride(String),
}

/// A steady-state probe specification. Both variants compile to a `process`
/// probe with a regex tolerance — the runnable shape used throughout the
/// example experiments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeSpec {
    /// Run a shell command; the probe passes when its output matches `expect`.
    Exec {
        /// Shell command line (run via `sh -c`).
        command: String,
        /// Regex the output must match for the steady state to hold.
        expect: String,
    },
    /// HTTP health check via `curl`; passes when the response matches `expect`.
    Http {
        /// URL to GET.
        url: String,
        /// Regex the response must match (default `.` — any response).
        expect: String,
    },
}

impl ProbeSpec {
    /// A sensible default probe for `target`: a shell health check that simply
    /// confirms the host shell is responsive. Callers that know a real health
    /// endpoint should prefer [`ProbeSpec::Http`] or a tailored
    /// [`ProbeSpec::Exec`].
    #[must_use]
    pub fn default_for(target: &str) -> Self {
        Self::Exec {
            command: format!("echo {} steady-state ok", shell_quote(target)),
            expect: "steady-state ok".to_string(),
        }
    }

    /// Convert to the underlying shell command and expected-output regex.
    fn command_and_expect(&self) -> (String, String) {
        match self {
            Self::Exec { command, expect } => (command.clone(), expect.clone()),
            Self::Http { url, expect } => (
                format!("curl -fsS {}", shell_quote(url)),
                if expect.is_empty() {
                    ".".to_string()
                } else {
                    expect.clone()
                },
            ),
        }
    }

    fn to_activity(&self, name: &str) -> Activity {
        let (command, expect) = self.command_and_expect();
        Activity {
            name: name.to_string(),
            activity_type: ActivityType::Probe,
            provider: Provider::Process {
                path: "sh".to_string(),
                arguments: vec!["-c".to_string(), command],
                env: HashMap::new(),
                timeout_s: Some(10.0),
            },
            tolerance: Some(Tolerance::Regex { pattern: expect }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }
}

/// A request to scaffold a validated experiment from a catalog action.
#[derive(Debug, Clone)]
pub struct ScaffoldRequest {
    /// Experiment title.
    pub title: String,
    /// Owning plugin, e.g. `tumult-network`.
    pub plugin: String,
    /// Action/probe name, e.g. `add-latency`.
    pub action: String,
    /// Argument values (name → value). Numeric/boolean strings are coerced.
    pub args: IndexMap<String, String>,
    /// Logical target of the fault (host, container, service, …). Recorded in
    /// the method arguments as `target` and woven into the default title.
    pub target: String,
    /// Steady-state probe.
    pub probe: ProbeSpec,
}

/// Single-quote a value for safe interpolation into a `sh -c` command line:
/// wrapped in single quotes, with embedded quotes escaped as `'\''`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Native (compiled-in Rust) plugin names — mirrors the CLI's composition-root
/// registry (`tumult-cli`'s `exec::native`). Every other plugin name is a
/// script plugin, dispatched through the discovery search paths at runtime.
const NATIVE_PLUGINS: [&str; 5] = [
    "tumult-kubernetes",
    "tumult-ssh",
    "tumult-net",
    "tumult-cloud",
    "tumult-windows",
];

/// The provider for a scaffolded step: `native` for the compiled-in plugins,
/// `script` for everything else. Emitting `native` for a script plugin
/// produces an experiment that passes validation but fails at runtime with
/// `unknown native plugin` — the native registry holds no script plugins.
fn provider_for(
    plugin: &str,
    function: &str,
    arguments: HashMap<String, serde_json::Value>,
) -> Provider {
    if NATIVE_PLUGINS.contains(&plugin) {
        Provider::Native {
            plugin: plugin.to_string(),
            function: function.to_string(),
            arguments,
        }
    } else {
        Provider::Script {
            plugin: plugin.to_string(),
            function: function.to_string(),
            arguments,
            timeout_s: None,
        }
    }
}

/// Build a validated [`Experiment`] from a scaffold request.
///
/// The result always contains a steady-state hypothesis with one probe, a
/// single method step referencing `plugin::action` through the matching
/// provider (native or script), and a rollback step when [`rollback_action`]
/// knows the plugin's undo action.
///
/// # Errors
///
/// Returns [`AuthoringError::Validation`] if the generated experiment does not
/// pass `validate_experiment` (e.g. a probe with a malformed regex).
pub fn build_experiment(request: &ScaffoldRequest) -> Result<Experiment, AuthoringError> {
    let experiment = build_experiment_unvalidated(request);
    validate_experiment(&experiment).map_err(|e| AuthoringError::Validation(e.to_string()))?;
    Ok(experiment)
}

/// Build an [`Experiment`] from a scaffold request without validating it.
///
/// The method step references `plugin::action` through the provider matching
/// the plugin: `native` for the compiled-in plugins, `script` for the
/// discovered script plugins (and any custom plugin on the search paths).
///
/// Callers that want to report validity themselves (e.g. the
/// `tumult_scaffold_experiment` MCP tool, which returns the TOON alongside a
/// `valid` flag) use this and run [`validate_experiment`] separately.
#[must_use]
pub fn build_experiment_unvalidated(request: &ScaffoldRequest) -> Experiment {
    let domain = domain_for(&request.plugin);

    // Method arguments: the supplied args plus the logical target.
    let mut arguments: HashMap<String, serde_json::Value> = request
        .args
        .iter()
        .map(|(k, v)| (k.clone(), coerce_value(v)))
        .collect();
    arguments
        .entry("target".to_string())
        .or_insert_with(|| serde_json::Value::String(request.target.clone()));

    let method_step = Activity {
        name: request.action.clone(),
        activity_type: ActivityType::Action,
        provider: provider_for(&request.plugin, &request.action, arguments),
        tolerance: None,
        pause_before_s: None,
        pause_after_s: Some(3.0),
        background: false,
        label_selector: None,
    };

    let rollbacks = rollback_action(&request.plugin, &request.action)
        .map(|undo| {
            let mut undo_args: HashMap<String, serde_json::Value> = HashMap::new();
            undo_args.insert(
                "target".to_string(),
                serde_json::Value::String(request.target.clone()),
            );
            vec![Activity {
                name: undo.to_string(),
                activity_type: ActivityType::Action,
                provider: provider_for(&request.plugin, undo, undo_args),
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }]
        })
        .unwrap_or_default();

    Experiment {
        version: "v1".to_string(),
        title: request.title.clone(),
        description: Some(format!(
            "Scaffolded from {}::{} against {}",
            request.plugin, request.action, request.target
        )),
        tags: vec![domain.tag().to_string(), "resilience".to_string()],
        steady_state_hypothesis: Some(Hypothesis {
            title: format!("{} is healthy", request.target),
            probes: vec![request.probe.to_activity("steady-state-check")],
        }),
        method: vec![method_step],
        rollbacks,
        ..Default::default()
    }
}

/// Build a validated experiment and serialize it to TOON.
///
/// # Errors
///
/// Returns [`AuthoringError::Validation`] if validation fails or
/// [`AuthoringError::Encode`] if TOON encoding fails.
pub fn build_experiment_toon(request: &ScaffoldRequest) -> Result<String, AuthoringError> {
    let experiment = build_experiment(request)?;
    encode_experiment(&experiment)
}

/// Serialize an already-built experiment to TOON.
///
/// # Errors
///
/// Returns [`AuthoringError::Encode`] if TOON encoding fails.
pub fn encode_experiment(experiment: &Experiment) -> Result<String, AuthoringError> {
    toon_format::encode_default(experiment).map_err(|e| AuthoringError::Encode(e.to_string()))
}

/// The complementary undo action for a `plugin::action`, if one exists. Used
/// to auto-populate the rollback step of a scaffolded experiment.
#[must_use]
pub fn rollback_action(plugin: &str, action: &str) -> Option<&'static str> {
    match (plugin, action) {
        ("tumult-network", "add-latency" | "add-packet-loss" | "add-corruption") => {
            Some("reset-tc")
        }
        ("tumult-network", "block-dns" | "redirect-dns") => Some("block-dns-rollback"),
        ("tumult-containers", "pause-container") => Some("unpause-container"),
        ("tumult-containers", "stop-container") => Some("start-container"),
        ("tumult-timewarp", "skew-clock" | "advance-clock-past-cert-expiry") => {
            Some("restore-clock")
        }
        ("tumult-timewarp", "entropy-drain") => Some("stop-entropy-drain"),
        _ => None,
    }
}

/// Coerce a string argument to a JSON value: integers and floats become
/// numbers, `true`/`false` become booleans, everything else stays a string.
fn coerce_value(raw: &str) -> serde_json::Value {
    if let Ok(i) = raw.parse::<i64>() {
        return serde_json::Value::Number(i.into());
    }
    if let Ok(f) = raw.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return serde_json::Value::Number(n);
        }
    }
    match raw {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        other => serde_json::Value::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> ScaffoldRequest {
        let mut args = IndexMap::new();
        args.insert("delay_ms".to_string(), "100".to_string());
        ScaffoldRequest {
            title: "Net latency scaffold".to_string(),
            plugin: "tumult-network".to_string(),
            action: "add-latency".to_string(),
            args,
            target: "checkout-api".to_string(),
            probe: ProbeSpec::default_for("checkout-api"),
        }
    }

    #[test]
    fn build_experiment_is_valid() {
        let exp = build_experiment(&sample_request()).unwrap();
        assert_eq!(exp.version, "v1");
        assert_eq!(exp.method.len(), 1);
        assert!(exp.steady_state_hypothesis.is_some());
        // add-latency has a curated rollback.
        assert_eq!(exp.rollbacks.len(), 1);
        assert_eq!(exp.rollbacks[0].name, "reset-tc");
    }

    #[test]
    fn numeric_args_are_coerced() {
        let exp = build_experiment(&sample_request()).unwrap();
        let Provider::Script { arguments, .. } = &exp.method[0].provider else {
            panic!("expected script provider");
        };
        assert_eq!(arguments["delay_ms"], serde_json::json!(100));
        assert_eq!(arguments["target"], serde_json::json!("checkout-api"));
    }

    #[test]
    fn script_plugins_get_script_providers() {
        let exp = build_experiment(&sample_request()).unwrap();
        let Provider::Script {
            plugin, function, ..
        } = &exp.method[0].provider
        else {
            panic!("expected script provider, got {:?}", exp.method[0].provider);
        };
        assert_eq!(plugin, "tumult-network");
        assert_eq!(function, "add-latency");
        // The curated rollback dispatches through the same provider kind.
        let Provider::Script { function, .. } = &exp.rollbacks[0].provider else {
            panic!(
                "expected script rollback, got {:?}",
                exp.rollbacks[0].provider
            );
        };
        assert_eq!(function, "reset-tc");
    }

    #[test]
    fn native_plugins_keep_native_providers() {
        let mut req = sample_request();
        req.plugin = "tumult-kubernetes".to_string();
        req.action = "delete_pod".to_string();
        let exp = build_experiment(&req).unwrap();
        let Provider::Native {
            plugin, function, ..
        } = &exp.method[0].provider
        else {
            panic!("expected native provider, got {:?}", exp.method[0].provider);
        };
        assert_eq!(plugin, "tumult-kubernetes");
        assert_eq!(function, "delete_pod");
    }

    #[test]
    fn scaffolded_toon_uses_script_provider_tag() {
        let toon = build_experiment_toon(&sample_request()).unwrap();
        assert!(toon.contains("type: script"), "{toon}");
        let parsed = tumult_core::engine::parse_experiment(&toon).unwrap();
        assert!(matches!(parsed.method[0].provider, Provider::Script { .. }));
    }

    #[test]
    fn toon_round_trips_through_parse_and_validate() {
        let toon = build_experiment_toon(&sample_request()).unwrap();
        let parsed = tumult_core::engine::parse_experiment(&toon).unwrap();
        assert!(validate_experiment(&parsed).is_ok());
        assert_eq!(parsed.title, "Net latency scaffold");
    }

    #[test]
    fn action_without_rollback_has_none() {
        let mut req = sample_request();
        req.plugin = "tumult-db-redis".to_string();
        req.action = "flush-all".to_string();
        let exp = build_experiment(&req).unwrap();
        assert!(exp.rollbacks.is_empty());
    }

    #[test]
    fn http_probe_builds_curl_command() {
        let mut req = sample_request();
        req.probe = ProbeSpec::Http {
            url: "http://localhost:8080/health".to_string(),
            expect: "ok".to_string(),
        };
        let exp = build_experiment(&req).unwrap();
        let probe = &exp.steady_state_hypothesis.unwrap().probes[0];
        let Provider::Process { arguments, .. } = &probe.provider else {
            panic!("expected process provider");
        };
        assert!(arguments[1].contains("curl -fsS 'http://localhost:8080/health'"));
    }

    #[test]
    fn interpolated_probe_values_are_shell_quoted() {
        // A URL carrying shell metacharacters must not break out of its
        // single-quoted slot in the generated `sh -c` command.
        let mut req = sample_request();
        req.probe = ProbeSpec::Http {
            url: "http://x/'; rm -rf / #".to_string(),
            expect: "ok".to_string(),
        };
        let exp = build_experiment(&req).unwrap();
        let probe = &exp.steady_state_hypothesis.unwrap().probes[0];
        let Provider::Process { arguments, .. } = &probe.provider else {
            panic!("expected process provider");
        };
        assert_eq!(
            arguments[1], "curl -fsS 'http://x/'\\''; rm -rf / #'",
            "url must be single-quoted with embedded quotes escaped: {}",
            arguments[1]
        );
    }

    #[test]
    fn default_probe_quotes_target() {
        // Double quotes pass through verbatim inside single quotes; an
        // embedded single quote is escaped as '\''.
        let probe = ProbeSpec::default_for("web'; rm -rf /; '");
        let ProbeSpec::Exec { command, .. } = probe else {
            panic!("expected exec probe");
        };
        assert_eq!(command, "echo 'web'\\''; rm -rf /; '\\''' steady-state ok");

        let probe = ProbeSpec::default_for("web\"; touch /tmp/pwned; \"");
        let ProbeSpec::Exec { command, .. } = probe else {
            panic!("expected exec probe");
        };
        assert_eq!(
            command,
            "echo 'web\"; touch /tmp/pwned; \"' steady-state ok"
        );
    }
}
