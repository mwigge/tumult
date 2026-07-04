//! Experiment-file discovery: recursively list `.toon` experiments.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// One discovered experiment file.
struct ExperimentEntry {
    name: String,
    path: String,
    title: String,
}

/// List all `.toon` experiment files found recursively under `search_root`,
/// sorted by relative path.
///
/// Returns one page of `limit` entries starting at `offset`. The structured
/// object is `{items, total, offset, limit}` with `{name, path, title}`
/// items; the text keeps the legacy `Experiments: N` header (total count)
/// plus one line per returned entry. Files that cannot be parsed are
/// skipped.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `search_root` directory cannot be read.
pub fn list_experiments(
    search_root: &str,
    limit: usize,
    offset: usize,
) -> Result<StructuredReport, ToolError> {
    let root = Path::new(search_root);
    let mut entries: Vec<ExperimentEntry> = Vec::new();

    collect_toon_files(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let total = entries.len();
    let page: Vec<ExperimentEntry> = entries.into_iter().skip(offset).take(limit).collect();

    let mut text = if total == 0 {
        "No experiment files found.".to_string()
    } else {
        format!("Experiments: {total}\n")
    };
    let items: Vec<serde_json::Value> = page
        .iter()
        .map(|entry| {
            let _ = writeln!(
                text,
                "  name={}  path={}  title={}",
                entry.name, entry.path, entry.title
            );
            serde_json::json!({
                "name": entry.name,
                "path": entry.path,
                "title": entry.title,
            })
        })
        .collect();

    let mut structured = serde_json::Map::new();
    structured.insert("items".into(), serde_json::json!(items));
    structured.insert("total".into(), serde_json::json!(total));
    structured.insert("offset".into(), serde_json::json!(offset));
    structured.insert("limit".into(), serde_json::json!(limit));
    Ok(StructuredReport { text, structured })
}

/// Recursively collect `.toon` experiment entries under `dir`.
fn collect_toon_files(
    base: &Path,
    dir: &Path,
    results: &mut Vec<ExperimentEntry>,
) -> Result<(), ToolError> {
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
            .unwrap_or_default()
            .to_string();

        results.push(ExperimentEntry {
            name,
            path: rel,
            title,
        });
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

        let report = list_experiments(dir.path().to_str().unwrap(), 100, 0)
            .expect("list_experiments should succeed");
        let output = &report.text;

        assert!(output.contains("First Experiment"), "must include first");
        assert!(output.contains("Second Experiment"), "must include second");
        assert!(
            !output.contains("readme.md"),
            "non-.toon file must be excluded"
        );
        // Count: exactly 2 experiments found.
        assert!(output.contains("Experiments: 2"), "count must be 2");
        assert_eq!(report.structured["total"], 2);
        let items = report.structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "first.toon");
        assert_eq!(items[0]["title"], "First Experiment");
    }

    #[test]
    fn list_experiments_empty_dir() {
        let dir = TempDir::new().unwrap();
        let report = list_experiments(dir.path().to_str().unwrap(), 100, 0).unwrap();
        assert!(report.text.contains("No experiment files found."));
        assert_eq!(report.structured["total"], 0);
    }

    #[test]
    fn list_experiments_skips_toon_without_title() {
        let dir = TempDir::new().unwrap();
        // File with no title field is skipped.
        std::fs::write(dir.path().join("no_title.toon"), "status: done\n").unwrap();
        let report = list_experiments(dir.path().to_str().unwrap(), 100, 0).unwrap();
        assert!(report.text.contains("No experiment files found."));
    }

    #[test]
    fn list_experiments_recurses_subdirectories() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.toon"), "title: Deep Experiment\n").unwrap();

        let report = list_experiments(dir.path().to_str().unwrap(), 100, 0).unwrap();
        assert!(
            report.text.contains("Deep Experiment"),
            "must recurse into subdirectory"
        );
    }

    #[test]
    fn list_experiments_paginates_with_honest_totals() {
        let dir = TempDir::new().unwrap();
        for i in 0..5 {
            std::fs::write(
                dir.path().join(format!("exp-{i}.toon")),
                format!("title: Experiment {i}\n"),
            )
            .unwrap();
        }
        let report = list_experiments(dir.path().to_str().unwrap(), 2, 3).unwrap();
        assert_eq!(report.structured["total"], 5);
        assert_eq!(report.structured["offset"], 3);
        assert_eq!(report.structured["limit"], 2);
        let items = report.structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "exp-3.toon");
        assert!(report.text.contains("Experiments: 5"), "{}", report.text);
        assert!(!report.text.contains("exp-0.toon"));
    }
}
