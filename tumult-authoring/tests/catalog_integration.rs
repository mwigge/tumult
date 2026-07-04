//! Integration tests that build the fault catalog from the repository's
//! shipped `plugins/` directory and scaffold an experiment from each domain.

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;

use tumult_authoring::builder::{build_experiment_toon, ProbeSpec, ScaffoldRequest};
use tumult_authoring::catalog::{build_catalog_with_config, ActionKind, Domain};
use tumult_core::engine::{parse_experiment, validate_experiment};
use tumult_plugin::discovery::PluginDiscoveryConfig;

/// Discovery config pointing at the repo's shipped plugins directory
/// (`<workspace>/plugins`), independent of the test's working directory.
fn shipped_plugins_config() -> PluginDiscoveryConfig {
    let plugins = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a parent workspace dir")
        .join("plugins");
    PluginDiscoveryConfig {
        plugin_paths: vec![plugins],
    }
}

#[test]
fn catalog_is_non_empty_and_covers_shipped_plugins() {
    let catalog = build_catalog_with_config(&shipped_plugins_config()).unwrap();
    assert!(!catalog.is_empty(), "catalog must not be empty");

    // Covers the first-party plugins shipped in the repo.
    let plugins: std::collections::HashSet<&str> =
        catalog.all_actions().map(|a| a.plugin.as_str()).collect();
    for expected in [
        "tumult-network",
        "tumult-stress",
        "tumult-db-postgres",
        "tumult-db-redis",
        "tumult-containers",
        "tumult-timewarp",
    ] {
        assert!(
            plugins.contains(expected),
            "catalog must cover built-in plugin '{expected}'; got {plugins:?}"
        );
    }

    // The catalog is grouped into the curated domains.
    let domains: std::collections::HashSet<Domain> =
        catalog.domains.iter().map(|d| d.domain).collect();
    assert!(domains.contains(&Domain::Network));
    assert!(domains.contains(&Domain::Resource));
    assert!(domains.contains(&Domain::Database));

    // Every action/probe surfaces at least one documented argument.
    for action in catalog.all_actions() {
        assert!(
            !action.args.is_empty(),
            "action {}::{} must surface documented args",
            action.plugin,
            action.name
        );
    }
}

#[test]
fn scaffold_from_each_domain_validates() {
    let catalog = build_catalog_with_config(&shipped_plugins_config()).unwrap();

    // Scaffold the first fault (non-probe) action of each domain and confirm
    // the generated TOON parses and validates.
    for domain in &catalog.domains {
        let Some(action) = domain.actions.iter().find(|a| a.kind == ActionKind::Action) else {
            continue;
        };

        let mut args = IndexMap::new();
        for arg in &action.args {
            if arg.required {
                args.insert(arg.name.clone(), "1".to_string());
            }
        }

        let request = ScaffoldRequest {
            title: format!("{} scaffold", action.name),
            plugin: action.plugin.clone(),
            action: action.name.clone(),
            args,
            target: "demo-target".to_string(),
            probe: ProbeSpec::default_for("demo-target"),
        };

        let toon = build_experiment_toon(&request).unwrap_or_else(|e| {
            panic!(
                "scaffold for {}::{} ({}) failed: {e}",
                action.plugin, action.name, domain.label
            )
        });
        let parsed = parse_experiment(&toon).unwrap();
        validate_experiment(&parsed).unwrap_or_else(|e| {
            panic!(
                "scaffold for {}::{} did not validate: {e}",
                action.plugin, action.name
            )
        });
    }
}

#[test]
fn scaffold_carries_target_and_rollback_where_applicable() {
    let mut args = IndexMap::new();
    args.insert("delay_ms".to_string(), "150".to_string());
    let request = ScaffoldRequest {
        title: "latency".to_string(),
        plugin: "tumult-network".to_string(),
        action: "add-latency".to_string(),
        args,
        target: "checkout".to_string(),
        probe: ProbeSpec::default_for("checkout"),
    };
    let toon = build_experiment_toon(&request).unwrap();
    assert!(toon.contains("checkout"));
    // add-latency has a curated rollback (reset-tc).
    let _empty: HashMap<String, String> = HashMap::new();
    let parsed = parse_experiment(&toon).unwrap();
    assert_eq!(parsed.rollbacks.len(), 1);
    assert_eq!(parsed.rollbacks[0].name, "reset-tc");
}
