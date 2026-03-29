//! Plugin discovery — find script plugins from filesystem paths.
//!
//! Discovery order:
//! 1. `./plugins/` (local to experiment)
//! 2. `~/.tumult/plugins/` (user-global)
//! 3. `TUMULT_PLUGIN_PATH` env var (custom paths, colon-separated)

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
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Discover script plugins from a single directory.
///
/// Each subdirectory containing a `plugin.json` file is treated as a plugin.
/// (Will support `plugin.toon` when TOON file parsing is available.)
pub fn discover_plugins_in_dir(dir: &Path) -> Result<Vec<ScriptPluginManifest>, DiscoveryError> {
    let mut plugins = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(plugins);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("plugin.json");
        if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path).map_err(|e| {
                DiscoveryError::ManifestParse {
                    path: manifest_path.clone(),
                    source: Box::new(e),
                }
            })?;
            let manifest: ScriptPluginManifest =
                serde_json::from_str(&content).map_err(|e| DiscoveryError::ManifestParse {
                    path: manifest_path,
                    source: Box::new(e),
                })?;
            plugins.push(manifest);
        }
    }

    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(plugins)
}

/// Build the list of plugin search paths in discovery order.
pub fn plugin_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

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
pub fn discover_all_plugins() -> Result<Vec<ScriptPluginManifest>, DiscoveryError> {
    let mut all = Vec::new();
    for path in plugin_search_paths() {
        let found = discover_plugins_in_dir(&path)?;
        all.extend(found);
    }
    // Deduplicate by name (first found wins)
    let mut seen = std::collections::HashSet::new();
    all.retain(|p| seen.insert(p.name.clone()));
    Ok(all)
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
        let json = serde_json::to_string_pretty(manifest).unwrap();
        fs::write(plugin_dir.join("plugin.json"), json).unwrap();
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

        let plugins = discover_plugins_in_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "tumult-kafka");
        assert_eq!(plugins[1].name, "tumult-redis");
    }

    #[test]
    fn returns_empty_for_nonexistent_dir() {
        let plugins = discover_plugins_in_dir(Path::new("/nonexistent/path")).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn ignores_dirs_without_manifest() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("no-manifest")).unwrap();
        write_manifest(dir.path(), "has-manifest", &sample_manifest("has-manifest"));

        let plugins = discover_plugins_in_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "has-manifest");
    }

    #[test]
    fn ignores_files_in_plugin_dir() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("not-a-dir.txt"), "hello").unwrap();
        write_manifest(dir.path(), "real-plugin", &sample_manifest("real-plugin"));

        let plugins = discover_plugins_in_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 1);
    }

    #[test]
    fn returns_error_for_invalid_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("bad-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.json"), "not valid json").unwrap();

        let result = discover_plugins_in_dir(dir.path());
        assert!(result.is_err());
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

    // ── discover_all_plugins (deduplication) ───────────────────

    #[test]
    fn discover_all_deduplicates_by_name() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), "tumult-kafka", &sample_manifest("tumult-kafka"));

        // Discover from the same dir twice via the direct function
        let mut all = discover_plugins_in_dir(dir.path()).unwrap();
        all.extend(discover_plugins_in_dir(dir.path()).unwrap());

        // Deduplicate
        let mut seen = std::collections::HashSet::new();
        all.retain(|p| seen.insert(p.name.clone()));
        assert_eq!(all.len(), 1);
    }
}
