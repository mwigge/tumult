//! Every experiment that ships in the repository must load.
//!
//! The examples, the demo experiments, the Krönika seed, the `GameDay`
//! children and the proof experiments are what a new user copies first.
//! This test parses and validates each of them, so a broken file fails a
//! pull request instead of a demo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tumult_core::engine::{parse_experiment, validate_experiment, validate_placeholders};
use tumult_core::types::{
    Activity, ActivityType, Experiment, GameDay, GameDayJournal, LoadConfig, LoadTool, NfrKind,
    Provider, TestKind,
};

/// Directories that hold experiment files, relative to the workspace root.
const EXPERIMENT_DIRS: &[&str] = &[
    "examples",
    "demo/experiments",
    "demo/kronika/experiments",
    "gamedays",
    "scripts/proof/rip-phase2",
    "scripts/proof/rip-phase4",
    "scripts/docs-shots/experiments",
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

/// The `--var` values a file is run with, for the files that take any.
/// The scripts of the docs screenshots give these two their values.
const VARS: &[(&str, &[(&str, &str)])] = &[
    (
        "scripts/docs-shots/experiments/checkout-soak.toon",
        &[("port", "8080")],
    ),
    (
        "scripts/docs-shots/experiments/mq-consumer-pause.toon",
        &[("hold_s", "30")],
    ),
];

fn vars_of(path: &Path) -> HashMap<String, String> {
    let file = shown(path);
    VARS.iter()
        .filter(|(listed, _)| *listed == file)
        .flat_map(|(_, vars)| vars.iter())
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// What `tumult validate` checks: the file, then its placeholders with
/// the values the file is run with.
fn load(path: &Path) -> Result<Experiment, String> {
    let experiment = parse_experiment(&read(path)).map_err(|e| e.to_string())?;
    validate_experiment(&experiment).map_err(|e| e.to_string())?;
    validate_placeholders(&experiment, &vars_of(path)).map_err(|e| e.to_string())?;
    Ok(experiment)
}

#[test]
fn every_file_with_vars_is_a_shipped_experiment() {
    let files: Vec<String> = experiment_files().iter().map(|p| shown(p)).collect();
    for (file, _) in VARS {
        assert!(files.iter().any(|f| f == file), "{file} is not shipped");
    }
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

// ── Kind and target ────────────────────────────────────────────

/// Whether a file sets `kind` itself. The parsed value cannot tell: a
/// file without the key reads as chaos.
fn names_its_kind(text: &str) -> bool {
    text.lines().any(|line| line.starts_with("kind:"))
}

/// What a file leaves out of the things the scorecard groups runs by.
fn naming_gaps(path: &Path) -> Vec<&'static str> {
    let Ok(experiment) = load(path) else {
        return Vec::new();
    };
    let target = experiment.target.as_ref();
    let mut gaps = Vec::new();
    if !names_its_kind(&read(path)) {
        gaps.push("kind");
    }
    if target.is_none() {
        gaps.push("target");
    }
    if target.is_some_and(|t| t.environment.is_none()) {
        gaps.push("target.environment");
    }
    gaps
}

#[test]
fn a_kind_is_found_only_at_the_top_level() {
    assert!(names_its_kind("title: t\nkind: dr\n"));
    assert!(!names_its_kind(
        "title: t\nnfr[1]:\n  - id: a\n    kind: latency\n"
    ));
}

#[test]
fn every_shipped_experiment_names_its_kind_and_target() {
    let unnamed: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let gaps = naming_gaps(p);
            (!gaps.is_empty()).then(|| format!("{}: no {}", shown(p), gaps.join(", no ")))
        })
        .collect();
    assert!(
        unnamed.is_empty(),
        "{} files without a kind or a target:\n{}",
        unnamed.len(),
        unnamed.join("\n")
    );
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

/// The demo app listens on port 8080 when the demo stack is up.
const DEMO_APP: &str = "http://127.0.0.1:8080";
const DEMO_APP_ROUTER: &str = "demo/app/src/app.rs";

/// The routes in the text of a router: the first argument of each `.route(`.
fn routes_of(router: &str) -> Vec<String> {
    router
        .split(".route(\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(route, _)| route.to_string())
        .collect()
}

/// Whether `route` serves `path`: the same segments one by one, where a
/// `{name}` segment of the route takes any segment that is not empty. What
/// follows a `?` is not part of the path.
fn serves(route: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let asked: Vec<&str> = path.split('/').collect();
    let served: Vec<&str> = route.split('/').collect();
    asked.len() == served.len()
        && served.iter().zip(&asked).all(|(served, asked)| {
            served == asked || (served.starts_with('{') && !asked.is_empty())
        })
}

#[test]
fn routes_are_read_from_the_text_of_a_router() {
    let router = r#"Router::new()
        .route("/health", get(health))
        .route("/orders/{id}", get(get_order))
        .with_state(state)"#;
    assert_eq!(routes_of(router), ["/health", "/orders/{id}"]);
}

#[test]
fn a_route_serves_a_path_segment_by_segment() {
    assert!(serves("/health", "/health"));
    assert!(serves("/orders", "/orders?limit=5"));
    assert!(serves("/checkout/{id}", "/checkout/1"));
    assert!(!serves("/checkout/{id}", "/checkout/"));
    assert!(!serves("/checkout/{id}", "/checkout/1/items"));
    assert!(!serves("/orders", "/orders-of-today"));
    assert!(!serves("/orders/{id}", "/orders"));
}

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
    let routes = routes_of(&read(&workspace_root().join(DEMO_APP_ROUTER)));
    assert!(!routes.is_empty(), "no route in {DEMO_APP_ROUTER}");
    let mut wrong = Vec::new();
    for file in toon_files("examples") {
        for path in demo_app_paths(&read(&file)) {
            let served = routes.iter().any(|route| serves(route, &path));
            if !path.is_empty() && !served {
                wrong.push(format!("{}: {path}", shown(&file)));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the demo app serves {routes:?}, not:\n{}",
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
                .and_then(|experiment| {
                    validate_experiment(&experiment)?;
                    validate_placeholders(&experiment, &HashMap::new())
                })
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

/// The duplicate key check reads the text, so it must also accept the
/// spelling the encoder chooses (tables, inline lists) for every shipped
/// experiment: that is what template substitution and the builder feed it.
#[test]
fn every_shipped_experiment_is_read_back_from_the_encoder() {
    let broken: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let experiment = load(p).ok()?;
            let encoded = toon_format::encode_default(&experiment).expect("encode");
            let error = parse_experiment(&encoded).err()?;
            Some(format!("{}: {error}", shown(p)))
        })
        .collect();
    assert!(broken.is_empty(), "not read back:\n{}", broken.join("\n"));
}

// ── Faults that outlive their step ─────────────────────────────

/// The `listen` of a `tumult-net` step, and whether the step stops the
/// proxy there. Every other function of `tumult-net` starts a proxy, which
/// outlives the step.
fn net_proxy_step(step: &Activity) -> Option<(&str, bool)> {
    let Provider::Native {
        plugin,
        function,
        arguments,
    } = &step.provider
    else {
        return None;
    };
    if plugin != "tumult-net" {
        return None;
    }
    let listen = arguments.get("listen")?.as_str()?;
    Some((listen, function == "stop_proxy"))
}

/// The `listen` of each proxy a method starts and does not stop later in
/// the method. A run that completes runs no rollback, so such a proxy stays
/// up, and the next start on that `listen` is refused.
fn proxies_left_running(method: &[Activity]) -> Vec<&str> {
    method
        .iter()
        .enumerate()
        .filter_map(|(at, step)| {
            let (listen, stops) = net_proxy_step(step)?;
            let stopped_later = method[at + 1..]
                .iter()
                .any(|later| net_proxy_step(later) == Some((listen, true)));
            (!stops && !stopped_later).then_some(listen)
        })
        .collect()
}

#[test]
fn a_proxy_counts_as_stopped_only_on_its_own_listen() {
    let experiment = parse_experiment(
        r"title: t
method[3]:
  - name: start-a
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: inject_latency
      arguments:
        listen: 127.0.0.1:1
        upstream: 127.0.0.1:2
  - name: start-b
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: start_proxy
      arguments:
        listen: 127.0.0.1:3
        upstream: 127.0.0.1:2
  - name: stop-b
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: stop_proxy
      arguments:
        listen: 127.0.0.1:3
",
    )
    .unwrap();
    assert_eq!(proxies_left_running(&experiment.method), ["127.0.0.1:1"]);
}

#[test]
fn a_stop_before_the_start_leaves_the_proxy_running() {
    let experiment = parse_experiment(
        r"title: t
method[2]:
  - name: stop-a
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: stop_proxy
      arguments:
        listen: 127.0.0.1:1
  - name: start-a
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: inject_latency
      arguments:
        listen: 127.0.0.1:1
        upstream: 127.0.0.1:2
",
    )
    .unwrap();
    assert_eq!(proxies_left_running(&experiment.method), ["127.0.0.1:1"]);
}

#[test]
fn every_proxy_a_method_starts_is_stopped_by_the_method() {
    let left: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let experiment = load(p).ok()?;
            let listens = proxies_left_running(&experiment.method);
            (!listens.is_empty()).then(|| format!("{}: {}", shown(p), listens.join(", ")))
        })
        .collect();
    assert!(
        left.is_empty(),
        "a run that completes leaves a proxy running:\n{}",
        left.join("\n")
    );
}

/// The action that undoes a script action of a plugin: the one named
/// `<function>-rollback`, where the plugin ships it.
fn undo_of(plugin: &str, function: &str) -> Option<String> {
    let undo = format!("{function}-rollback");
    let script = workspace_root()
        .join("plugins")
        .join(plugin)
        .join("actions")
        .join(format!("{undo}.sh"));
    script.is_file().then_some(undo)
}

/// Each change a method makes with a script action that has an undo, where
/// the method does not call the undo later. A run that completes runs no
/// rollback, so such a change stays.
fn changes_left_in_place(method: &[Activity]) -> Vec<String> {
    let script = |step: &Activity| match &step.provider {
        Provider::Script {
            plugin, function, ..
        } => Some((plugin.clone(), function.clone())),
        _ => None,
    };
    method
        .iter()
        .enumerate()
        .filter_map(|(at, step)| {
            let (plugin, function) = script(step)?;
            let undo = undo_of(&plugin, &function)?;
            let undone_later = method[at + 1..]
                .iter()
                .any(|later| script(later) == Some((plugin.clone(), undo.clone())));
            (!undone_later).then(|| format!("{plugin}::{function}"))
        })
        .collect()
}

#[test]
fn an_undo_before_the_change_leaves_the_change_in_place() {
    let experiment = parse_experiment(
        r"title: t
method[4]:
  - name: undo-first
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      function: redirect-dns-rollback
  - name: change
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      function: redirect-dns
  - name: change-with-undo
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      function: block-dns
  - name: undo
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      function: block-dns-rollback
",
    )
    .unwrap();
    assert_eq!(
        changes_left_in_place(&experiment.method),
        ["tumult-network::redirect-dns"]
    );
}

#[test]
fn every_change_a_method_makes_with_a_plugin_is_undone_by_the_method() {
    let left: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let experiment = load(p).ok()?;
            let changes = changes_left_in_place(&experiment.method);
            (!changes.is_empty()).then(|| format!("{}: {}", shown(p), changes.join(", ")))
        })
        .collect();
    assert!(
        left.is_empty(),
        "a run that completes leaves a change in place:\n{}",
        left.join("\n")
    );
}

// ── Examples that change the host ──────────────────────────────

/// Examples that change the machine that runs them: they need root, edit a
/// system file, kill processes by pattern or load every core.
const HOST_CHANGING: &[&str] = &[
    "examples/cpu-stress.toon",
    "examples/dns-redirect-chaos.toon",
    "examples/network-latency.toon",
    "examples/process-kill.toon",
];

const HOST_CHANGING_TAG: &str = "host-changing";

/// How the description of such an example starts.
const HOST_CHANGING_NOTE: &str = "Changes this host: ";

/// Words in a file that give a host change away.
const HOST_CHANGE_MARKS: &[&str] = &["sudo ", "/etc/hosts", "pkill ", "tc qdisc", "redirect-dns"];

fn is_host_changing(path: &Path) -> bool {
    HOST_CHANGING.contains(&shown(path).as_str())
}

/// What a host-changing example fails to say.
fn host_note_gaps(experiment: &Experiment) -> Vec<&str> {
    let tagged = experiment.tags.iter().any(|tag| tag == HOST_CHANGING_TAG);
    let noted = experiment
        .description
        .as_deref()
        .is_some_and(|text| text.starts_with(HOST_CHANGING_NOTE));
    let mut gaps = Vec::new();
    if !tagged {
        gaps.push("no host-changing tag");
    }
    if !noted {
        gaps.push("the description does not start with the host note");
    }
    gaps
}

#[test]
fn every_host_changing_example_says_so_at_the_top() {
    let root = workspace_root();
    let silent: Vec<String> = HOST_CHANGING
        .iter()
        .filter_map(|file| {
            let experiment = load(&root.join(file)).expect("a listed example loads");
            let gaps = host_note_gaps(&experiment);
            (!gaps.is_empty()).then(|| format!("{file}: {}", gaps.join(", ")))
        })
        .collect();
    assert!(silent.is_empty(), "{}", silent.join("\n"));
}

#[test]
fn no_other_shipped_experiment_changes_the_host() {
    let unlisted: Vec<String> = experiment_files()
        .iter()
        .filter(|p| !is_host_changing(p))
        .filter_map(|p| {
            let text = read(p);
            let mark = HOST_CHANGE_MARKS
                .iter()
                .chain(&[HOST_CHANGING_TAG])
                .find(|mark| text.contains(**mark))?;
            Some(format!("{}: holds {mark:?}", shown(p)))
        })
        .collect();
    assert!(
        unlisted.is_empty(),
        "not in HOST_CHANGING:\n{}",
        unlisted.join("\n")
    );
}

/// The plugin that makes the change also knows how to undo it, and its
/// rollback can run twice.
#[test]
fn the_dns_example_undoes_its_change_with_the_plugin() {
    let experiment = load(&workspace_root().join("examples/dns-redirect-chaos.toon")).unwrap();

    let undo: Vec<(&str, &str)> = experiment
        .rollbacks
        .iter()
        .filter_map(|step| match &step.provider {
            Provider::Script {
                plugin, function, ..
            } => Some((plugin.as_str(), function.as_str())),
            _ => None,
        })
        .collect();

    assert_eq!(undo, [("tumult-network", "redirect-dns-rollback")]);
}

/// The redirect must be seen by a probe, or the run shows nothing.
#[test]
fn the_dns_example_looks_at_the_name_it_redirected() {
    let experiment = load(&workspace_root().join("examples/dns-redirect-chaos.toon")).unwrap();
    let redirect = experiment
        .method
        .iter()
        .position(|step| step.name == "redirect-dns")
        .expect("the redirect step");

    let seen_after = experiment.method[redirect + 1..].iter().any(|step| {
        step.tolerance.is_some()
            && matches!(&step.provider, Provider::Process { arguments, .. }
                if arguments.iter().any(|a| a.contains("test-redirect.tumult.local")))
    });

    assert!(seen_after, "no probe resolves the redirected name");
}

// ── Pumba in the lab ───────────────────────────────────────────

/// The experiments that the examples proof runs against the lab.
const LAB_DIRS: &[&str] = &["examples", "gamedays"];

/// The Pumba image of the lab, pinned by tag and digest. A tag alone can
/// move under a run, and the workflow pulls this one before the runs.
const PUMBA_IMAGE: &str = "ghcr.io/alexei-led/pumba:1.6.0@sha256:11ed15538c7ec688628e2f53917d949e8c16418c146742731c3030ef276b6057";

/// The stress-ng image a stress step of the lab gives Pumba. The default
/// image of Pumba picks its code by the model of the CPU, and it stopped
/// with SIGILL on a runner where AVX-512 could not run. The SSH server
/// image of the lab is Alpine, whose stress-ng has no such code. The lab
/// builds it, so Pumba must not pull it.
const LAB_STRESS_IMAGE: &str = "--stress-image tumult-sshd:lab --pull-image=false";

/// Each shell command of a lab experiment that runs Pumba, with its file.
fn pumba_commands() -> Vec<(String, String)> {
    LAB_DIRS
        .iter()
        .flat_map(|dir| toon_files(dir))
        .filter(|p| !is_gameday(p) && !is_journal(p))
        .flat_map(|p| {
            let experiment = load(&p).unwrap_or_else(|e| panic!("{}: {e}", shown(&p)));
            let probes = experiment.steady_state_hypothesis.map(|h| h.probes);
            let steps = experiment
                .method
                .into_iter()
                .chain(experiment.rollbacks)
                .chain(probes.into_iter().flatten());
            let file = shown(&p);
            steps
                .filter_map(|step| match step.provider {
                    Provider::Process { arguments, .. } => Some(arguments),
                    _ => None,
                })
                .flatten()
                .filter(|argument| argument.contains("alexei-led/pumba"))
                .map(move |command| (file.clone(), command))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The words of a command after the Pumba image: the Pumba command and
/// its flags.
fn pumba_words(command: &str) -> Vec<&str> {
    command
        .split_whitespace()
        .skip_while(|word| !word.contains("alexei-led/pumba"))
        .skip(1)
        .collect()
}

#[test]
fn every_pumba_step_of_the_lab_runs_the_pinned_image() {
    let commands = pumba_commands();
    assert!(!commands.is_empty(), "no lab experiment runs Pumba");

    let unpinned: Vec<String> = commands
        .iter()
        .filter(|(_, command)| !command.contains(&format!(" {PUMBA_IMAGE} ")))
        .map(|(file, command)| format!("{file}: {command}"))
        .collect();

    assert!(unpinned.is_empty(), "{}", unpinned.join("\n"));
}

#[test]
fn every_pumba_stress_step_of_the_lab_uses_the_stress_ng_of_the_lab() {
    let stress: Vec<(String, String)> = pumba_commands()
        .into_iter()
        .filter(|(_, command)| pumba_words(command).first() == Some(&"stress"))
        .collect();
    assert!(!stress.is_empty(), "no lab experiment runs a Pumba stress");

    let default_image: Vec<String> = stress
        .iter()
        .filter(|(_, command)| !pumba_words(command).join(" ").contains(LAB_STRESS_IMAGE))
        .map(|(file, command)| format!("{file}: {command}"))
        .collect();

    assert!(default_image.is_empty(), "{}", default_image.join("\n"));
}

// ── Probes that read, DR examples that break something ─────────

/// Words of a shell command that write or repair. The runner reads the
/// probes of the steady state again while the method runs and after it,
/// so a probe that writes can heal the fault it is there to see. A `>`
/// writes too, unless it goes to `/dev/null` or to another descriptor.
const WRITE_WORDS: &[&str] = &[
    "mkdir",
    "rm",
    "touch",
    "mv",
    "cp",
    "tee",
    "sed -i",
    "setsid",
    "nohup",
    "systemctl start",
    "systemctl restart",
    "docker start",
    "docker restart",
    "docker unpause",
];

/// Words of a shell command that stop something, beside the
/// [`WRITE_WORDS`]. A fault of a process step holds one of the two.
const STOP_WORDS: &[&str] = &[
    "kill",
    "pkill",
    "docker stop",
    "docker kill",
    "docker pause",
    "systemctl stop",
];

/// Plugin actions that break nothing: a fault of a DR example is none of
/// them.
const ACTIONS_THAT_BREAK_NOTHING: &[(&str, &str)] = &[
    ("tumult-db-postgres", "backup"),
    ("tumult-db-postgres", "write-markers"),
];

/// Files of `kind: dr` with no fault, each with the reason. A file here
/// is no DR example.
const DR_WITHOUT_A_FAULT: &[(&str, &str)] = &[
    (
        "scripts/proof/rip-phase2/dr-objectives.toon",
        "a fixture of the objective verdicts of the posture proof: its method waits and changes nothing on purpose",
    ),
    (
        "scripts/proof/rip-phase2/dr-breach.toon",
        "a fixture of a blocking breach of the posture proof: its method waits and changes nothing on purpose",
    ),
    (
        "scripts/docs-shots/experiments/dr-config-replica-kill.toon",
        "a fixture of the docs pictures: its step changes nothing on purpose, so the DR page shows a run with no outage observed",
    ),
];

/// The command of a process step: its program and its arguments.
fn command_of(step: &Activity) -> Option<String> {
    let Provider::Process {
        path, arguments, ..
    } = &step.provider
    else {
        return None;
    };
    Some(format!("{path} {}", arguments.join(" ")))
}

/// The words of a command, split at blanks and at the characters that
/// join or group commands.
fn words_of(command: &str) -> Vec<&str> {
    command
        .split(|c: char| c.is_whitespace() || ";&|()`".contains(c))
        .filter(|word| !word.is_empty())
        .collect()
}

/// The first of `listed` that is in `words`: one word, or two words one
/// after the other.
fn first_of<'a>(words: &[&str], listed: &[&'a str]) -> Option<&'a str> {
    listed.iter().copied().find(|entry| {
        let wanted: Vec<&str> = entry.split(' ').collect();
        words
            .windows(wanted.len())
            .any(|seen| seen == wanted.as_slice())
    })
}

/// Whether a command writes a file with `>`.
fn redirects_to_a_file(command: &str) -> bool {
    command.match_indices('>').any(|(at, _)| {
        let target = command[at + 1..].trim_start_matches('>').trim_start();
        !(target.starts_with('&') || target.starts_with("/dev/null"))
    })
}

/// What in a command writes: a write word, or a redirect to a file.
fn write_in(command: &str) -> Option<&'static str> {
    first_of(&words_of(command), WRITE_WORDS)
        .or_else(|| redirects_to_a_file(command).then_some(">"))
}

/// Each probe of the steady state whose command writes, with what writes.
fn probes_that_write(experiment: &Experiment) -> Vec<String> {
    let probes = experiment
        .steady_state_hypothesis
        .iter()
        .flat_map(|hypothesis| &hypothesis.probes);
    probes
        .filter_map(|probe| {
            let found = write_in(&command_of(probe)?)?;
            Some(format!("{} ({found})", probe.name))
        })
        .collect()
}

/// Whether a step of a method takes something away: a plugin action that
/// breaks something, or a process action that writes or stops.
fn breaks_something(step: &Activity) -> bool {
    if step.activity_type != ActivityType::Action {
        return false;
    }
    match &step.provider {
        Provider::Native { .. } => true,
        Provider::Script {
            plugin, function, ..
        } => !ACTIONS_THAT_BREAK_NOTHING.contains(&(plugin.as_str(), function.as_str())),
        Provider::Process { .. } => command_of(step).is_some_and(|command| {
            write_in(&command).is_some() || first_of(&words_of(&command), STOP_WORDS).is_some()
        }),
    }
}

fn is_dr_without_a_fault_on_purpose(path: &Path) -> bool {
    let file = shown(path);
    DR_WITHOUT_A_FAULT.iter().any(|(listed, _)| *listed == file)
}

#[test]
fn a_write_is_found_by_its_word_or_its_redirect() {
    assert_eq!(write_in("sh -c mkdir -p /tmp/d && echo ok"), Some("mkdir"));
    assert_eq!(write_in("sh -c [ -f x ] || (rm x)"), Some("rm"));
    assert_eq!(write_in("sh -c sed -i s/a/b/ f"), Some("sed -i"));
    assert_eq!(write_in("sh -c echo up > /tmp/state"), Some(">"));
    assert_eq!(write_in("sh -c date >>/tmp/log"), Some(">"));
    assert_eq!(write_in("sh -c docker start db"), Some("docker start"));
}

#[test]
fn a_read_is_not_a_write() {
    assert_eq!(write_in("sh -c cat /tmp/x 2>/dev/null || echo gone"), None);
    assert_eq!(
        write_in("sh -c kill -0 $(cat /tmp/pid) 2>&1 >/dev/null"),
        None
    );
    assert_eq!(write_in("sh -c ls /tmp/rmdir-me >&2"), None);
    assert_eq!(write_in("sh -c docker exec db pg_isready"), None);
}

#[test]
fn a_dr_fault_is_a_plugin_action_or_a_process_that_writes_or_stops() {
    let experiment = parse_experiment(
        r#"title: t
method[5]:
  - name: read
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "psql -c 'SELECT pg_terminate_backend(1)'"
  - name: probe-that-removes
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "rm -f /tmp/x"
  - name: back-up
    activity_type: action
    provider:
      type: script
      plugin: tumult-db-postgres
      function: backup
  - name: kill
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "kill $(cat /tmp/pid)"
  - name: stop-primary
    activity_type: action
    provider:
      type: script
      plugin: tumult-db-postgres
      function: stop-primary
"#,
    )
    .unwrap();
    let breaking: Vec<&str> = experiment
        .method
        .iter()
        .filter(|step| breaks_something(step))
        .map(|step| step.name.as_str())
        .collect();
    assert_eq!(breaking, ["kill", "stop-primary"]);
}

#[test]
fn every_steady_state_probe_only_reads() {
    let writing: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let probes = probes_that_write(&load(p).ok()?);
            (!probes.is_empty()).then(|| format!("{}: {}", shown(p), probes.join(", ")))
        })
        .collect();
    assert!(
        writing.is_empty(),
        "a steady-state probe writes; set the stage in a control or in the method:\n{}",
        writing.join("\n")
    );
}

#[test]
fn every_dr_example_has_a_fault_that_changes_state() {
    let blank: Vec<String> = experiment_files()
        .iter()
        .filter(|p| !is_dr_without_a_fault_on_purpose(p))
        .filter_map(|p| {
            let experiment = load(p).ok()?;
            let dr = experiment.kind == TestKind::Dr;
            (dr && !experiment.method.iter().any(breaks_something)).then(|| shown(p))
        })
        .collect();
    assert!(
        blank.is_empty(),
        "a DR example whose method breaks nothing:\n{}",
        blank.join("\n")
    );
}

/// A run that comes back after its recovery window breaches its RTO,
/// however long the RTO is, and `tumult validate` warns of it. No shipped
/// file gets that warning.
#[test]
fn every_shipped_rto_fits_its_recovery_window() {
    let short: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let experiment = load(p).ok()?;
            let rto_target_s = experiment.rto_target_s?;
            let window_s = experiment.recovery.unwrap_or_default().window_s;
            (rto_target_s > window_s)
                .then(|| format!("{}: RTO {rto_target_s} s, window {window_s} s", shown(p)))
        })
        .collect();
    assert!(
        short.is_empty(),
        "the recovery window is shorter than the RTO:\n{}",
        short.join("\n")
    );
}

#[test]
fn every_dr_file_without_a_fault_is_a_dr_file_and_breaks_nothing() {
    let root = workspace_root();
    for (file, reason) in DR_WITHOUT_A_FAULT {
        assert!(!reason.is_empty(), "{file} needs a reason");
        let experiment = load(&root.join(file)).expect("a listed file loads");
        assert_eq!(experiment.kind, TestKind::Dr, "{file}");
        assert!(
            !experiment.method.iter().any(breaks_something),
            "{file} breaks something"
        );
    }
}

// ── RPO examples lose what their method wrote ───────────────────

/// The plugin step that writes RPO markers, and the probe that reads back
/// the seconds of them that were lost.
const MARKER_WRITER: (&str, &str) = ("tumult-db-postgres", "write-markers");
const MARKER_READER: (&str, &str) = ("tumult-db-postgres", "markers-lost");

/// The arguments that say where markers go: the server, the database and
/// the ledger. A reader reads back only what a writer of the same ones
/// wrote.
const MARKER_STORE: &[&str] = &["pg_host", "pg_port", "pg_database", "ledger_path"];

/// The plugin and the function of a script step.
fn script_of(step: &Activity) -> Option<(&str, &str)> {
    let Provider::Script {
        plugin, function, ..
    } = &step.provider
    else {
        return None;
    };
    Some((plugin.as_str(), function.as_str()))
}

/// Where a marker step writes or reads: its [`MARKER_STORE`] arguments.
fn marker_store(step: &Activity) -> Vec<Option<&serde_json::Value>> {
    let Provider::Script { arguments, .. } = &step.provider else {
        return Vec::new();
    };
    MARKER_STORE.iter().map(|key| arguments.get(*key)).collect()
}

/// Why an `rpo` objective of `experiment` that reads `markers-lost` cannot
/// measure a loss, one line per objective. The method must write markers
/// to the store the reader reads, and a step after the last such write
/// must break something: else the RPO is 0 by construction.
fn rpo_faults(experiment: &Experiment) -> Vec<String> {
    let probes: Vec<&Activity> = experiment
        .steady_state_hypothesis
        .iter()
        .flat_map(|hypothesis| &hypothesis.probes)
        .collect();
    let faults = experiment
        .nfr
        .iter()
        .filter(|objective| objective.kind == NfrKind::Rpo)
        .filter_map(|objective| {
            let name = objective
                .metric
                .strip_prefix("probe.")?
                .strip_suffix(".lost_s")?;
            let reader = probes.iter().find(|probe| probe.name == name)?;
            if script_of(reader) != Some(MARKER_READER) {
                return None;
            }
            let store = marker_store(reader);
            let Some(last_write) = experiment.method.iter().rposition(|step| {
                script_of(step) == Some(MARKER_WRITER) && marker_store(step) == store
            }) else {
                return Some(format!("{name}: no step writes the markers it reads"));
            };
            let can_lose = experiment.method[last_write + 1..]
                .iter()
                .any(breaks_something);
            (!can_lose).then(|| format!("{name}: no step after the last write can lose it"))
        });
    faults.collect()
}

/// A steady state that reads the markers of `orders.ledger`, and a method
/// of `steps`.
fn rpo_fixture(steps: &[(&str, &str, &str)]) -> Experiment {
    let step = |(name, function, ledger): &(&str, &str, &str)| {
        format!(
            "  - name: {name}\n    activity_type: action\n    provider:\n      type: script\n      plugin: tumult-db-postgres\n      function: {function}\n      arguments:\n        pg_database: orders\n        ledger_path: {ledger}\n        backup_path: orders.dump\n"
        )
    };
    let method: String = steps.iter().map(step).collect();
    let text = format!(
        "title: t\nkind: dr\nrpo_target_s: 5\nnfr[1]:\n  - id: rpo\n    kind: rpo\n    metric: probe.lost.lost_s\n    operator: lte\n    threshold: 5\nsteady_state_hypothesis:\n  title: s\n  probes[1]:\n    - name: lost\n      activity_type: probe\n      provider:\n        type: script\n        plugin: tumult-db-postgres\n        function: markers-lost\n        arguments:\n          pg_database: orders\n          ledger_path: orders.ledger\n      tolerance:\n        type: number\nmethod[{}]:\n{method}",
        steps.len()
    );
    parse_experiment(&text).unwrap()
}

#[test]
fn an_rpo_reader_needs_a_write_of_its_store_and_a_loss_after_it() {
    let lost = [
        ("before", "write-markers", "orders.ledger"),
        ("back-up", "backup", ""),
        ("after", "write-markers", "orders.ledger"),
        ("restore", "restore", ""),
    ];
    assert_eq!(rpo_faults(&rpo_fixture(&lost)), Vec::<String>::new());

    let elsewhere = [
        ("write", "write-markers", "other.ledger"),
        ("restore", "restore", ""),
    ];
    assert_eq!(
        rpo_faults(&rpo_fixture(&elsewhere)),
        ["lost: no step writes the markers it reads"]
    );

    let kept = [
        ("back-up", "backup", ""),
        ("restore", "restore", ""),
        ("after", "write-markers", "orders.ledger"),
    ];
    assert_eq!(
        rpo_faults(&rpo_fixture(&kept)),
        ["lost: no step after the last write can lose it"]
    );
}

#[test]
fn every_shipped_rpo_reader_reads_back_writes_its_method_can_lose() {
    let faults: Vec<String> = experiment_files()
        .iter()
        .filter_map(|p| {
            let faults = rpo_faults(&load(p).ok()?);
            (!faults.is_empty()).then(|| format!("{}: {}", shown(p), faults.join("; ")))
        })
        .collect();
    assert!(
        faults.is_empty(),
        "an RPO that cannot measure a loss:\n{}",
        faults.join("\n")
    );
}
