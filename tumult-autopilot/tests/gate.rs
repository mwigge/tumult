//! Behaviour tests for `tumult_autopilot::gate::evaluate` — verdict
//! precedence, reason aggregation, and the fixed-order audit trail.

use tumult_autopilot::gate::{
    RULE_AUTONOMY, RULE_BUSINESS_HOURS, RULE_COOLDOWN, RULE_DAILY_BUDGET, RULE_ENABLED,
    RULE_GUARD_PRESENT, RULE_TELEMETRY,
};
use tumult_autopilot::{
    evaluate, validate, AmbientContext, AutonomyRecord, Candidate, ConfidenceTier, LoadedPolicy,
    Trigger, Verdict, RULE_ORDER,
};

const POLICY: &str = r#"
[autopilot]
enabled = true
max_runs_per_day = 6
cooldown_hours = 12
enact_tiers = ["service", "edge"]

[[autopilot.pretrusted]]
plugin = "tumult-net"
action = "inject_latency"
tier = "service"
"#;

fn policy() -> tumult_autopilot::AutopilotPolicy {
    LoadedPolicy::parse(POLICY).unwrap().policy
}

fn candidate() -> Candidate {
    Candidate {
        id: "c-1".to_string(),
        service_id: "svc:demo-app".to_string(),
        tier: Some("service".to_string()),
        plugin: "tumult-net".to_string(),
        action: "inject_latency".to_string(),
        article_id: "compliance:DORA/Art.25".to_string(),
        score: 1.5,
        reasons: vec!["DORA/Art.25 is stale on svc:demo-app".to_string()],
        confidence: ConfidenceTier::High,
        playbook_experiment: Some("demo/experiments/demo-net.toon".to_string()),
        experiment_has_guard: true,
        experiment_has_rollback: true,
        experiment_has_steady_state: true,
        experiment_fault_count: 1,
        trigger: Trigger::Manual,
    }
}

fn ambient() -> AmbientContext {
    AmbientContext {
        open_deviation_for_target: false,
        runs_today: 0,
        hours_since_last_run_on_service: None,
        within_business_hours: true,
        concurrent_experiments: 0,
        guard_telemetry_ok: Some(true),
    }
}

fn decide(candidate: &Candidate, ambient: &AmbientContext) -> tumult_autopilot::GateDecision {
    let report = validate(candidate);
    evaluate(&policy(), candidate, ambient, None, &report)
}

#[test]
fn enacts_when_every_rule_passes() {
    let decision = decide(&candidate(), &ambient());
    assert_eq!(decision.verdict, Verdict::Enact);
    assert!(decision.rules_evaluated.iter().all(|(_, passed)| *passed));
}

#[test]
fn audit_trail_always_lists_every_rule_in_fixed_order() {
    // Enact case and veto case must produce the same rule ids in the same
    // order — no short-circuiting, or the audit record lies by omission.
    let enact = decide(&candidate(), &ambient());
    let mut vetoed_ambient = ambient();
    vetoed_ambient.open_deviation_for_target = true;
    let veto = decide(&candidate(), &vetoed_ambient);

    let ids = |d: &tumult_autopilot::GateDecision| {
        d.rules_evaluated
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&enact), RULE_ORDER.map(String::from).to_vec());
    assert_eq!(ids(&veto), ids(&enact));
}

#[test]
fn veto_takes_precedence_over_downgrade_conditions() {
    // Disabled policy + active cooldown: the hard rule must win and be named.
    let disabled = LoadedPolicy::parse("[autopilot]\nenabled = false\n")
        .unwrap()
        .policy;
    let c = candidate();
    let mut a = ambient();
    a.hours_since_last_run_on_service = Some(1.0);
    let report = validate(&c);
    let decision = evaluate(&disabled, &c, &a, None, &report);
    assert_eq!(
        decision.verdict,
        Verdict::Veto {
            rule: RULE_ENABLED.to_string()
        }
    );
}

#[test]
fn daily_budget_exhaustion_is_a_veto() {
    let mut a = ambient();
    a.runs_today = 6; // budget is 6/day
    let decision = decide(&candidate(), &a);
    assert_eq!(
        decision.verdict,
        Verdict::Veto {
            rule: RULE_DAILY_BUDGET.to_string()
        }
    );
}

#[test]
fn hollow_veto_beats_enactability_blockers() {
    // No steady state AND no rollback: cannot-falsify is the harder failure.
    let mut c = candidate();
    c.experiment_has_steady_state = false;
    c.experiment_has_rollback = false;
    let decision = decide(&c, &ambient());
    assert!(
        matches!(decision.verdict, Verdict::Veto { ref rule } if rule == "validator.not_hollow")
    );
}

#[test]
fn downgrade_collects_every_failing_bounded_condition() {
    let mut c = candidate();
    c.confidence = ConfidenceTier::Directional;
    let mut a = ambient();
    a.hours_since_last_run_on_service = Some(3.0);
    a.guard_telemetry_ok = None;
    let decision = decide(&c, &a);
    let Verdict::Downgrade { reasons } = decision.verdict else {
        panic!("expected downgrade, got {:?}", decision.verdict);
    };
    // cooldown + telemetry + confidence — all three named at once, so the
    // operator sees the complete distance to autonomy.
    assert_eq!(reasons.len(), 3, "reasons: {reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("cooldown")));
    assert!(reasons.iter().any(|r| r.contains("pre-flight not run")));
    assert!(reasons.iter().any(|r| r.contains("directional")));
    let failed: Vec<&str> = decision
        .rules_evaluated
        .iter()
        .filter(|(_, passed)| !passed)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        failed,
        vec![RULE_TELEMETRY, RULE_COOLDOWN, "confidence.high"]
    );
}

#[test]
fn propose_reasons_are_the_validator_blockers() {
    let mut c = candidate();
    c.experiment_has_rollback = false;
    let decision = decide(&c, &ambient());
    let Verdict::Propose { reasons } = decision.verdict else {
        panic!("expected propose, got {:?}", decision.verdict);
    };
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains("rollback"));
}

#[test]
fn missing_guard_downgrades_only_when_policy_requires_one() {
    let mut c = candidate();
    c.experiment_has_guard = false;
    let decision = decide(&c, &ambient());
    let Verdict::Downgrade { reasons } = decision.verdict else {
        panic!("expected downgrade, got {:?}", decision.verdict);
    };
    assert!(reasons.iter().any(|r| r.contains("no guard")));

    let lax = LoadedPolicy::parse(
        "[autopilot]\nenabled = true\nrequire_guard = false\nenact_tiers = [\"service\"]\n\n\
         [[autopilot.pretrusted]]\nplugin = \"tumult-net\"\naction = \"inject_latency\"\n\
         tier = \"service\"\n",
    )
    .unwrap()
    .policy;
    let report = validate(&c);
    let decision = evaluate(&lax, &c, &ambient(), None, &report);
    assert_eq!(decision.verdict, Verdict::Enact);
    assert!(decision
        .rules_evaluated
        .iter()
        .any(|(id, passed)| id == RULE_GUARD_PRESENT && *passed));
}

#[test]
fn business_hours_restriction_downgrades_outside_the_window() {
    let strict = LoadedPolicy::parse(
        "[autopilot]\nenabled = true\nbusiness_hours_only = true\nenact_tiers = [\"service\"]\n\n\
         [[autopilot.pretrusted]]\nplugin = \"tumult-net\"\naction = \"inject_latency\"\n\
         tier = \"service\"\n",
    )
    .unwrap()
    .policy;
    let c = candidate();
    let mut a = ambient();
    a.within_business_hours = false;
    let report = validate(&c);
    let decision = evaluate(&strict, &c, &a, None, &report);
    let Verdict::Downgrade { reasons } = decision.verdict else {
        panic!("expected downgrade, got {:?}", decision.verdict);
    };
    assert!(reasons.iter().any(|r| r.contains("business hours")));
    assert!(decision
        .rules_evaluated
        .iter()
        .any(|(id, passed)| id == RULE_BUSINESS_HOURS && !passed));
}

#[test]
fn earned_record_substitutes_for_pretrust() {
    // Same candidate but a class the policy does not pretrust: autonomy
    // must come from the record.
    let mut c = candidate();
    c.action = "drop_packets".to_string();
    c.playbook_experiment = Some("demo/experiments/drop.toon".to_string());
    let report = validate(&c);

    let without = evaluate(&policy(), &c, &ambient(), None, &report);
    let Verdict::Downgrade { reasons } = without.verdict else {
        panic!("expected downgrade, got {:?}", without.verdict);
    };
    assert!(reasons
        .iter()
        .any(|r| r.contains("autonomy not earned for class tumult-net::drop_packets@service")));

    let record = AutonomyRecord {
        enacted_total: 5,
        enacted_clean: 5,
    };
    let with = evaluate(&policy(), &c, &ambient(), Some(&record), &report);
    assert_eq!(with.verdict, Verdict::Enact);
    assert!(with
        .rules_evaluated
        .iter()
        .any(|(id, passed)| id == RULE_AUTONOMY && *passed));
}

#[test]
fn evaluation_is_deterministic() {
    let c = candidate();
    let a = ambient();
    let report = validate(&c);
    let first = evaluate(&policy(), &c, &a, None, &report);
    let second = evaluate(&policy(), &c, &a, None, &report);
    assert_eq!(first, second);
}
