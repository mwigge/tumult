//! Behaviour tests for `tumult_autopilot::policy`.

use tumult_autopilot::{LoadedPolicy, PolicyError};

/// The full policy shape from the 2.15 plan, verbatim in spirit.
const FULL_POLICY: &str = r#"
[autopilot]
enabled = true
max_runs_per_day = 6
cooldown_hours = 12
evidence_ttl_days = 90
enact_tiers = ["service", "edge"]
require_guard = true
business_hours_only = false
require_enrollment = true
enrolled_services = ["demo-app", "svc:billing"]
autonomy_threshold = 0.8
autonomy_min_samples = 3

[[autopilot.pretrusted]]
plugin = "tumult-postgres"
action = "kill-connections"
tier = "data"

[[autopilot.playbook]]
plugin = "tumult-net"
action = "inject_latency"
service = "demo-app"
experiment = "demo/experiments/demo-net.toon"

[[autopilot.playbook]]
plugin = "tumult-net"
action = "inject_latency"
experiment = "demo/experiments/generic-net.toon"

[autopilot.evidence_ttl_days_by_framework]
DORA = 90
NIS2 = 120
"#;

#[test]
fn full_policy_parses_with_every_field() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    assert!(p.enabled);
    assert_eq!(p.max_runs_per_day, 6);
    assert_eq!(p.cooldown_hours, 12);
    assert_eq!(p.evidence_ttl_days, 90);
    assert_eq!(p.enact_tiers, vec!["service", "edge"]);
    assert!(p.require_guard);
    assert!(!p.business_hours_only);
    assert!(p.require_enrollment);
    assert_eq!(p.enrolled_services, vec!["demo-app", "svc:billing"]);
    assert!((p.autonomy_threshold - 0.8).abs() < 1e-12);
    assert_eq!(p.autonomy_min_samples, 3);
    assert_eq!(p.pretrusted.len(), 1);
    assert_eq!(p.playbook.len(), 2);
    assert_eq!(p.evidence_ttl_days_by_framework.len(), 2);
    assert_eq!(loaded.raw_toml, FULL_POLICY);
    assert_eq!(loaded.policy_hash().len(), 64);
}

#[test]
fn empty_document_parses_to_the_disabled_conservative_default() {
    let loaded = LoadedPolicy::parse("").unwrap();
    let p = &loaded.policy;
    assert!(!p.enabled);
    assert_eq!(p.max_runs_per_day, 6);
    assert_eq!(p.cooldown_hours, 12);
    assert_eq!(p.evidence_ttl_days, 90);
    assert!(p.enact_tiers.is_empty()); // no tier may ever enact
    assert!(p.require_guard);
    assert!(!p.business_hours_only);
    assert!(!p.require_enrollment); // enrollment is opt-in
    assert!(p.enrolled_services.is_empty());
    assert_eq!(p.autonomy_min_samples, 3);
}

#[test]
fn enrollment_matches_with_or_without_the_svc_prefix() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    // Bare entry, both spellings of the query.
    assert!(p.is_enrolled("demo-app"));
    assert!(p.is_enrolled("svc:demo-app"));
    // Prefixed entry, both spellings of the query.
    assert!(p.is_enrolled("billing"));
    assert!(p.is_enrolled("svc:billing"));
    assert!(!p.is_enrolled("svc:unlisted"));
}

#[test]
fn every_service_counts_as_enrolled_when_enrollment_is_not_required() {
    let loaded = LoadedPolicy::parse("").unwrap();
    assert!(loaded.policy.is_enrolled("svc:anything"));
}

#[test]
fn unknown_keys_fail_loudly() {
    // A typo in a safety policy must not silently deserialise into a more
    // permissive default.
    let err = LoadedPolicy::parse("[autopilot]\nbussiness_hours_only = true\n").unwrap_err();
    assert!(matches!(err, PolicyError::Parse(_)), "got {err}");
}

#[test]
fn threshold_out_of_range_is_rejected() {
    let err = LoadedPolicy::parse("[autopilot]\nautonomy_threshold = 1.5\n").unwrap_err();
    assert!(
        matches!(err, PolicyError::ThresholdOutOfRange(_)),
        "got {err}"
    );
}

#[test]
fn zero_min_samples_is_rejected() {
    let err = LoadedPolicy::parse("[autopilot]\nautonomy_min_samples = 0\n").unwrap_err();
    assert!(matches!(err, PolicyError::ZeroMinSamples), "got {err}");
}

#[test]
fn incomplete_pretrust_entry_is_rejected() {
    let text =
        "[[autopilot.pretrusted]]\nplugin = \"tumult-net\"\naction = \"\"\ntier = \"data\"\n";
    let err = LoadedPolicy::parse(text).unwrap_err();
    assert!(
        matches!(err, PolicyError::IncompletePretrust(0)),
        "got {err}"
    );
}

#[test]
fn playbook_resolution_prefers_the_service_specific_entry() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    let specific = p
        .playbook_for("tumult-net", "inject_latency", Some("demo-app"))
        .unwrap();
    assert_eq!(specific.experiment, "demo/experiments/demo-net.toon");
    let generic = p
        .playbook_for("tumult-net", "inject_latency", Some("other-app"))
        .unwrap();
    assert_eq!(generic.experiment, "demo/experiments/generic-net.toon");
    assert!(p.playbook_for("tumult-net", "drop_packets", None).is_none());
}

#[test]
fn pretrust_requires_an_exact_class_match() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    assert!(p.is_pretrusted("tumult-postgres", "kill-connections", Some("data")));
    // Wrong tier, missing tier, wrong action: all misses.
    assert!(!p.is_pretrusted("tumult-postgres", "kill-connections", Some("service")));
    assert!(!p.is_pretrusted("tumult-postgres", "kill-connections", None));
    assert!(!p.is_pretrusted("tumult-postgres", "kill-processes", Some("data")));
}

#[test]
fn evidence_ttl_uses_framework_override_then_default() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    assert_eq!(p.evidence_ttl_days_for("NIS2"), 120);
    assert_eq!(p.evidence_ttl_days_for("DORA"), 90);
    assert_eq!(p.evidence_ttl_days_for("SOC2"), 90); // falls back to default
}

#[test]
fn tier_allows_enact_only_for_listed_tiers() {
    let loaded = LoadedPolicy::parse(FULL_POLICY).unwrap();
    let p = &loaded.policy;
    assert!(p.tier_allows_enact(Some("service")));
    assert!(p.tier_allows_enact(Some("edge")));
    assert!(!p.tier_allows_enact(Some("data")));
    assert!(!p.tier_allows_enact(None));
}

#[test]
fn incomplete_playbook_entry_is_rejected() {
    let text =
        "[[autopilot.playbook]]\nplugin = \"tumult-net\"\naction = \"inject_latency\"\nexperiment = \"\"\n";
    let err = LoadedPolicy::parse(text).unwrap_err();
    assert!(
        matches!(err, PolicyError::IncompletePlaybook(0)),
        "got {err}"
    );

    // An empty plugin is just as incomplete, at its own index.
    let text =
        "[[autopilot.playbook]]\nplugin = \"tumult-net\"\naction = \"a\"\nexperiment = \"e\"\n\
                [[autopilot.playbook]]\nplugin = \"\"\naction = \"a\"\nexperiment = \"e\"\n";
    let err = LoadedPolicy::parse(text).unwrap_err();
    assert!(
        matches!(err, PolicyError::IncompletePlaybook(1)),
        "got {err}"
    );
}

#[test]
fn boundary_thresholds_are_accepted() {
    // 0.0 and 1.0 are both inside the documented 0.0..=1.0 range.
    for (threshold, expected_bits) in [("0.0", 0.0_f64.to_bits()), ("1.0", 1.0_f64.to_bits())] {
        let text = format!("[autopilot]\nautonomy_threshold = {threshold}\n");
        let loaded = LoadedPolicy::parse(&text).unwrap_or_else(|e| {
            panic!("threshold {threshold} must be accepted: {e}");
        });
        assert_eq!(loaded.policy.autonomy_threshold.to_bits(), expected_bits);
    }
}

#[test]
fn every_policy_error_variant_has_a_human_readable_message() {
    let parse = LoadedPolicy::parse("[autopilot]\nnope = true\n").unwrap_err();
    assert!(parse.to_string().contains("TOML parse error"), "{parse}");

    let threshold = LoadedPolicy::parse("[autopilot]\nautonomy_threshold = 1.5\n").unwrap_err();
    assert_eq!(
        threshold.to_string(),
        "autonomy_threshold 1.5 must be within 0.0..=1.0"
    );

    let samples = LoadedPolicy::parse("[autopilot]\nautonomy_min_samples = 0\n").unwrap_err();
    assert_eq!(
        samples.to_string(),
        "autonomy_min_samples must be at least 1"
    );

    let pretrust = LoadedPolicy::parse(
        "[[autopilot.pretrusted]]\nplugin = \"\"\naction = \"a\"\ntier = \"t\"\n",
    )
    .unwrap_err();
    assert_eq!(
        pretrust.to_string(),
        "pretrusted entry 0 has an empty plugin/action/tier"
    );

    let playbook = LoadedPolicy::parse(
        "[[autopilot.playbook]]\nplugin = \"p\"\naction = \"a\"\nexperiment = \"\"\n",
    )
    .unwrap_err();
    assert_eq!(
        playbook.to_string(),
        "playbook entry 0 has an empty plugin/action/experiment"
    );
}
