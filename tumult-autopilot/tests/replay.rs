//! Gate regression harness: replay every corpus scenario and assert its
//! verdict — the "test the autopilot before it tests you" loop from the
//! 2.15 plan. Any change to policy semantics or gate logic must replay
//! this corpus; a verdict flip is a regression unless the corpus is
//! deliberately updated with it.
//!
//! # Corpus shape (`tests/corpus/*.json`)
//!
//! One scenario per file:
//!
//! ```json
//! {
//!   "name": "human-readable scenario name",
//!   "policy_toml": "full autopilot.toml text",
//!   "candidate": { ... },        // tumult_autopilot::Candidate
//!   "ambient": { ... },          // tumult_autopilot::AmbientContext
//!   "autonomy": { ... } | null,  // tumult_autopilot::AutonomyRecord
//!   "expected_verdict": "enact" | "downgrade" | "propose" | "veto",
//!   "expected_rule": "rule.id"   // optional; for veto: the fired rule,
//! }                              // otherwise: a rule that must have failed
//! ```
//!
//! These files are the seed of the offline scoring harness: replays are
//! pure, so the same corpus can score candidate policies without touching
//! a live system.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tumult_autopilot::{
    evaluate, validate, AmbientContext, AutonomyRecord, Candidate, LoadedPolicy, Verdict,
};

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    policy_toml: String,
    candidate: Candidate,
    ambient: AmbientContext,
    autonomy: Option<AutonomyRecord>,
    expected_verdict: String,
    #[serde(default)]
    expected_rule: Option<String>,
}

fn verdict_word(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Enact => "enact",
        Verdict::Downgrade { .. } => "downgrade",
        Verdict::Propose { .. } => "propose",
        Verdict::Veto { .. } => "veto",
    }
}

fn corpus_paths() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("tests/corpus must exist")
        .map(|entry| entry.expect("corpus dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort(); // deterministic replay order
    paths
}

#[test]
fn every_corpus_scenario_replays_to_its_expected_verdict() {
    let paths = corpus_paths();
    assert!(
        paths.len() >= 10,
        "corpus shrank to {} files — the harness would silently weaken",
        paths.len()
    );

    for path in paths {
        let text = fs::read_to_string(&path).expect("corpus file readable");
        let scenario: Scenario =
            serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let loaded = LoadedPolicy::parse(&scenario.policy_toml)
            .unwrap_or_else(|err| panic!("{}: policy: {err}", scenario.name));

        let report = validate(&scenario.candidate);
        let decision = evaluate(
            &loaded.policy,
            &scenario.candidate,
            &scenario.ambient,
            scenario.autonomy.as_ref(),
            &report,
        );

        assert_eq!(
            verdict_word(&decision.verdict),
            scenario.expected_verdict,
            "scenario '{}' ({}): verdict was {:?}",
            scenario.name,
            path.display(),
            decision.verdict
        );

        if let Some(expected_rule) = &scenario.expected_rule {
            match &decision.verdict {
                Verdict::Veto { rule } => assert_eq!(
                    rule, expected_rule,
                    "scenario '{}': wrong veto rule fired",
                    scenario.name
                ),
                _ => assert!(
                    decision
                        .rules_evaluated
                        .iter()
                        .any(|(id, passed)| id == expected_rule && !passed),
                    "scenario '{}': rule {expected_rule} did not fail; trail: {:?}",
                    scenario.name,
                    decision.rules_evaluated
                ),
            }
        }
    }
}

#[test]
fn corpus_replay_is_bit_reproducible() {
    // The headline 2.15 property: same (policy text, inputs) — same
    // decision, byte for byte, run to run.
    for path in corpus_paths() {
        let text = fs::read_to_string(&path).expect("corpus file readable");
        let scenario: Scenario = serde_json::from_str(&text).expect("corpus parses");
        let loaded = LoadedPolicy::parse(&scenario.policy_toml).expect("policy parses");
        let report = validate(&scenario.candidate);
        let run = || {
            evaluate(
                &loaded.policy,
                &scenario.candidate,
                &scenario.ambient,
                scenario.autonomy.as_ref(),
                &report,
            )
        };
        assert_eq!(run(), run(), "{}", path.display());
        assert_eq!(
            loaded.hash,
            tumult_autopilot::policy_hash(&scenario.policy_toml)
        );
    }
}
