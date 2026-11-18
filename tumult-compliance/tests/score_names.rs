//! Each score has one name. The compliance score of `scoring` is never
//! called a resilience score: that is the name of the continuous score of
//! `/api/posture`, which the Resilience page shows. The `GameDay` score has
//! a resilience score of its own.
//!
//! The tests read the sources a person reads a score from (the pages, the
//! report builders, the commands and the terminal UI) and the reports kept
//! with the tests.

use std::path::{Path, PathBuf};

use tumult_compliance::scoring::COMPLIANCE_SCORE_LINE;

/// The folders that are read, from the root of the workspace.
const READ: [&str; 6] = [
    "web/src",
    "tumult-compliance/src",
    "tumult-compliance/tests/golden",
    "tumult-cli/src",
    "tumult-cli/tests/golden",
    "tumult-tui/src",
];

/// The files that may call a score a resilience score, and on how many
/// lines.
const RESILIENCE_SCORE_LINES: [(&str, usize); 3] = [
    // The GameDay score.
    ("tumult-cli/src/commands/gameday.rs", 2),
    // The continuous score of `/api/posture`.
    ("web/src/lib/score-names.ts", 2),
    ("web/src/routes/resilience/+page.svelte", 1),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace")
        .to_path_buf()
}

/// Every file under `dir` that is not a test.
fn files(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("an entry of the folder").path();
        let name = path
            .file_name()
            .map_or(String::new(), |name| name.to_string_lossy().into_owned());
        if path.is_dir() {
            if name != "tests" {
                files(&path, found);
            }
        } else if !name.ends_with(".test.ts") && name != "tests.rs" {
            found.push(path);
        }
    }
}

/// `path` from the root of the workspace, with `/` between its parts.
fn shown(path: &Path, root: &Path) -> String {
    let parts: Vec<String> = path
        .strip_prefix(root)
        .expect("a file of the workspace")
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Each file read that says "resilience score", in any case, and on how
/// many lines.
fn resilience_score_lines() -> Vec<(String, usize)> {
    let root = workspace();
    let mut found = Vec::new();
    for dir in READ {
        files(&root.join(dir), &mut found);
    }
    let mut named: Vec<(String, usize)> = found
        .iter()
        .filter_map(|path| {
            let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let text = String::from_utf8_lossy(&bytes).to_lowercase();
            let lines = text
                .lines()
                .filter(|line| line.contains("resilience score"))
                .count();
            (lines > 0).then(|| (shown(path, &root), lines))
        })
        .collect();
    named.sort();
    named
}

#[test]
fn only_the_resilience_page_and_the_game_day_call_a_score_a_resilience_score() {
    let allowed: Vec<(String, usize)> = RESILIENCE_SCORE_LINES
        .iter()
        .map(|(path, lines)| ((*path).to_string(), *lines))
        .collect();

    assert_eq!(resilience_score_lines(), allowed);
}

#[test]
fn the_web_says_what_the_compliance_score_measures_in_the_words_of_the_digest() {
    let names = std::fs::read_to_string(workspace().join("web/src/lib/score-names.ts"))
        .expect("the names of the scores in the web");

    assert!(
        names.contains(&format!("'{COMPLIANCE_SCORE_LINE}'")),
        "{names}"
    );
}
