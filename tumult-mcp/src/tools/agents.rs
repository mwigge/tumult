//! Agent CLI adapter detection (`tumult agents` parity).

use std::fmt::Write as _;

use tumult_agent_cli::AdapterRegistry;

use crate::tools::StructuredReport;

/// List every registered agent CLI adapter with its probed install,
/// version, and auth state.
///
/// Detection spawns short local version probes (see
/// `AdapterRegistry::detect_all`); probe failures are reported structurally
/// per adapter, never as tool errors.
#[must_use]
pub fn agents() -> StructuredReport {
    let registry = AdapterRegistry::builtin();

    let mut text = String::new();
    writeln!(
        text,
        "{:<14} {:<10} {:<10} DETAIL",
        "ADAPTER", "INSTALLED", "VERSION"
    )
    .ok();

    let mut adapters = Vec::new();
    for (name, probe) in registry.detect_all() {
        let installed = if probe.installed { "yes" } else { "no" };
        let version = probe.version.as_deref().unwrap_or("-");
        let mut detail = probe.detail.trim().to_string();
        if !probe.installed {
            if let Ok(adapter) = registry.get(name) {
                let hint = adapter.install_hint();
                if !detail.contains(hint) {
                    write!(detail, " Install with: {hint}").ok();
                }
            }
        }
        writeln!(text, "{name:<14} {installed:<10} {version:<10} {detail}").ok();

        adapters.push(serde_json::json!({
            "name": name,
            "installed": probe.installed,
            "version": probe.version,
            "logged_in": probe.logged_in,
            "detail": detail,
        }));
    }

    let mut structured = serde_json::Map::new();
    structured.insert("adapters".into(), serde_json::Value::Array(adapters));

    StructuredReport { text, structured }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_reports_every_builtin_adapter() {
        let report = agents();
        let adapters = report.structured["adapters"].as_array().unwrap();
        let names: Vec<&str> = adapters
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"claude-code"), "adapters: {names:?}");
        assert!(names.contains(&"codex"), "adapters: {names:?}");
        for adapter in adapters {
            assert!(adapter["installed"].is_boolean());
            assert!(adapter["detail"].is_string());
            let name = adapter["name"].as_str().unwrap();
            assert!(
                report.text.contains(name),
                "text table must list '{name}': {}",
                report.text
            );
        }
    }
}
