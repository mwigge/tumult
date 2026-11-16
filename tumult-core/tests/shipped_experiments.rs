//! Every experiment that ships in the repository must load.
//!
//! The examples, the demo experiments, the Krönika seed, the `GameDay`
//! children and the proof experiments are what a new user copies first.
//! This test parses and validates each of them, so a broken file fails a
//! pull request instead of a demo.

use std::path::{Path, PathBuf};

use tumult_core::engine::{parse_experiment, validate_experiment};
use tumult_core::types::{Experiment, GameDay, GameDayJournal, LoadConfig, LoadTool};

/// Directories that hold experiment files, relative to the workspace root.
const EXPERIMENT_DIRS: &[&str] = &[
    "examples",
    "demo/experiments",
    "demo/kronika/experiments",
    "gamedays",
    "scripts/proof/rip-phase2",
    "scripts/proof/rip-phase4",
];

/// Each `GameDay` definition with the directory its experiments are read
/// from. The Krönika seed mounts its experiments from the directory next to
/// the definition.
const GAMEDAYS: &[(&str, &str)] = &[
    ("gamedays/q2-postgres-resilience.gameday.toon", "gamedays"),
    (
        "demo/kronika/gamedays/platform-resilience.gameday.toon",
        "demo/kronika/experiments",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the workspace root")
        .to_path_buf()
}

fn toon_files(dir: &str) -> Vec<PathBuf> {
    let path = workspace_root().join(dir);
    let entries = std::fs::read_dir(&path).unwrap_or_else(|e| panic!("read {dir}: {e}"));
    let mut files: Vec<PathBuf> = entries
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "toon"))
        .collect();
    files.sort();
    files
}

fn has_suffix(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(suffix))
}

fn is_gameday(path: &Path) -> bool {
    has_suffix(path, ".gameday.toon")
}

fn is_journal(path: &Path) -> bool {
    has_suffix(path, ".journal.toon")
}

fn experiment_files() -> Vec<PathBuf> {
    EXPERIMENT_DIRS
        .iter()
        .flat_map(|dir| toon_files(dir))
        .filter(|p| !is_gameday(p) && !is_journal(p))
        .collect()
}

fn shown(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", shown(path)))
}

fn load(path: &Path) -> Result<Experiment, String> {
    let experiment = parse_experiment(&read(path)).map_err(|e| e.to_string())?;
    validate_experiment(&experiment).map_err(|e| e.to_string())?;
    Ok(experiment)
}

fn load_gameday(file: &str, children: &str) -> Result<(), String> {
    let root = workspace_root();
    let gameday: GameDay =
        toon_format::decode_default(&read(&root.join(file))).map_err(|e| e.to_string())?;
    if gameday.experiments.is_empty() {
        return Err("no experiments".into());
    }
    for child in &gameday.experiments {
        load(&root.join(children).join(&child.path))
            .map_err(|e| format!("child {}: {e}", child.path.display()))?;
    }
    Ok(())
}

#[test]
fn every_experiment_directory_holds_experiments() {
    for dir in EXPERIMENT_DIRS {
        let count = toon_files(dir)
            .iter()
            .filter(|p| !is_gameday(p) && !is_journal(p))
            .count();
        assert!(count > 0, "{dir} holds no experiment file");
    }
}

#[test]
fn every_shipped_experiment_parses_and_validates() {
    let broken: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| load(p).err().map(|e| format!("{}: {e}", shown(p))))
        .collect();
    assert!(
        broken.is_empty(),
        "broken experiments:\n{}",
        broken.join("\n")
    );
}

#[test]
fn every_shipped_gameday_parses_and_its_experiments_load() {
    let broken: Vec<String> = GAMEDAYS
        .iter()
        .filter_map(|(file, children)| {
            load_gameday(file, children)
                .err()
                .map(|e| format!("{file}: {e}"))
        })
        .collect();
    assert!(broken.is_empty(), "broken GameDays:\n{}", broken.join("\n"));
}

#[test]
fn every_gameday_file_is_listed() {
    let found: Vec<String> = ["gamedays", "demo/kronika/gamedays"]
        .iter()
        .flat_map(|dir| toon_files(dir))
        .filter(|p| is_gameday(p))
        .map(|p| shown(&p))
        .collect();
    let listed: Vec<&str> = GAMEDAYS.iter().map(|(file, _)| *file).collect();
    assert_eq!(found.len(), listed.len(), "found {found:?}");
    for file in &found {
        assert!(listed.contains(&file.as_str()), "{file} is not in GAMEDAYS");
    }
}

#[test]
fn the_recorded_gameday_journal_parses() {
    let path = workspace_root().join("gamedays/q2-postgres-resilience.gameday.journal.toon");
    let journal: GameDayJournal =
        toon_format::decode_default(&read(&path)).expect("decode journal");
    assert_eq!(journal.experiment_journals.len(), 4);
}

// ── The load and the target of the examples ────────────────────

/// What is wrong with a load block, if anything. A k6 script must be in
/// the repository and must send requests; an oha load names a URL.
fn load_fault(load: &LoadConfig) -> Option<String> {
    let script = load.script.to_string_lossy();
    if load.tool == LoadTool::Oha {
        return (!script.starts_with("http")).then(|| format!("oha needs a URL, not {script}"));
    }
    let path = workspace_root().join(load.script.as_path());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Some(format!("load script {script} is not in the repository"));
    };
    let sends = text.contains("k6/http") || text.contains("k6/x/sql");
    (!sends).then(|| format!("load script {script} sends no request"))
}

fn gameday_load(file: &str) -> Option<LoadConfig> {
    let gameday: GameDay = toon_format::decode_default(&read(&workspace_root().join(file)))
        .unwrap_or_else(|e| panic!("decode {file}: {e}"));
    gameday.load
}

#[test]
fn every_load_sends_real_requests() {
    let mut broken: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let load = load(p).ok()?.load?;
            load_fault(&load).map(|fault| format!("{}: {fault}", shown(p)))
        })
        .collect();
    for (file, _) in GAMEDAYS {
        let fault = gameday_load(file).as_ref().and_then(load_fault);
        broken.extend(fault.map(|fault| format!("{file}: {fault}")));
    }
    assert!(broken.is_empty(), "broken loads:\n{}", broken.join("\n"));
}

/// The routes of the demo app (`demo/app/src/app.rs`), which listens on
/// port 8080 when the demo stack is up.
const DEMO_APP_ROUTES: &[&str] = &["/health", "/orders", "/checkout/"];
const DEMO_APP: &str = "http://127.0.0.1:8080";

/// The paths asked of the demo app in a text.
fn demo_app_paths(text: &str) -> Vec<String> {
    text.match_indices(DEMO_APP)
        .map(|(at, _)| {
            text[at + DEMO_APP.len()..]
                .chars()
                .take_while(|c| !c.is_whitespace() && !"\"'\\".contains(*c))
                .collect()
        })
        .collect()
}

#[test]
fn demo_app_paths_are_read_from_a_text() {
    let text =
        "a http://127.0.0.1:8080/health b 'http://127.0.0.1:8080/checkout/1' http://127.0.0.1:8080";
    assert_eq!(demo_app_paths(text), vec!["/health", "/checkout/1", ""]);
}

#[test]
fn examples_ask_the_demo_app_only_for_routes_it_serves() {
    let mut wrong = Vec::new();
    for file in toon_files("examples") {
        for path in demo_app_paths(&read(&file)) {
            let served = DEMO_APP_ROUTES.iter().any(|route| path.starts_with(route));
            if !path.is_empty() && !served {
                wrong.push(format!("{}: {path}", shown(&file)));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the demo app does not serve:\n{}",
        wrong.join("\n")
    );
}

// ── Experiments shown in the documentation ─────────────────────

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", shown(dir)));
    for path in entries.map(|entry| entry.expect("directory entry").path()) {
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

/// The `toon` code blocks of a Markdown text, each with its first line number.
fn toon_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in markdown.lines().enumerate() {
        match open.take() {
            None if line.trim_end() == "```toon" => open = Some((index + 2, String::new())),
            None => {}
            Some(block) if line.trim_end() == "```" => blocks.push(block),
            Some((start, mut text)) => {
                text.push_str(line);
                text.push('\n');
                open = Some((start, text));
            }
        }
    }
    blocks
}

fn has_top_level(block: &str, key: &str) -> bool {
    block.lines().any(|line| line.starts_with(key))
}

/// How much of a documentation block can be checked.
#[derive(Debug, PartialEq)]
enum DocBlock {
    /// A method is there: parse and validate.
    Whole(String),
    /// Only a hypothesis or rollbacks: parse.
    Part(String),
    /// Not an experiment (a journal, a plugin manifest, a single step), or
    /// the author left steps out with a `...` line.
    Skip,
}

fn classify(block: &str) -> DocBlock {
    if block.lines().any(|line| line.trim() == "...") {
        return DocBlock::Skip;
    }
    let titled = if has_top_level(block, "title:") {
        block.to_owned()
    } else {
        format!("title: from the docs\n{block}")
    };
    if has_top_level(block, "method[") {
        return DocBlock::Whole(titled);
    }
    if has_top_level(block, "rollbacks[") || has_top_level(block, "steady_state_hypothesis:") {
        return DocBlock::Part(titled);
    }
    DocBlock::Skip
}

fn check_doc_block(block: &str) -> Option<Result<(), String>> {
    match classify(block) {
        DocBlock::Skip => None,
        DocBlock::Part(text) => Some(parse_experiment(&text).map(drop).map_err(|e| e.to_string())),
        DocBlock::Whole(text) => Some(
            parse_experiment(&text)
                .and_then(|experiment| validate_experiment(&experiment))
                .map_err(|e| e.to_string()),
        ),
    }
}

#[test]
fn documentation_blocks_are_classified() {
    assert_eq!(classify("plugin: x\n"), DocBlock::Skip);
    assert_eq!(
        classify("method[1]:\n  - name: a\n    ...\n"),
        DocBlock::Skip
    );
    assert_eq!(
        classify("title: t\nmethod[0]:\n"),
        DocBlock::Whole("title: t\nmethod[0]:\n".into())
    );
    assert_eq!(
        classify("rollbacks[0]:\n"),
        DocBlock::Part("title: from the docs\nrollbacks[0]:\n".into())
    );
}

#[test]
fn toon_blocks_are_read_with_their_line_number() {
    let markdown = "text\n```toon\ntitle: a\n```\n```bash\nls\n```\n```toon\nb: 1\nc: 2\n```\n";
    let blocks = toon_blocks(markdown);
    assert_eq!(
        blocks,
        vec![(3, "title: a\n".to_owned()), (9, "b: 1\nc: 2\n".to_owned())]
    );
}

#[test]
fn every_experiment_in_the_docs_parses_and_validates() {
    let root = workspace_root();
    let mut files = vec![root.join("README.md"), root.join("QUICKSTART.md")];
    markdown_files(&root.join("docs"), &mut files);
    files.sort();
    let mut checked = 0;
    let mut broken = Vec::new();
    for file in &files {
        for (line, block) in toon_blocks(&read(file)) {
            let Some(result) = check_doc_block(&block) else {
                continue;
            };
            checked += 1;
            if let Err(e) = result {
                broken.push(format!("{}:{line}: {e}", shown(file)));
            }
        }
    }
    assert!(
        checked >= 20,
        "only {checked} documentation experiments found"
    );
    assert!(
        broken.is_empty(),
        "broken documentation experiments:\n{}",
        broken.join("\n")
    );
}
