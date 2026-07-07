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
    assert_eq!(p.autonomy_min_samples, 3);
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
