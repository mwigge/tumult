//! Catalog-checked experiment scaffolding — the shared orchestration behind
//! the REST endpoint (`POST /api/authoring/scaffold`) and the MCP
//! `tumult_scaffold_experiment` tool.
//!
//! Both surfaces run exactly one code path: resolve the action against the
//! live catalog (rejecting probe-kind entries — a catalog probe used as the
//! method's action would still validate and could register a semantically
//! wrong experiment), assemble the [`ScaffoldRequest`], and report the
//! generated TOON alongside its engine-validity as
//! `{action, toon, valid, validation_error?}`.

use serde_json::{json, Map, Value};

use crate::builder::{
    build_experiment_unvalidated, encode_experiment, AuthoringError, ProbeSpec, ScaffoldRequest,
};
use crate::catalog::{ActionKind, FaultCatalog};

/// Why scaffolding was rejected.
#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    /// Neither `plugin` nor a fully-qualified `plugin::action` was given.
    #[error("provide `plugin`, or a fully-qualified `action` as plugin::action")]
    UnqualifiedAction,
    /// The action is not in the catalog — including catalog *probes*, which
    /// are rejected exactly like unknown names (see the module docs).
    #[error("unknown action {0:?}")]
    UnknownAction(String),
    /// TOON encoding failed.
    #[error("{0}")]
    Encode(#[from] AuthoringError),
}

/// Inputs for [`scaffold_from_catalog`], mirroring the REST request body
/// and the MCP tool's argument schema.
pub struct ScaffoldInput<'a> {
    /// Owning plugin (e.g. `tumult-network`). Optional when `action` is
    /// fully qualified as `plugin::action`.
    pub plugin: Option<&'a str>,
    /// Action name, or `plugin::action`.
    pub action: &'a str,
    /// Argument values (numbers/booleans are stringified, then re-coerced
    /// by the builder).
    pub args: &'a Map<String, Value>,
    /// Logical target of the fault.
    pub target: &'a str,
    /// Shell command for the steady-state probe (mutually exclusive with
    /// `probe_url`; a default health check is used when both are absent).
    pub probe_command: Option<&'a str>,
    /// HTTP URL for the steady-state probe.
    pub probe_url: Option<&'a str>,
    /// Regex the probe output/response must match.
    pub probe_expect: Option<&'a str>,
    /// Experiment title (defaults to `<action> — <target>`).
    pub title: Option<&'a str>,
}

/// The scaffolded experiment: the generated TOON plus whether it passes
/// `tumult_core::engine::validate_experiment`.
#[derive(Debug)]
pub struct ScaffoldOutcome {
    /// Fully-qualified `plugin::action`.
    pub qualified: String,
    /// The experiment as TOON.
    pub toon: String,
    /// Whether the generated experiment validates.
    pub valid: bool,
    /// The validation error, when `valid` is false.
    pub validation_error: Option<String>,
}

impl ScaffoldOutcome {
    /// The shared response shape both surfaces return:
    /// `{action, toon, valid, validation_error?}`.
    #[must_use]
    pub fn to_json(&self) -> Map<String, Value> {
        let mut body = Map::new();
        body.insert("action".into(), json!(self.qualified));
        body.insert("toon".into(), json!(self.toon));
        body.insert("valid".into(), json!(self.valid));
        if let Some(error) = &self.validation_error {
            body.insert("validation_error".into(), json!(error));
        }
        body
    }
}

/// Scaffold an experiment from a catalog action. The action must exist in
/// `catalog` and be a fault action ([`ActionKind::Action`]); probe-kind
/// entries are rejected like unknown names.
///
/// # Errors
///
/// Returns [`ScaffoldError::UnqualifiedAction`] when neither `plugin` nor a
/// `plugin::action` form is given, [`ScaffoldError::UnknownAction`] when
/// the action is not a fault action in the catalog, and
/// [`ScaffoldError::Encode`] when TOON encoding fails.
pub fn scaffold_from_catalog(
    catalog: &FaultCatalog,
    input: &ScaffoldInput<'_>,
) -> Result<ScaffoldOutcome, ScaffoldError> {
    let (plugin, action) = match input.plugin {
        Some(p) => (p.to_string(), input.action.to_string()),
        None => match input.action.split_once("::") {
            Some((p, a)) => (p.to_string(), a.to_string()),
            None => return Err(ScaffoldError::UnqualifiedAction),
        },
    };

    let qualified = format!("{plugin}::{action}");
    let is_action = catalog
        .find(&plugin, &action)
        .is_some_and(|a| a.kind == ActionKind::Action);
    if !is_action {
        return Err(ScaffoldError::UnknownAction(qualified));
    }

    let mut args = indexmap::IndexMap::new();
    for (k, v) in input.args {
        let value = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        args.insert(k.clone(), value);
    }

    let probe = if let Some(url) = input.probe_url {
        ProbeSpec::Http {
            url: url.to_string(),
            expect: input.probe_expect.unwrap_or_default().to_string(),
        }
    } else if let Some(command) = input.probe_command {
        ProbeSpec::Exec {
            command: command.to_string(),
            expect: input.probe_expect.unwrap_or(".").to_string(),
        }
    } else {
        ProbeSpec::default_for(input.target)
    };

    let title = input
        .title
        .map_or_else(|| format!("{action} — {}", input.target), str::to_string);

    let request = ScaffoldRequest {
        title,
        plugin,
        action,
        args,
        target: input.target.to_string(),
        probe,
    };
    let experiment = build_experiment_unvalidated(&request);
    let validity = tumult_core::engine::validate_experiment(&experiment);
    let toon = encode_experiment(&experiment)?;

    Ok(ScaffoldOutcome {
        qualified,
        toon,
        valid: validity.is_ok(),
        validation_error: validity.err().map(|e| e.to_string()),
    })
}

/// The catalog as a JSON object (`{action_count, domains}`) — the shared
/// shape of the REST catalog endpoint and the MCP catalog tool's
/// structured content.
#[must_use]
pub fn catalog_summary(catalog: &FaultCatalog) -> Map<String, Value> {
    let mut summary = Map::new();
    summary.insert("action_count".into(), json!(catalog.action_count()));
    summary.insert("domains".into(), json!(catalog.domains));
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogAction, CatalogArg, CatalogDomain, Domain};

    /// A minimal catalog with one fault action and one probe.
    fn test_catalog() -> FaultCatalog {
        let action = |name: &str, kind: ActionKind| CatalogAction {
            plugin: "tumult-network".into(),
            name: name.into(),
            description: String::new(),
            kind,
            args: vec![CatalogArg {
                name: "delay_ms".into(),
                required: true,
                description: String::new(),
            }],
        };
        FaultCatalog {
            domains: vec![CatalogDomain {
                domain: Domain::Network,
                label: "Network".into(),
                actions: vec![
                    action("add-latency", ActionKind::Action),
                    action("ping-latency", ActionKind::Probe),
                ],
            }],
        }
    }

    fn input<'a>(action: &'a str, args: &'a Map<String, Value>) -> ScaffoldInput<'a> {
        ScaffoldInput {
            plugin: Some("tumult-network"),
            action,
            args,
            target: "checkout",
            probe_command: None,
            probe_url: None,
            probe_expect: None,
            title: None,
        }
    }

    #[test]
    fn probe_kind_actions_are_rejected_like_unknown_names() {
        // Safety rule: a catalog probe used as the method's action would
        // still validate and could register a semantically wrong experiment.
        let catalog = test_catalog();
        let args = Map::new();
        let err = scaffold_from_catalog(&catalog, &input("ping-latency", &args)).unwrap_err();
        assert!(
            matches!(&err, ScaffoldError::UnknownAction(q) if q == "tumult-network::ping-latency"),
            "expected UnknownAction, got {err:?}"
        );
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn unknown_actions_are_rejected() {
        let catalog = test_catalog();
        let args = Map::new();
        let err = scaffold_from_catalog(&catalog, &input("nuke-everything", &args)).unwrap_err();
        assert!(matches!(err, ScaffoldError::UnknownAction(_)));
    }

    #[test]
    fn unqualified_action_without_plugin_is_rejected() {
        let catalog = test_catalog();
        let args = Map::new();
        let mut input = input("add-latency", &args);
        input.plugin = None;
        let err = scaffold_from_catalog(&catalog, &input).unwrap_err();
        assert!(matches!(err, ScaffoldError::UnqualifiedAction));
        assert!(err.to_string().contains("plugin::action"));
    }

    #[test]
    fn qualified_action_resolves_without_plugin_field() {
        let catalog = test_catalog();
        let args = Map::new();
        let mut input = input("tumult-network::add-latency", &args);
        input.plugin = None;
        let outcome = scaffold_from_catalog(&catalog, &input).unwrap();
        assert_eq!(outcome.qualified, "tumult-network::add-latency");
        assert!(outcome.valid);
        let json = outcome.to_json();
        assert_eq!(json["action"], json!("tumult-network::add-latency"));
        assert!(json["toon"].as_str().unwrap().contains("checkout"));
        assert!(!json.contains_key("validation_error"));
    }

    #[test]
    fn invalid_experiment_reports_validity_without_erroring() {
        let catalog = test_catalog();
        let args = Map::new();
        let mut input = input("add-latency", &args);
        input.probe_command = Some("echo hi");
        input.probe_expect = Some("(unclosed");
        let outcome = scaffold_from_catalog(&catalog, &input).unwrap();
        assert!(!outcome.valid);
        assert!(outcome.validation_error.is_some());
        assert!(outcome.to_json().contains_key("validation_error"));
    }

    #[test]
    fn catalog_summary_shape() {
        let summary = catalog_summary(&test_catalog());
        assert_eq!(summary["action_count"], json!(2));
        assert_eq!(summary["domains"].as_array().unwrap().len(), 1);
    }
}
