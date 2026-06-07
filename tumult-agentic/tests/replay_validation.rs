use tumult_agentic::adapters::AgentAdapter;
use tumult_agentic::model::{AgenticError, AgenticScenario};
use tumult_agentic::replay::{
    complete_replay_fixture, incomplete_replay_fixture_missing_output_ref, ReplayAdapter,
};

#[test]
fn replay_validation_accepts_complete_local_fixture() {
    let fixture = complete_replay_fixture();

    fixture
        .validate()
        .expect("complete fixture should validate");

    assert_eq!(
        fixture.output_refs(),
        vec![
            "model-output-001",
            "tool-output-001",
            "retrieval-output-001"
        ]
    );
}

#[test]
fn replay_validation_rejects_missing_output_refs() {
    let fixture = incomplete_replay_fixture_missing_output_ref();

    let error = fixture
        .validate()
        .expect_err("missing output_ref should fail");

    assert_eq!(
        error,
        AgenticError::IncompleteReplay(
            "source=local-smoke session_id=replay-smoke-missing-output step_index=0 step_type=model_response missing=output_ref"
                .to_string()
        )
    );
}

#[test]
fn replay_adapter_returns_deterministic_fixture_response() {
    let adapter =
        ReplayAdapter::new(complete_replay_fixture()).expect("complete fixture should adapt");
    let scenario = AgenticScenario {
        name: "replay-validation-success".to_string(),
        input: "replay locally".to_string(),
        expected_behavior: Some("return replay refs".to_string()),
    };

    let response = adapter
        .invoke(&scenario)
        .expect("replay adapter should respond");

    assert!(response
        .body
        .contains(r#""replay_session":"replay-smoke-001""#));
    assert!(response.body.contains("model-output-001"));
    assert!(response.body.contains("tool-output-001"));
    assert!(response.body.contains("retrieval-output-001"));
}
