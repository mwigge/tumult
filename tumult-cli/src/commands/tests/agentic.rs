//! Tests for the agentic command handlers: scenario-pack listing, the
//! deterministic smoke/scenario/trajectory/replay paths, and the error paths
//! of the proxy and live-run handlers that do not need external services.

use super::super::*;
use super::helpers::{use_temp_store, ENV_LOCK};
use tempfile::TempDir;

#[test]
fn list_scenario_packs_renders_bundled_matrix() {
    let out = cmd_agentic_list_scenario_packs().unwrap();

    assert!(out.contains("Agentic scenario packs"), "{out}");
    assert!(out.contains("malformed-json-recovery"), "{out}");
    assert!(out.contains("adapters:"), "{out}");
    assert!(out.contains("faults:"), "{out}");
    assert!(out.contains("contracts:"), "{out}");
    assert!(
        out.contains("Agentic trajectory packs (multi-turn)"),
        "{out}"
    );
    assert!(out.contains("rag-grounding-failure"), "{out}");
    assert!(out.contains("trajectory_contracts:"), "{out}");
}

#[test]
fn smoke_passes_and_writes_metadata_journal() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    let journal = dir.path().join("smoke.journal.json");

    let out = cmd_agentic_smoke(&journal).unwrap();

    assert!(
        out.contains("Agentic smoke: malformed-json-recovery"),
        "{out}"
    );
    assert!(out.contains("fault_applied: true"), "{out}");
    assert!(out.contains("contract:"), "{out}");
    assert!(out.contains("resilience_score:"), "{out}");
    assert!(out.contains("trace_id:"), "{out}");
    assert!(out.contains("result: pass"), "{out}");
    assert!(journal.exists(), "metadata journal must be written");

    std::env::remove_var("TUMULT_LAKE_PATH");

    // The run is ingested into the analytics store the report pointed at.
    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    let rows = store.query("SELECT count(*) FROM agentic_runs").unwrap();
    assert_eq!(rows[0][0], "1");
}

#[test]
fn run_scenario_reports_fault_and_contract() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());
    let journal = dir.path().join("scenario.journal.json");

    let out = cmd_agentic_run_scenario("malformed-json-recovery", &journal).unwrap();

    assert!(
        out.contains("Agentic run: malformed-json-recovery"),
        "{out}"
    );
    assert!(out.contains("scenario: malformed-json-recovery"), "{out}");
    assert!(out.contains("result: pass"), "{out}");
    assert!(journal.exists());

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn run_scenario_unknown_pack_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_agentic_run_scenario("no-such-pack", &dir.path().join("j.json")).unwrap_err();
    assert!(err.to_string().contains("unknown scenario pack"), "{err}");
}

#[test]
fn trajectory_reports_steps_subscores_and_journal() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());
    let journal = dir.path().join("trajectory.journal.json");

    let out = cmd_agentic_trajectory("rag-grounding-failure", &journal).unwrap();

    assert!(
        out.contains("Agentic trajectory: rag-grounding-failure"),
        "{out}"
    );
    assert!(out.contains("adapter:"), "{out}");
    assert!(out.contains("steps:"), "{out}");
    assert!(out.contains("step[0]"), "{out}");
    assert!(out.contains("trajectory_contract:"), "{out}");
    assert!(out.contains("resilience_score:"), "{out}");
    assert!(out.contains("trace_id:"), "{out}");
    assert!(out.contains("result: pass"), "{out}");
    assert!(journal.exists());

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn trajectory_unknown_pack_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_agentic_trajectory("no-such-pack", &dir.path().join("j.json")).unwrap_err();
    assert!(err.to_string().contains("unknown trajectory pack"), "{err}");
}

#[test]
fn replay_complete_fixture_passes() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());
    let fixture_path = dir.path().join("fixture.json");
    let fixture = tumult_agentic::replay::complete_replay_fixture();
    std::fs::write(
        &fixture_path,
        serde_json::to_string_pretty(&fixture).unwrap(),
    )
    .unwrap();
    let journal = dir.path().join("replay.journal.json");

    let out = cmd_agentic_replay(&fixture_path, &journal).unwrap();

    assert!(out.contains("Agentic replay: captured fixture"), "{out}");
    assert!(out.contains("replay_source:"), "{out}");
    assert!(out.contains("replay_session:"), "{out}");
    assert!(out.contains("replay_steps:"), "{out}");

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn replay_missing_fixture_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_agentic_replay(&dir.path().join("missing.json"), &dir.path().join("j.json"))
        .unwrap_err();
    assert!(err.to_string().contains("read replay fixture"), "{err}");
}

#[test]
fn replay_malformed_json_errors() {
    let dir = TempDir::new().unwrap();
    let fixture_path = dir.path().join("broken.json");
    std::fs::write(&fixture_path, "this is not json {{{").unwrap();
    let err = cmd_agentic_replay(&fixture_path, &dir.path().join("j.json")).unwrap_err();
    assert!(err.to_string().contains("decode replay fixture"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_unknown_scenario_errors() {
    let err = cmd_agentic_proxy(
        "127.0.0.1:0",
        "http://upstream.invalid",
        "no-such-pack",
        None,
        42,
        "claude",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown scenario pack"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_invalid_listen_address_errors() {
    let err = cmd_agentic_proxy(
        "not-an-addr",
        "http://upstream.invalid",
        "malformed-json-recovery",
        None,
        42,
        "claude",
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid --listen address"),
        "{err}"
    );
}

#[test]
fn run_live_unknown_scenario_errors() {
    let err = cmd_agentic_run_live(
        "say hi",
        "no-such-pack",
        "http://127.0.0.1:1",
        None,
        "claude",
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown scenario pack"), "{err}");
}
