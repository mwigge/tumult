//! Experiment-file discovery: recursively list `.toon` experiments.

use std::path::Path;

use crate::error::ToolError;

/// List all `.toon` experiment files found recursively under `search_root`.
///
/// Each result line contains the file name, relative path, and the `title`
/// field parsed from the experiment. Files that cannot be parsed are skipped.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `search_root` directory cannot be read.
pub fn list_experiments(search_root: &str) -> Result<String, ToolError> {
    let root = Path::new(search_root);
    let mut results: Vec<String> = Vec::new();

    collect_toon_files(root, root, &mut results)?;

    if results.is_empty() {
        return Ok("No experiment files found.".to_string());
    }

    let count = results.len();
    let mut output = format!("Experiments: {count}\n");
    for line in &results {
        output += line;
        output += "\n";
    }
    Ok(output)
}

/// Recursively collect `.toon` experiment entries under `dir`.
fn collect_toon_files(base: &Path, dir: &Path, results: &mut Vec<String>) -> Result<(), ToolError> {
    let read_dir = std::fs::read_dir(dir).map_err(ToolError::Io)?;

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse, but ignore errors from subdirectories (permissions etc.)
            let _ = collect_toon_files(base, &path, results);
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("toon") {
            continue;
        }

        // Try to extract the title field; skip files that aren't experiments.
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        // Quick parse: look for `title:` line (TOON format) or JSON/YAML title key.
        let title = extract_title(&content);
        let Some(title) = title else { continue };

        let rel = path
            .strip_prefix(base)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        results.push(format!("  name={name}  path={rel}  title={title}"));
    }

    Ok(())
}

/// Extract the `title` field from a TOON file's raw text content.
///
/// Supports both `title: value` (TOON/YAML) and `"title": "value"` (JSON) formats.
/// Returns `None` if no title field is found or the value is empty.
pub(crate) fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        // TOON / YAML style: `title: My experiment`
        if let Some(rest) = trimmed.strip_prefix("title:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        // JSON style: `"title": "My experiment"`
        if let Some(rest) = trimmed.strip_prefix("\"title\":") {
            let value = rest
                .trim()
                .trim_matches('"')
                .trim_matches(',')
                .trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn list_experiments_finds_toon_files() {
        let dir = TempDir::new().unwrap();

        // Write two experiment files with title fields.
        let exp1 = "title: First Experiment\nmethod[0]:\n";
        let exp2 = "title: Second Experiment\nmethod[0]:\n";
        // A journal file — no title field so it should NOT appear.
        let not_exp = "status: completed\n";
        // A non-.toon file — must be ignored.
        let not_toon = "title: ignored\n";

        std::fs::write(dir.path().join("first.toon"), exp1).unwrap();
        std::fs::write(dir.path().join("second.toon"), exp2).unwrap();
        std::fs::write(dir.path().join("journal.toon"), not_exp).unwrap();
        std::fs::write(dir.path().join("readme.md"), not_toon).unwrap();

        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok(), "list_experiments should succeed");
        let output = result.unwrap();

        assert!(output.contains("First Experiment"), "must include first");
        assert!(output.contains("Second Experiment"), "must include second");
        assert!(
            !output.contains("readme.md"),
            "non-.toon file must be excluded"
        );
        // Count: exactly 2 experiments found.
        assert!(output.contains("Experiments: 2"), "count must be 2");
    }

    #[test]
    fn list_experiments_empty_dir() {
        let dir = TempDir::new().unwrap();
        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("No experiment files found."));
    }

    #[test]
    fn list_experiments_skips_toon_without_title() {
        let dir = TempDir::new().unwrap();
        // File with no title field is skipped.
        std::fs::write(dir.path().join("no_title.toon"), "status: done\n").unwrap();
        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No experiment files found."));
    }

    #[test]
    fn list_experiments_recurses_subdirectories() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.toon"), "title: Deep Experiment\n").unwrap();

        let result = list_experiments(dir.path().to_str().unwrap());
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("Deep Experiment"),
            "must recurse into subdirectory"
        );
    }
}
