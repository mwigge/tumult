//! Authoring tools: fault catalog and experiment scaffolding.
//!
//! These wrap the shared [`tumult_authoring`] crate so the MCP surface, the
//! REST authoring endpoints and the CLI (`tumult new` / `tumult templates`)
//! build experiments through exactly the same code path. Both tools are
//! read-only with respect to the store — they generate content and never
//! mutate persistent state.

use tumult_authoring::scaffold::{ScaffoldError, ScaffoldInput};

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

    let structured = tumult_authoring::catalog_summary(&catalog);
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

/// Scaffold an experiment from a chosen catalog action and return the
/// generated TOON plus whether it validates, as structured content.
///
/// The structured object is `{action, toon, valid, validation_error?}`. The
/// text content is the generated TOON. Read-only w.r.t. the store.
///
/// The action is resolved against the live fault catalog through
/// [`tumult_authoring::scaffold_from_catalog`] — the same code path as the
/// REST authoring endpoint — so an action not in the catalog, including a
/// catalog *probe*, is rejected (a probe used as the method's action would
/// still validate and could register a semantically wrong experiment).
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if neither `plugin` nor a
/// `plugin::action` form is provided or the action is not a fault action in
/// the catalog, or [`ToolError::Execution`] if plugin discovery or TOON
/// encoding fails.
pub fn scaffold_experiment(args: &ScaffoldArgs<'_>) -> Result<StructuredReport, ToolError> {
    let catalog =
        tumult_authoring::build_catalog().map_err(|e| ToolError::Execution(e.to_string()))?;
    let outcome = tumult_authoring::scaffold_from_catalog(
        &catalog,
        &ScaffoldInput {
            plugin: args.plugin,
            action: args.action,
            args: args.args,
            target: args.target,
            probe_command: args.probe_command,
            probe_url: args.probe_url,
            probe_expect: args.probe_expect,
            title: args.title,
        },
    )
    .map_err(|e| match e {
        ScaffoldError::UnqualifiedAction | ScaffoldError::UnknownAction(_) => {
            ToolError::InvalidInput(e.to_string())
        }
        ScaffoldError::Encode(inner) => ToolError::Execution(inner.to_string()),
    })?;

    Ok(StructuredReport {
        text: cap_text(
            outcome.toon.clone(),
            "generated experiment in structuredContent.toon",
        ),
        structured: outcome.to_json(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scaffolding resolves actions against the live catalog, so tests point
    /// discovery at the workspace's real `plugins/` directory (every test
    /// sets the same value, so parallel test threads cannot race on it).
    fn point_discovery_at_workspace_plugins() {
        let plugins = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../plugins")
            .canonicalize()
            .unwrap();
        std::env::set_var("TUMULT_PLUGIN_PATH", plugins);
    }

    #[test]
    fn scaffold_produces_valid_toon() {
        point_discovery_at_workspace_plugins();
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
        point_discovery_at_workspace_plugins();
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
        point_discovery_at_workspace_plugins();
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
    fn scaffold_rejects_probe_kind_catalog_entries() {
        point_discovery_at_workspace_plugins();
        let args = serde_json::Map::new();
        let err = scaffold_experiment(&ScaffoldArgs {
            plugin: Some("tumult-network"),
            action: "ping-latency",
            args: &args,
            target: "checkout",
            probe_command: None,
            probe_url: None,
            probe_expect: None,
            title: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
        assert!(err.to_string().contains("unknown action"), "{err}");
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
