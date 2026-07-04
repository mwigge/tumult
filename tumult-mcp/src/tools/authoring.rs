//! Authoring tools: fault catalog and experiment scaffolding.
//!
//! These wrap the shared [`tumult_authoring`] crate so the MCP surface and the
//! CLI (`tumult new` / `tumult templates`) build experiments through exactly
//! the same code path. Both tools are read-only with respect to the store —
//! they generate content and never mutate persistent state.

use indexmap::IndexMap;

use tumult_authoring::builder::{
    build_experiment_unvalidated, encode_experiment, ProbeSpec, ScaffoldRequest,
};

use crate::error::ToolError;
use crate::tools::{cap_text, StructuredReport};

/// Return the live fault catalog (domains → actions → args) as structured
/// content.
///
/// # Errors
///
/// Returns [`ToolError::Execution`] if plugin discovery fails or the catalog
/// cannot be serialized.
pub fn fault_catalog() -> Result<StructuredReport, ToolError> {
    let catalog =
        tumult_authoring::build_catalog().map_err(|e| ToolError::Execution(e.to_string()))?;

    let mut structured = serde_json::Map::new();
    structured.insert(
        "action_count".into(),
        serde_json::json!(catalog.action_count()),
    );
    structured.insert(
        "domains".into(),
        serde_json::to_value(&catalog.domains).map_err(|e| ToolError::Execution(e.to_string()))?,
    );

    let text = serde_json::to_string_pretty(&serde_json::Value::Object(structured.clone()))
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    Ok(StructuredReport {
        text: cap_text(text, "full catalog in structuredContent"),
        structured,
    })
}

/// Inputs for [`scaffold_experiment`], mirroring the tool's argument schema.
pub struct ScaffoldArgs<'a> {
    /// Owning plugin (e.g. `tumult-network`). Optional when `action` is
    /// fully-qualified as `plugin::action`.
    pub plugin: Option<&'a str>,
    /// Action name, or `plugin::action`.
    pub action: &'a str,
    /// Argument values as a JSON object (numbers/booleans are stringified,
    /// then re-coerced by the builder).
    pub args: &'a serde_json::Map<String, serde_json::Value>,
    /// Logical target of the fault.
    pub target: &'a str,
    /// Shell command for the steady-state probe (mutually exclusive with
    /// `probe_url`; falls back to a default health check when both are absent).
    pub probe_command: Option<&'a str>,
    /// HTTP URL for the steady-state probe (health check via `curl`).
    pub probe_url: Option<&'a str>,
    /// Regex the probe output/response must match.
    pub probe_expect: Option<&'a str>,
    /// Experiment title (defaults to `<action> — <target>`).
    pub title: Option<&'a str>,
}

/// Scaffold an experiment from a chosen action and return the generated TOON
/// plus whether it validates, as structured content.
///
/// The structured object is `{action, toon, valid, validation_error?}`. The
/// text content is the generated TOON. Read-only w.r.t. the store.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if neither `plugin` nor a
/// `plugin::action` form is provided, or [`ToolError::Execution`] if TOON
/// encoding fails.
pub fn scaffold_experiment(args: &ScaffoldArgs<'_>) -> Result<StructuredReport, ToolError> {
    let (plugin, action) = match args.plugin {
        Some(p) => (p.to_string(), args.action.to_string()),
        None => match args.action.split_once("::") {
            Some((p, a)) => (p.to_string(), a.to_string()),
            None => {
                return Err(ToolError::InvalidInput(
                    "provide `plugin`, or a fully-qualified `action` as plugin::action".into(),
                ))
            }
        },
    };

    let mut scaffold_args: IndexMap<String, String> = IndexMap::new();
    for (k, v) in args.args {
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        scaffold_args.insert(k.clone(), value);
    }

    let probe = if let Some(url) = args.probe_url {
        ProbeSpec::Http {
            url: url.to_string(),
            expect: args.probe_expect.unwrap_or("").to_string(),
        }
    } else if let Some(command) = args.probe_command {
        ProbeSpec::Exec {
            command: command.to_string(),
            expect: args.probe_expect.unwrap_or(".").to_string(),
        }
    } else {
        ProbeSpec::default_for(args.target)
    };

    let title = args
        .title
        .map_or_else(|| format!("{action} — {}", args.target), str::to_string);

    let qualified = format!("{plugin}::{action}");
    let request = ScaffoldRequest {
        title,
        plugin,
        action,
        args: scaffold_args,
        target: args.target.to_string(),
        probe,
    };

    let experiment = build_experiment_unvalidated(&request);
    let validity = tumult_core::engine::validate_experiment(&experiment);
    let toon = encode_experiment(&experiment).map_err(|e| ToolError::Execution(e.to_string()))?;

    let mut structured = serde_json::Map::new();
    structured.insert("action".into(), serde_json::Value::String(qualified));
    structured.insert("toon".into(), serde_json::Value::String(toon.clone()));
    structured.insert("valid".into(), serde_json::Value::Bool(validity.is_ok()));
    if let Err(e) = &validity {
        structured.insert(
            "validation_error".into(),
            serde_json::Value::String(e.to_string()),
        );
    }

    Ok(StructuredReport {
        text: cap_text(toon, "generated experiment in structuredContent.toon"),
        structured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_produces_valid_toon() {
        let mut args = serde_json::Map::new();
        args.insert("delay_ms".into(), serde_json::json!(100));
        let report = scaffold_experiment(&ScaffoldArgs {
            plugin: Some("tumult-network"),
            action: "add-latency",
            args: &args,
            target: "checkout",
            probe_command: None,
            probe_url: None,
            probe_expect: None,
            title: None,
        })
        .unwrap();
        assert_eq!(report.structured["valid"], serde_json::json!(true));
        assert!(report.structured["toon"]
            .as_str()
            .unwrap()
            .contains("checkout"));
        assert_eq!(
            report.structured["action"],
            serde_json::json!("tumult-network::add-latency")
        );
    }

    #[test]
    fn scaffold_accepts_qualified_action() {
        let args = serde_json::Map::new();
        let report = scaffold_experiment(&ScaffoldArgs {
            plugin: None,
            action: "tumult-db-redis::flush-all",
            args: &args,
            target: "cache",
            probe_command: None,
            probe_url: None,
            probe_expect: None,
            title: Some("redis flush"),
        })
        .unwrap();
        assert_eq!(report.structured["valid"], serde_json::json!(true));
    }

    #[test]
    fn scaffold_reports_invalid_regex_probe() {
        let args = serde_json::Map::new();
        let report = scaffold_experiment(&ScaffoldArgs {
            plugin: Some("tumult-network"),
            action: "add-latency",
            args: &args,
            target: "x",
            probe_command: Some("echo hi"),
            probe_url: None,
            probe_expect: Some("(unclosed"),
            title: None,
        })
        .unwrap();
        assert_eq!(report.structured["valid"], serde_json::json!(false));
        assert!(report.structured.contains_key("validation_error"));
    }

    #[test]
    fn scaffold_requires_plugin_or_qualified_action() {
        let args = serde_json::Map::new();
        let err = scaffold_experiment(&ScaffoldArgs {
            plugin: None,
            action: "add-latency",
            args: &args,
            target: "x",
            probe_command: None,
            probe_url: None,
            probe_expect: None,
            title: None,
        });
        assert!(err.is_err());
    }
}
