//! Approval storage tests (moved out of `src/approvals.rs`): canonical pin
//! hashing, request/decision round-trips, quorum guards, consume and
//! break-glass stamps, expiry listing, and the run-audit hash chain.

#![cfg(feature = "duckdb")]

use std::collections::BTreeMap;

use tumult_lake::approvals::{
    approval_pin, decision, ApprovalDecision, ApprovalRequest, CanonicalPin,
};
use tumult_lake::{run_state, NewRun, RegisteredDefinition, Store};

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn params_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn pin_is_deterministic_and_order_independent() {
    let a = CanonicalPin {
        definition_toon: "title: x",
        params: &params_map(&[("a", "1"), ("b", "2")]),
        env: "staging",
        target: Some("svc-a"),
    };
    let b = CanonicalPin {
        definition_toon: "title: x",
        params: &params_map(&[("b", "2"), ("a", "1")]),
        env: "staging",
        target: Some("svc-a"),
    };
    assert_eq!(approval_pin(&a), approval_pin(&b));
    assert_eq!(approval_pin(&a).len(), 64);
}

#[test]
fn pin_changes_on_any_input_edit() {
    let base = approval_pin(&CanonicalPin {
        definition_toon: "title: x",
        params: &params_map(&[("a", "1")]),
        env: "staging",
        target: None,
    });
    let edited_params = approval_pin(&CanonicalPin {
        definition_toon: "title: x",
        params: &params_map(&[("a", "2")]),
        env: "staging",
        target: None,
    });
    let edited_toon = approval_pin(&CanonicalPin {
        definition_toon: "title: y",
        params: &params_map(&[("a", "1")]),
        env: "staging",
        target: None,
    });
    let edited_env = approval_pin(&CanonicalPin {
        definition_toon: "title: x",
        params: &params_map(&[("a", "1")]),
        env: "prod",
        target: None,
    });
    assert_ne!(base, edited_params);
    assert_ne!(base, edited_toon);
    assert_ne!(base, edited_env);
}

fn request(run_id: &str) -> ApprovalRequest {
    ApprovalRequest {
        run_id: run_id.into(),
        tier: "T1".into(),
        pin_hash: "abc123".into(),
        env: "dev".into(),
        target: None,
        quorum_required: 1,
        requested_by: "alice".into(),
        requested_at_ns: 10,
        expires_at_ns: 20,
    }
}

fn decision(run_id: &str, approver: &str, dec: &str) -> ApprovalDecision {
    ApprovalDecision {
        run_id: run_id.into(),
        approver: approver.into(),
        decision: dec.into(),
        note: None,
        decided_at_ns: 15,
    }
}

#[test]
fn request_decision_roundtrip_and_queue() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_approval_request(&request("run-1")).unwrap();
    writer
        .insert_approval_decision(&decision("run-1", "bob", decision::APPROVED))
        .unwrap();

    let reader = store.read_only().unwrap();
    let req = reader.approval_request("run-1").unwrap().unwrap();
    assert_eq!(req["tier"], serde_json::json!("T1"));
    assert_eq!(req["pin_hash"], serde_json::json!("abc123"));
    let decs = reader.approval_decisions("run-1").unwrap();
    assert_eq!(decs.len(), 1);
    assert_eq!(decs[0]["approver"], serde_json::json!("bob"));
}

#[test]
fn self_approval_and_duplicate_decision_are_rejected() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_approval_request(&request("run-1")).unwrap();

    let err = writer
        .insert_approval_decision(&decision("run-1", "alice", decision::APPROVED))
        .unwrap_err();
    assert!(err.to_string().contains("self-approval"), "{err}");

    writer
        .insert_approval_decision(&decision("run-1", "bob", decision::APPROVED))
        .unwrap();
    let err = writer
        .insert_approval_decision(&decision("run-1", "bob", decision::REJECTED))
        .unwrap_err();
    assert!(err.to_string().contains("already decided"), "{err}");

    let err = writer
        .insert_approval_decision(&decision("run-9", "bob", decision::APPROVED))
        .unwrap_err();
    assert!(err.to_string().contains("no approval request"), "{err}");
}

#[test]
fn consume_and_break_glass_stamp_the_request() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_approval_request(&request("run-1")).unwrap();
    writer.consume_approval("run-1", 30).unwrap();
    writer
        .mark_break_glass("run-1", "admin", "prod down")
        .unwrap();

    let reader = store.read_only().unwrap();
    let req = reader.approval_request("run-1").unwrap().unwrap();
    assert_eq!(req["consumed_at_ns"], serde_json::json!(30));
    assert_eq!(req["break_glass"], serde_json::json!(true));
    assert_eq!(req["break_glass_by"], serde_json::json!("admin"));
    assert_eq!(
        req["break_glass_justification"],
        serde_json::json!("prod down")
    );
}

#[test]
fn expired_pending_lists_only_stale_gated_runs() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer
        .register_definition(&RegisteredDefinition {
            id: "reg-1".into(),
            name: "exp".into(),
            definition_toon: "title: exp".into(),
            content_hash: "h".into(),
            registered_at_ns: 1,
            registered_by: None,
        })
        .unwrap();
    for (id, expires) in [("run-stale", 20_i64), ("run-fresh", 100)] {
        writer
            .insert_run(&NewRun {
                id: id.into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: 10,
                actor: Some("alice".into()),
            })
            .unwrap();
        writer
            .set_run_state(id, run_state::PENDING_APPROVAL)
            .unwrap();
        let mut req = request(id);
        req.expires_at_ns = expires;
        writer.insert_approval_request(&req).unwrap();
    }

    let reader = store.read_only().unwrap();
    assert_eq!(
        reader.expired_pending_approvals(50).unwrap(),
        ["run-stale".to_string()]
    );
    assert_eq!(reader.approvals_queue().unwrap().len(), 2);
    assert_eq!(reader.approvals_list(10).unwrap().len(), 2);
}

#[test]
fn audit_chain_verifies_and_detects_tampering() {
    let d = tempfile::TempDir::new().unwrap();
    let db = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        writer
            .register_definition(&RegisteredDefinition {
                id: "reg-1".into(),
                name: "exp".into(),
                definition_toon: "title: exp".into(),
                content_hash: "h".into(),
                registered_at_ns: 1,
                registered_by: None,
            })
            .unwrap();
        writer
            .insert_run(&NewRun {
                id: "run-1".into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: 10,
                actor: Some("alice".into()),
            })
            .unwrap();
        writer
            .set_run_state("run-1", run_state::VALIDATING)
            .unwrap();
        writer.mark_run_started("run-1", None).unwrap();

        let reader = store.read_only().unwrap();
        assert!(reader.verify_run_audit_chain("run-1").unwrap());
        let trail = reader.run_audit_trail("run-1").unwrap();
        // Every v7 row carries a chain link; links chain pairwise.
        for w in trail.windows(2) {
            assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
        }
    }
    // Tamper with the trail behind the store's back (single-writer lock
    // released by dropping the store above).
    {
        let conn = duckdb::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE run_audit SET detail = 'forged' WHERE run_id = 'run-1' AND event = 'enqueued'",
            [],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let reader = store.read_only().unwrap();
    assert!(!reader.verify_run_audit_chain("run-1").unwrap());
}
