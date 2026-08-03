//! Plugin discovery — find script plugins from filesystem paths.
//!
//! Discovery order:
//! 1. `./plugins/` (local to experiment)
//! 2. `~/.tumult/plugins/` (user-global)
//! 3. `TUMULT_PLUGIN_PATH` env var (custom paths, colon-separated)
//!
//! Discovery is fault-tolerant: one unreadable directory or malformed
//! manifest is collected as a warning and never aborts the pass — the
//! remaining search paths are still searched.

use std::path::{Path, PathBuf};

use crate::manifest::ScriptPluginManifest;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DiscoveryError {
    #[error("failed to read plugin directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("failed to parse plugin manifest at {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// A script plugin discovered on disk: the canonical directory containing
/// its `plugin.toon` (the root manifest script paths resolve against) plus
/// the parsed manifest.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub root: PathBuf,
    pub manifest: ScriptPluginManifest,
}

/// Outcome of a discovery pass: every usable plugin found, plus one warning
/// per skipped path or manifest. A single bad entry never aborts the pass.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryReport {
    pub plugins: Vec<DiscoveredPlugin>,
    pub warnings: Vec<String>,
}

impl DiscoveryReport {
    /// Manifests only, dropping roots and warnings — for registry use cases
    /// that predate the report API.
    #[must_use]
    pub fn into_manifests(self) -> Vec<ScriptPluginManifest> {
        self.plugins.into_iter().map(|p| p.manifest).collect()
    }
}

/// Discover script plugins from a single directory.
///
/// Each subdirectory containing a `plugin.toon` file is treated as a plugin.
/// A directory that simply does not exist yields an empty report (the normal
/// case for search paths that are not configured); an unreadable directory
/// or malformed manifest is collected as a warning and skipped.
#[must_use]
pub fn discover_plugins_in_dir(dir: &Path) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();

    if !dir.exists() || !dir.is_dir() {
        return report;
    }

    // Canonicalize base dir to prevent symlink escapes
    let canonical_dir = match std::fs::canonicalize(dir) {
        Ok(canonical) => canonical,
        Err(e) => {
            report
                .warnings
                .push(format!("skipping plugin directory {}: {e}", dir.display()));
            return report;
        }
    };

    let entries = match std::fs::read_dir(&canonical_dir) {
        Ok(entries) => entries,
        Err(e) => {
            report.warnings.push(format!(
                "skipping plugin directory {}: {e}",
                canonical_dir.display()
            ));
            return report;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                report.warnings.push(format!(
                    "skipping unreadable entry in {}: {e}",
                    canonical_dir.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() || path.is_symlink() {
            continue;
        }

        // Ensure resolved path stays within plugin directory
        let Ok(canonical_path) = std::fs::canonicalize(&path) else {
            continue;
        };
        if !canonical_path.starts_with(&canonical_dir) {
            continue; // symlink escape attempt
        }

        let manifest_path = canonical_path.join("plugin.toon");
        if !manifest_path.exists() {
            continue;
        }
        match read_manifest(&manifest_path) {
            Ok(manifest) => report.plugins.push(DiscoveredPlugin {
                root: canonical_path,
                manifest,
            }),
            Err(e) => report.warnings.push(e.to_string()),
        }
    }

    report
        .plugins
        .sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    report
}

/// Read and parse a single `plugin.toon` manifest.
fn read_manifest(manifest_path: &Path) -> Result<ScriptPluginManifest, DiscoveryError> {
    let content =
        std::fs::read_to_string(manifest_path).map_err(|e| DiscoveryError::ManifestParse {
            path: manifest_path.to_path_buf(),
            source: Box::new(e),
        })?;
    toon_format::decode_default(&content).map_err(|e| DiscoveryError::ManifestParse {
        path: manifest_path.to_path_buf(),
        source: Box::new(e),
    })
}

/// Configuration for plugin discovery paths.
#[derive(Debug, Clone, Default)]
pub struct PluginDiscoveryConfig {
    /// Additional plugin search paths (prepended to defaults).
    pub plugin_paths: Vec<PathBuf>,
}

/// Build the list of plugin search paths in discovery order.
#[must_use]
pub fn plugin_search_paths() -> Vec<PathBuf> {
    plugin_search_paths_with_config(&PluginDiscoveryConfig::default())
}

/// Build the list of plugin search paths with explicit config.
///
/// Discovery order:
/// 1. Paths from `config.plugin_paths`
/// 2. `./plugins/` (local to experiment)
/// 3. `~/.tumult/plugins/` (user-global)
/// 4. `TUMULT_PLUGIN_PATH` env var (colon-separated)
#[must_use]
pub fn plugin_search_paths_with_config(config: &PluginDiscoveryConfig) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 0. Explicit config paths
    paths.extend(config.plugin_paths.iter().cloned());

    // 1. Local ./plugins/
    paths.push(PathBuf::from("./plugins"));

    // 2. User-global ~/.tumult/plugins/
    if let Some(home) = dirs_path() {
        paths.push(home.join(".tumult").join("plugins"));
    }

    // 3. TUMULT_PLUGIN_PATH env var (colon-separated)
    if let Ok(env_paths) = std::env::var("TUMULT_PLUGIN_PATH") {
        for p in env_paths.split(':') {
            if !p.is_empty() {
                paths.push(PathBuf::from(p));
            }
        }
    }

    paths
}

/// Discover all script plugins from all search paths.
///
/// Fault-tolerant: every search path is tried even when an earlier one is
/// unreadable or holds a malformed manifest — problems land in
/// [`DiscoveryReport::warnings`]. Plugins are deduplicated by name,
/// first-found-wins; a shadowed copy (e.g. a user-global plugin hidden by a
/// `./plugins` entry of the same name) produces a warning naming the
/// shadowed path.
#[must_use]
pub fn discover_all_report() -> DiscoveryReport {
    discover_report_with_config(&PluginDiscoveryConfig::default())
}

/// Discover all script plugins using explicit config.
///
/// See [`discover_all_report`] for the tolerance and dedup semantics.
#[must_use]
pub fn discover_report_with_config(config: &PluginDiscoveryConfig) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    for path in plugin_search_paths_with_config(config) {
        let found = discover_plugins_in_dir(&path);
        report.plugins.extend(found.plugins);
        report.warnings.extend(found.warnings);
    }

    // Deduplicate by name (first found wins); a shadowed copy warns.
    let mut seen = std::collections::HashSet::new();
    let mut unique = Vec::with_capacity(report.plugins.len());
    for plugin in report.plugins {
        if seen.insert(plugin.manifest.name.clone()) {
            unique.push(plugin);
        } else {
            report.warnings.push(format!(
                "plugin '{}' at {} ignored — shadowed by an earlier discovery path",
                plugin.manifest.name,
                plugin.root.display()
            ));
        }
    }
    report.plugins = unique;
    report
}

/// Discover all script plugins from all search paths.
///
/// Compatibility wrapper over [`discover_all_report`] for callers that only
/// need manifests; discovery warnings are dropped. Discovery no longer
/// fails wholesale on a bad path or manifest.
///
/// # Errors
///
/// Never fails — the `Result` is kept for source compatibility. Use
/// [`discover_all_report`] when warnings matter.
#[allow(clippy::unnecessary_wraps)] // `Result` kept for source compatibility
pub fn discover_all_plugins() -> Result<Vec<ScriptPluginManifest>, DiscoveryError> {
    Ok(discover_all_report().into_manifests())
}

/// Discover all script plugins using explicit config.
///
/// See [`discover_all_plugins`] — warnings are dropped, never fails.
///
/// # Errors
///
/// Never fails — the `Result` is kept for source compatibility. Use
/// [`discover_report_with_config`] when warnings matter.
#[allow(clippy::unnecessary_wraps)] // `Result` kept for source compatibility
pub fn discover_all_plugins_with_config(
    config: &PluginDiscoveryConfig,
) -> Result<Vec<ScriptPluginManifest>, DiscoveryError> {
    Ok(discover_report_with_config(config).into_manifests())
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ScriptAction, ScriptProbe};
    use std::fs;
    use tempfile::TempDir;

    fn write_manifest(dir: &Path, name: &str, manifest: &ScriptPluginManifest) {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        let toon = toon_format::encode_default(manifest).unwrap();
        fs::write(plugin_dir.join("plugin.toon"), toon).unwrap();
    }

    fn sample_manifest(name: &str) -> ScriptPluginManifest {
        ScriptPluginManifest {
            name: name.into(),
            version: "0.1.0".into(),
            description: format!("{name} plugin"),
            actions: vec![ScriptAction {
                name: "action-1".into(),
                script: PathBuf::from("actions/action-1.sh"),
                description: "Test action".into(),
            }],
            probes: vec![ScriptProbe {
                name: "probe-1".into(),
                script: PathBuf::from("probes/probe-1.sh"),
                description: "Test probe".into(),
            }],
        }
    }

    // ── discover_plugins_in_dir ────────────────────────────────

    #[test]
    fn discovers_plugins_in_directory() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "tumult-kafka", &sample_manifest("tumult-kafka"));
        write_manifest(dir.path(), "tumult-redis", &sample_manifest("tumult-redis"));

        let report = discover_plugins_in_dir(dir.path());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.plugins.len(), 2);
        assert_eq!(report.plugins[0].manifest.name, "tumult-kafka");
        assert_eq!(report.plugins[1].manifest.name, "tumult-redis");
    }

    #[test]
    fn report_carries_plugin_root() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "tumult-kafka", &sample_manifest("tumult-kafka"));

        let report = discover_plugins_in_dir(dir.path());
        assert_eq!(report.plugins.len(), 1);
        let root = &report.plugins[0].root;
        assert!(
            root.ends_with("tumult-kafka"),
            "root should be the plugin directory, got {}",
            root.display()
        );
        assert!(root.join("plugin.toon").exists());
    }

    #[test]
    fn returns_empty_for_nonexistent_dir() {
        let report = discover_plugins_in_dir(Path::new("/nonexistent/path"));
        assert!(report.plugins.is_empty());
        // A missing search path is the normal unconfigured case — no warning.
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn ignores_dirs_without_manifest() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("no-manifest")).unwrap();
        write_manifest(dir.path(), "has-manifest", &sample_manifest("has-manifest"));

        let report = discover_plugins_in_dir(dir.path());
        assert_eq!(report.plugins.len(), 1);
        assert_eq!(report.plugins[0].manifest.name, "has-manifest");
    }

    #[test]
    fn ignores_files_in_plugin_dir() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("not-a-dir.txt"), "hello").unwrap();
        write_manifest(dir.path(), "real-plugin", &sample_manifest("real-plugin"));

        let report = discover_plugins_in_dir(dir.path());
        assert_eq!(report.plugins.len(), 1);
    }

    #[test]
    fn malformed_manifest_warns_and_does_not_abort() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("bad-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.toon"), "not valid toon {{{}").unwrap();
        write_manifest(dir.path(), "good-plugin", &sample_manifest("good-plugin"));

        let report = discover_plugins_in_dir(dir.path());
        // The good plugin is still discovered; the bad one is a warning.
        assert_eq!(report.plugins.len(), 1);
        assert_eq!(report.plugins[0].manifest.name, "good-plugin");
        assert_eq!(report.warnings.len(), 1);
        assert!(
            report.warnings[0].contains("bad-plugin"),
            "warning should name the manifest path: {}",
            report.warnings[0]
        );
    }

    // ── plugin_search_paths ────────────────────────────────────

    #[test]
    fn search_paths_includes_local_plugins() {
        let paths = plugin_search_paths();
        assert!(paths.contains(&PathBuf::from("./plugins")));
    }

    #[test]
    fn search_paths_includes_home_tumult_plugins() {
        let paths = plugin_search_paths();
        let home = std::env::var("HOME").unwrap();
        let expected = PathBuf::from(home).join(".tumult").join("plugins");
        assert!(paths.contains(&expected));
    }

    // ── discover_report_with_config (dedup + shadow warnings) ──

    #[test]
    fn shadowed_plugin_warns_naming_shadowed_path() {
        let local = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        write_manifest(
            local.path(),
            "tumult-kafka",
            &sample_manifest("tumult-kafka"),
        );
        write_manifest(
            global.path(),
            "tumult-kafka",
            &sample_manifest("tumult-kafka"),
        );

        let config = PluginDiscoveryConfig {
            plugin_paths: vec![local.path().to_path_buf(), global.path().to_path_buf()],
        };
        let report = discover_report_with_config(&config);

        // Discovery canonicalizes plugin roots (symlink-escape guard), so
        // compare against canonicalized expectations — on macOS tempdirs
        // live under /var but resolve to /private/var.
        let local_canon = local.path().canonicalize().unwrap();
        let global_canon = global.path().canonicalize().unwrap();

        // First path wins; exactly one copy survives.
        let kafka: Vec<_> = report
            .plugins
            .iter()
            .filter(|p| p.manifest.name == "tumult-kafka")
            .collect();
        assert_eq!(kafka.len(), 1);
        assert!(kafka[0].root.starts_with(&local_canon));

        // The shadowed copy warns, naming the shadowed path.
        let shadow_warnings: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| w.contains("shadowed"))
            .collect();
        assert_eq!(shadow_warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            shadow_warnings[0].contains(&global_canon.display().to_string()),
            "warning should name the shadowed path: {}",
            shadow_warnings[0]
        );
    }

    #[test]
    fn bad_path_does_not_abort_remaining_paths() {
        let good = TempDir::new().unwrap();
        write_manifest(good.path(), "tumult-good", &sample_manifest("tumult-good"));
        let bad = TempDir::new().unwrap();
        let bad_plugin = bad.path().join("broken");
        fs::create_dir(&bad_plugin).unwrap();
        fs::write(bad_plugin.join("plugin.toon"), "{{{{").unwrap();

        let config = PluginDiscoveryConfig {
            plugin_paths: vec![bad.path().to_path_buf(), good.path().to_path_buf()],
        };
        let report = discover_report_with_config(&config);

        assert!(
            report
                .plugins
                .iter()
                .any(|p| p.manifest.name == "tumult-good"),
            "discovery must continue past the bad path"
        );
        assert!(!report.warnings.is_empty());
    }

    // ── PluginDiscoveryConfig ─────────────────────────────────

    #[test]
    fn config_paths_are_searched_first() {
        let dir = TempDir::new().unwrap();
        write_manifest(
            dir.path(),
            "tumult-custom",
            &sample_manifest("tumult-custom"),
        );

        let config = PluginDiscoveryConfig {
            plugin_paths: vec![dir.path().to_path_buf()],
        };
        let paths = plugin_search_paths_with_config(&config);
        assert_eq!(paths[0], dir.path().to_path_buf());
    }

    #[test]
    fn discover_with_config_finds_plugins() {
        let dir = TempDir::new().unwrap();
        write_manifest(
            dir.path(),
            "tumult-custom",
            &sample_manifest("tumult-custom"),
        );

        let config = PluginDiscoveryConfig {
            plugin_paths: vec![dir.path().to_path_buf()],
        };
        let plugins = discover_all_plugins_with_config(&config).unwrap();
        assert!(plugins.iter().any(|p| p.name == "tumult-custom"));
    }

    #[test]
    fn into_manifests_drops_roots_and_warnings() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "tumult-kafka", &sample_manifest("tumult-kafka"));
        let bad_plugin = dir.path().join("broken");
        fs::create_dir(&bad_plugin).unwrap();
        fs::write(bad_plugin.join("plugin.toon"), "{{{{").unwrap();

        let config = PluginDiscoveryConfig {
            plugin_paths: vec![dir.path().to_path_buf()],
        };
        let manifests = discover_report_with_config(&config).into_manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "tumult-kafka");
    }

    #[test]
    fn compatibility_wrapper_never_fails() {
        // The legacy `Result`-returning entry point scans the default search
        // paths and always succeeds, whatever they contain.
        let plugins = discover_all_plugins().unwrap();
        // Every returned manifest is well-formed enough to have a name.
        assert!(plugins.iter().all(|p| !p.name.is_empty()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_plugin_dirs_are_skipped() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        write_manifest(
            elsewhere.path(),
            "tumult-linked",
            &sample_manifest("tumult-linked"),
        );
        symlink(
            elsewhere.path().join("tumult-linked"),
            dir.path().join("tumult-linked"),
        )
        .unwrap();

        let report = discover_plugins_in_dir(dir.path());
        assert!(
            report.plugins.is_empty(),
            "symlinked plugin dirs must be skipped: {:?}",
            report.plugins
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_warns_instead_of_aborting() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o000)).unwrap();
        let report = discover_plugins_in_dir(dir.path());
        // Restore permissions so TempDir cleanup cannot fail.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        assert!(report.plugins.is_empty());
        // Root can read a 0o000 directory, so only non-root runs see the
        // permission error that produces the warning.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(report.warnings.len(), 1);
            assert!(
                report.warnings[0].contains("skipping plugin directory"),
                "unexpected warning: {}",
                report.warnings[0]
            );
        }
    }
}
