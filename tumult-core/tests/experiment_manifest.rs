//! Every shipped experiment is in the expectations file of its set.
//!
//! Each set has an `expectations.json` that names, per experiment, the proof
//! that runs it and what its journal must say.
//! `scripts/check-proof-journals.py` compares the journals of a proof run
//! with it. This test keeps the files complete and well formed, so a new
//! experiment cannot ship without a proof or a written reason for having none.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use tumult_core::engine::parse_experiment;
use tumult_core::types::{Experiment, ExperimentStatus, GameDay, NfrKind, NfrState};

/// Directories that hold a set of experiments, relative to the workspace root.
const SETS: &[&str] = &[
    "examples",
    "demo/experiments",
    "demo/kronika/experiments",
    "gamedays",
    "demo/kronika/gamedays",
    "scripts/proof/rip-phase2",
    "scripts/proof/rip-phase4",
];

/// Sets whose `GameDay` runs the experiments of another set, each with the
/// directory of those experiments: the demo `GameDay` runs the experiments
/// of the Krönika seed. Such a set lists the experiments its `GameDays` run.
const STEPS_FROM: &[(&str, &str)] = &[("demo/kronika/gamedays", "demo/kronika/experiments")];

/// Proofs that keep the journal of each run and compare it with `expect`.
const JOURNAL_PROOFS: &[&str] = &["demo-check", "examples-proof", "kronika-seed"];
/// A proof script that makes its own assertions about the run.
const PROOF_SCRIPT: &str = "proof-script";
/// No proof runs the file. The entry says why.
const VALIDATE_ONLY: &str = "validate-only";

/// The endings of a file in TOON, of a journal and of a `GameDay` definition.
const TOON: &str = ".toon";
const JOURNAL: &str = ".journal.toon";
const GAMEDAY: &str = ".gameday.toon";

/// What an `expect` block must say.
const FACTS: [&str; 5] = [
    "status",
    "hypothesis_before",
    "hypothesis_after",
    "rollbacks",
    "objectives",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    experiments: BTreeMap<String, Entry>,
    #[serde(default)]
    gamedays: BTreeMap<String, GamedayEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    proof: String,
    reason: Option<String>,
    within: Option<String>,
    expect: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // decoded for its shape: a wrong type or key is an error
struct Expect {
    status: ExperimentStatus,
    hypothesis_before: Option<bool>,
    hypothesis_after: Option<bool>,
    rollbacks: u32,
    #[serde(default)]
    rollback_failures: u32,
    load_result: Option<bool>,
    objectives: BTreeMap<String, NfrState>,
    /// The seconds of data the run must have lost: its RPO measured.
    rpo_measured_s: Option<Between>,
}

/// A range of numbers, both ends in it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Between {
    from: f64,
    to: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // decoded for its shape
struct GamedayEntry {
    proof: String,
    expect: GamedayExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // decoded for its shape
struct GamedayExpect {
    load_result: bool,
    score: Option<GamedayScore>,
}

/// The score a `GameDay` journal must hold: its status and its recovery
/// part, `null` when no run measured a recovery.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // decoded for its shape
struct GamedayScore {
    status: ScoreStatus,
    recovery_compliance: Option<f64>,
}

#[derive(Deserialize)]
#[allow(dead_code)] // decoded for its shape
enum ScoreStatus {
    #[serde(rename = "COMPLIANT")]
    Compliant,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "NON-COMPLIANT")]
    NonCompliant,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the workspace root")
        .to_path_buf()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn manifest(set: &str) -> Manifest {
    let path = format!("{set}/expectations.json");
    serde_json::from_str(&read(&path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn manifests() -> Vec<(&'static str, Manifest)> {
    SETS.iter().map(|set| (*set, manifest(set))).collect()
}

/// The directory that holds the experiments of a set.
fn experiments_dir(set: &str) -> &str {
    STEPS_FROM
        .iter()
        .find(|(listed, _)| *listed == set)
        .map_or(set, |(_, dir)| dir)
}

/// The names of the `.toon` files of a set that are, or are not, `GameDay`
/// definitions. Journals are never listed.
fn files(set: &str, gamedays: bool) -> BTreeSet<String> {
    let dir = workspace_root().join(set);
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {set}: {e}"))
        .map(|entry| entry.expect("directory entry").file_name())
        .filter_map(|name| name.into_string().ok())
        .filter(|name| name.ends_with(TOON) && !name.ends_with(JOURNAL))
        .filter(|name| name.ends_with(GAMEDAY) == gamedays)
        .collect()
}

/// One line per name that is a file and not listed, or listed and not a file.
fn listing_faults(set: &str, files: &BTreeSet<String>, listed: &BTreeSet<String>) -> Vec<String> {
    let unlisted = files
        .difference(listed)
        .map(|name| format!("{set}/{name} is not in {set}/expectations.json"));
    let stale = listed
        .difference(files)
        .map(|name| format!("{set}/expectations.json lists {name}, which is not a file"));
    unlisted.chain(stale).collect()
}

fn keys<T>(map: &BTreeMap<String, T>) -> BTreeSet<String> {
    map.keys().cloned().collect()
}

fn expect_of(entry: &Entry) -> Result<Expect, String> {
    let Some(expect) = &entry.expect else {
        return Err("has no expect".into());
    };
    let missing: Vec<&str> = FACTS
        .into_iter()
        .filter(|fact| expect.get(fact).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(format!("expect has no {}", missing.join(", ")));
    }
    serde_json::from_value(expect.clone()).map_err(|e| format!("expect: {e}"))
}

/// What is wrong with an entry, given the proof it names.
fn entry_fault(entry: &Entry) -> Option<String> {
    let has_reason = entry
        .reason
        .as_deref()
        .is_some_and(|reason| !reason.trim().is_empty());
    match entry.proof.as_str() {
        VALIDATE_ONLY if !has_reason => Some("validate-only needs a reason".into()),
        VALIDATE_ONLY | PROOF_SCRIPT if entry.expect.is_some() => {
            Some(format!("{} reads no expect", entry.proof))
        }
        VALIDATE_ONLY | PROOF_SCRIPT => None,
        proof if JOURNAL_PROOFS.contains(&proof) => expect_of(entry).err(),
        other => Some(format!("unknown proof '{other}'")),
    }
}

fn experiment(set: &str, name: &str) -> Experiment {
    let path = format!("{}/{name}", experiments_dir(set));
    parse_experiment(&read(&path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The objective ids a run of this experiment can give a verdict for: the
/// declared ones, and `rto` and `rpo` when it sets a recovery target.
fn objective_ids(experiment: Experiment) -> BTreeSet<String> {
    let recovery = [
        ("rto", experiment.rto_target_s),
        ("rpo", experiment.rpo_target_s),
    ];
    let implied = recovery
        .into_iter()
        .filter_map(|(id, target)| target.map(|_| id.to_string()));
    let declared = experiment.nfr.into_iter().map(|objective| objective.id);
    declared.chain(implied).collect()
}

fn gameday_paths(set: &str, name: &str) -> BTreeSet<String> {
    let path = format!("{set}/{name}");
    let gameday: GameDay =
        toon_format::decode_default(&read(&path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    gameday
        .experiments
        .iter()
        .map(|child| child.path.display().to_string())
        .collect()
}

/// A `GameDay` and the experiments of its set that say they run within it
/// must name each other, and run in the same proof.
fn gameday_faults(set: &str, name: &str, manifest: &Manifest) -> Vec<String> {
    let proof = &manifest.gamedays[name].proof;
    let paths = gameday_paths(set, name);
    let within: BTreeSet<String> = manifest
        .experiments
        .iter()
        .filter(|(_, entry)| entry.within.as_deref() == Some(name))
        .filter(|(_, entry)| &entry.proof == proof)
        .map(|(path, _)| path.clone())
        .collect();
    let unclaimed = paths
        .difference(&within)
        .map(|path| format!("{set}/{path} runs in {name} and its entry does not say so"));
    let strangers = within
        .difference(&paths)
        .map(|path| format!("{set}/{path} says it runs within {name}, which does not run it"));
    unclaimed.chain(strangers).collect()
}

fn assert_none(faults: &[String]) {
    assert!(faults.is_empty(), "\n  {}\n", faults.join("\n  "));
}

/// The experiments a set must list: its files, or the experiments its
/// `GameDays` run when they come from another set.
fn listed_experiments(set: &str, manifest: &Manifest) -> BTreeSet<String> {
    if experiments_dir(set) == set {
        return files(set, false);
    }
    manifest
        .gamedays
        .keys()
        .flat_map(|name| gameday_paths(set, name))
        .collect()
}

#[test]
fn every_experiment_is_listed_and_every_entry_is_a_file() {
    let faults: Vec<String> = manifests()
        .iter()
        .flat_map(|(set, manifest)| {
            let wanted = listed_experiments(set, manifest);
            listing_faults(set, &wanted, &keys(&manifest.experiments))
        })
        .collect();

    assert_none(&faults);
}

#[test]
fn a_set_of_steps_from_another_set_lists_what_its_gamedays_run() {
    let (set, dir) = STEPS_FROM[0];
    let listed = listed_experiments(set, &manifest(set));

    assert!(!listed.is_empty(), "{set} runs no experiment");
    for name in &listed {
        assert!(
            workspace_root().join(dir).join(name).is_file(),
            "{set}: {name} is not in {dir}"
        );
    }
}

#[test]
fn every_entry_names_a_proof_and_what_that_proof_needs() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        for (name, entry) in &manifest.experiments {
            faults.extend(entry_fault(entry).map(|fault| format!("{set}/{name}: {fault}")));
        }
    }

    assert_none(&faults);
}

#[test]
fn an_expected_verdict_names_an_objective_of_the_experiment() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        for (name, entry) in &manifest.experiments {
            let Ok(expect) = expect_of(entry) else {
                continue;
            };
            let known = objective_ids(experiment(set, name));
            let unknown = expect.objectives.keys().filter(|id| !known.contains(*id));
            faults.extend(unknown.map(|id| format!("{set}/{name}: no objective '{id}'")));
        }
    }

    assert_none(&faults);
}

#[test]
fn a_completed_run_with_a_load_block_expects_its_load_result() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        for (name, entry) in &manifest.experiments {
            let Ok(expect) = expect_of(entry) else {
                continue;
            };
            let completed = expect.status == ExperimentStatus::Completed;
            let has_load = experiment(set, name).load.is_some();
            if completed && has_load && expect.load_result != Some(true) {
                faults.push(format!("{set}/{name}: expect has no \"load_result\": true"));
            }
        }
    }

    assert_none(&faults);
}

#[test]
fn every_gameday_is_listed_with_its_experiments_within_it() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        let listed = keys(&manifest.gamedays);
        faults.extend(listing_faults(set, &files(set, true), &listed));
        for name in files(set, true).intersection(&listed) {
            faults.extend(gameday_faults(set, name, &manifest));
        }
        let orphans = manifest
            .experiments
            .iter()
            .filter_map(|(path, entry)| Some((path, entry.within.as_ref()?)))
            .filter(|(_, within)| !listed.contains(*within));
        faults.extend(
            orphans.map(|(path, within)| format!("{set}/{path}: no GameDay {within} is listed")),
        );
    }

    assert_none(&faults);
}

#[test]
fn a_gameday_is_checked_by_a_proof_that_keeps_journals() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        let wrong = manifest
            .gamedays
            .iter()
            .filter(|(_, entry)| !JOURNAL_PROOFS.contains(&entry.proof.as_str()));
        faults.extend(wrong.map(|(name, entry)| format!("{set}/{name}: proof '{}'", entry.proof)));
    }

    assert_none(&faults);
}

/// The id of each `rpo` objective of an experiment.
fn rpo_objectives(experiment: Experiment) -> BTreeSet<String> {
    let rpo = experiment
        .nfr
        .into_iter()
        .filter(|o| o.kind == NfrKind::Rpo);
    rpo.map(|objective| objective.id).collect()
}

#[test]
fn an_expected_rpo_is_a_range_of_a_run_that_judges_its_rpo() {
    let mut faults = Vec::new();
    for (set, manifest) in manifests() {
        for (name, entry) in &manifest.experiments {
            let Ok(Expect {
                rpo_measured_s: Some(range),
                objectives,
                ..
            }) = expect_of(entry)
            else {
                continue;
            };
            if !(0.0 <= range.from && range.from <= range.to) {
                faults.push(format!(
                    "{set}/{name}: rpo_measured_s is no range from 0 up"
                ));
            }
            let judged = rpo_objectives(experiment(set, name));
            if !judged.iter().any(|id| objectives.contains_key(id)) {
                faults.push(format!("{set}/{name}: expects an RPO and no rpo verdict"));
            }
        }
    }

    assert_none(&faults);
}

/// The demo loses data for real: an example the demo check runs must
/// have lost some, so a run that lost none fails the check.
#[test]
fn the_demo_check_runs_an_example_that_loses_data() {
    let examples = manifest("examples");
    let losing: Vec<&String> = examples
        .experiments
        .iter()
        .filter(|(_, entry)| entry.proof == "demo-check")
        .filter(|(_, entry)| {
            expect_of(entry).is_ok_and(|expect| expect.rpo_measured_s.is_some_and(|r| r.from > 0.0))
        })
        .map(|(name, _)| name)
        .collect();

    assert!(
        !losing.is_empty(),
        "no example of the demo check expects an RPO above 0"
    );
}
