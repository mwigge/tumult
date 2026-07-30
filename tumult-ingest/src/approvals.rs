//! Risk-tier classification and the T3 autopilot gate for run approvals
//! (ADR-012).
//!
//! Every run requested via `POST /api/runs` is classified into exactly one
//! tier from its environment, its definition's shape (fault count, rollback
//! presence, destructive actions), and an optional T0 catalog of pre-approved
//! content hashes. Classification is pure and deterministic — same inputs,
//! same tier — and unit-tested exhaustively.
//!
//! - **T0**: probe-only definitions, or definitions whose content hash is in
//!   the daemon's pre-approved catalog. No approval; dispatch immediately.
//! - **T1**: standard experiments (single fault kind, rollback present,
//!   non-production/staging environment). One approver ≠ requester.
//! - **T2**: staging environments or destructive-named faults. One approver
//!   ≠ requester, shorter TTL.
//! - **T3**: production environments, definitions without rollback, or
//!   multiple distinct fault kinds. Quorum 2 (two distinct approvers, both
//!   ≠ requester), shortest TTL, and the tumult-autopilot gate must return
//!   [`Verdict::Enact`] — an approval NEVER overrides a `Veto`; only
//!   break-glass does (and it leaves a compliance-debt trail).

use tumult_autopilot::{
    evaluate, validate, AmbientContext, Candidate, ConfidenceTier, LoadedPolicy, Trigger, Verdict,
};
use tumult_core::types::{ActivityType, Experiment, Provider};

/// The risk tier of a run request. Ordered `T0 < T1 < T2 < T3` so
/// classification can take the maximum of all triggered rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    T0,
    T1,
    T2,
    T3,
}

impl Tier {
    /// The stored/serialized tier label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::T2 => "T2",
            Self::T3 => "T3",
        }
    }

    /// Approvals required to dispatch (distinct approvers, all ≠ requester).
    #[must_use]
    pub fn quorum_required(self) -> i64 {
        match self {
            Self::T3 => 2,
            Self::T0 | Self::T1 | Self::T2 => 1,
        }
    }

    /// Approval time-to-live in nanoseconds (ADR-012): an approval is only
    /// as fresh as the system state it was granted against. T3's 4h bounds a
    /// production change to roughly half an operations shift; T1's 72h
    /// covers weekend-adjacent requests; single-use consumption bounds replay
    /// regardless of TTL. T0 never expires (it never gates).
    #[must_use]
    pub fn ttl_ns(self) -> i64 {
        const HOUR_NS: i64 = 3_600_000_000_000;
        match self {
            Self::T1 => 72 * HOUR_NS,
            Self::T2 => 24 * HOUR_NS,
            Self::T3 => 4 * HOUR_NS,
            Self::T0 => 0,
        }
    }
}

/// Environment classes, matched case-insensitively on the env name: an
/// exact class word, or a class word followed by a separator (`-`, `_`,
/// `.`), so `prod`, `prod-eu`, `production_2` are production and `devprod`
/// is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvClass {
    Production,
    Staging,
    Other,
}

const PRODUCTION_WORDS: [&str; 3] = ["prod", "production", "live"];
const STAGING_WORDS: [&str; 6] = ["staging", "stage", "stg", "preprod", "pre-prod", "uat"];

fn matches_class_word(env: &str, word: &str) -> bool {
    env == word
        || env.strip_prefix(word).is_some_and(|rest| {
            rest.starts_with('-') || rest.starts_with('_') || rest.starts_with('.')
        })
}

/// Classify a target-environment name. Deterministic; documented in ADR-012.
#[must_use]
pub fn env_class(env: &str) -> EnvClass {
    let env = env.trim().to_lowercase();
    if PRODUCTION_WORDS.iter().any(|w| matches_class_word(&env, w)) {
        return EnvClass::Production;
    }
    if STAGING_WORDS.iter().any(|w| matches_class_word(&env, w)) {
        return EnvClass::Staging;
    }
    EnvClass::Other
}

/// Action-name fragments treated as destructive faults for tiering
/// (case-insensitive substring match on the activity name and the provider
/// function/path). This is a deliberately conservative *heuristic*: a
/// destructive fault whose name matches none of these falls to T1/T2 via the
/// other rules, never silently to T0 — T0 requires probe-only or an explicit
/// catalog entry.
const DESTRUCTIVE_FRAGMENTS: [&str; 8] = [
    "kill",
    "terminate",
    "delete",
    "destroy",
    "corrupt",
    "drop",
    "purge",
    "wipe",
];

/// The definition facts tiering and the T3 gate decide on, extracted from
/// the resolved [`Experiment`] once at request time.
#[derive(Debug, Clone)]
pub struct Introspection {
    /// Method has zero action steps (probes only).
    pub probe_only: bool,
    /// Definition declares at least one rollback activity.
    pub has_rollback: bool,
    /// Definition declares at least one guard.
    pub has_guard: bool,
    /// Definition declares a steady-state hypothesis.
    pub has_steady_state: bool,
    /// Method action steps (fault injections), counting repetitions.
    pub fault_count: usize,
    /// Distinct fault kinds: unique (plugin, function) / process-path pairs
    /// among method actions. Seven repetitions of the same sleep step are
    /// one fault kind; a kill plus a network partition are two.
    pub fault_kinds: usize,
    /// Any action matched [`DESTRUCTIVE_FRAGMENTS`].
    pub destructive: bool,
    /// (plugin, function) — or (process path, "") — of the first fault, for
    /// the T3 gate's candidate.
    pub first_fault: Option<(String, String)>,
}

/// Extract the tiering facts from a resolved experiment definition.
#[must_use]
pub fn introspect(experiment: &Experiment) -> Introspection {
    let actions: Vec<_> = experiment
        .method
        .iter()
        .filter(|a| a.activity_type == ActivityType::Action)
        .collect();
    let mut kinds: Vec<String> = Vec::new();
    let mut destructive = false;
    let mut first_fault = None;
    for action in &actions {
        let (owner, function) = match &action.provider {
            Provider::Native {
                plugin, function, ..
            }
            | Provider::Script {
                plugin, function, ..
            } => (plugin.clone(), function.clone()),
            Provider::Process { path, .. } => (path.clone(), String::new()),
        };
        let kind = format!("{owner}/{function}");
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        if first_fault.is_none() {
            first_fault = Some((owner.clone(), function.clone()));
        }
        let haystack = format!("{} {owner} {function}", action.name).to_lowercase();
        if DESTRUCTIVE_FRAGMENTS.iter().any(|f| haystack.contains(f)) {
            destructive = true;
        }
    }
    Introspection {
        probe_only: actions.is_empty(),
        has_rollback: !experiment.rollbacks.is_empty(),
        has_guard: !experiment.guards.is_empty(),
        has_steady_state: experiment.steady_state_hypothesis.is_some(),
        fault_count: actions.len(),
        fault_kinds: kinds.len(),
        destructive,
        first_fault,
    }
}

/// The inputs to tier classification. All facts are computed once at request
/// time and frozen into the approval request.
#[derive(Debug, Clone)]
pub struct TierInput {
    /// Target environment name (see [`env_class`]).
    pub env: String,
    /// Definition content hash is in the daemon's pre-approved T0 catalog.
    pub catalog_matched: bool,
    /// [`introspect`] of the resolved definition.
    pub introspection: Introspection,
}

/// Classify a run request into its risk tier. Every rule is evaluated; the
/// highest triggered tier wins. Rules (ADR-012):
///
/// | # | rule                                                        | tier |
/// |---|-------------------------------------------------------------|------|
/// | 1 | catalog hash match OR probe-only definition                 | T0   |
/// | 2 | production-class environment                                | T3   |
/// | 3 | faults present AND no rollback declared                     | T3   |
/// | 4 | more than one distinct fault kind                           | T3   |
/// | 5 | staging-class environment                                   | T2   |
/// | 6 | destructive-named fault                                     | T2   |
/// | 7 | otherwise (standard experiment)                             | T1   |
#[must_use]
pub fn classify(input: &TierInput) -> Tier {
    let mut tier = Tier::T1;
    let intro = &input.introspection;
    // T0 short-circuits: nothing else can raise it.
    if input.catalog_matched || intro.probe_only {
        return Tier::T0;
    }
    if env_class(&input.env) == EnvClass::Production {
        tier = tier.max(Tier::T3);
    }
    if intro.fault_count > 0 && !intro.has_rollback {
        tier = tier.max(Tier::T3);
    }
    if intro.fault_kinds > 1 {
        tier = tier.max(Tier::T3);
    }
    if env_class(&input.env) == EnvClass::Staging {
        tier = tier.max(Tier::T2);
    }
    if intro.destructive {
        tier = tier.max(Tier::T2);
    }
    tier
}

/// The outcome of the T3 autopilot gate. Anything other than
/// [`GateOutcome::Enact`] refuses the approval — the gate is fail-closed by
/// construction.
#[derive(Debug, Clone)]
pub enum GateOutcome {
    /// The gate returned [`Verdict::Enact`] — the approval may proceed.
    Enact {
        policy_hash: String,
        rules_evaluated: Vec<(String, bool)>,
    },
    /// The gate vetoed. An approval NEVER overrides a Veto; only break-glass
    /// does (with its compliance-debt trail).
    Veto { rule: String },
    /// Downgrade / Propose — not an enact verdict; fail closed.
    NotEnact { verdict: String },
    /// No policy is loaded on the daemon (the integration boundary —
    /// ADR-012); fail closed.
    Unavailable { reason: String },
}

/// Run the tumult-autopilot gate for a T3 approval. The candidate is
/// synthesized from the definition's introspection (fault count, rollback,
/// guard, steady-state are exactly the fields the gate's validator reads);
/// `ambient` is caller-frozen current-state facts, like the MCP engine's.
///
/// `policy` is `None` when the daemon has no `KRONIKA_AUTOPILOT_POLICY`
/// configured — the gate is then unavailable and the outcome is
/// fail-closed. `autonomy` is `None` for operator-requested runs: no
/// autonomy has been earned, and a policy that requires earned autonomy
/// will not Enact.
#[must_use]
pub fn evaluate_t3_gate(
    policy: Option<&LoadedPolicy>,
    run_id: &str,
    introspection: &Introspection,
    env: &str,
    target: Option<&str>,
    ambient: &AmbientContext,
) -> GateOutcome {
    let Some(policy) = policy else {
        return GateOutcome::Unavailable {
            reason: "no autopilot policy configured (KRONIKA_AUTOPILOT_POLICY)".into(),
        };
    };
    let (plugin, action) = introspection
        .first_fault
        .clone()
        .unwrap_or_else(|| ("run".into(), "run".into()));
    let candidate = Candidate {
        id: run_id.to_string(),
        service_id: target.unwrap_or(env).to_string(),
        tier: Some(env.to_string()),
        plugin,
        action,
        article_id: "run-approval".into(),
        score: 0.0,
        reasons: vec!["operator-requested run".into()],
        // An explicit operator request with a validated definition: not
        // evidence-scored, so claim the operator's confidence explicitly.
        confidence: ConfidenceTier::High,
        playbook_experiment: None,
        experiment_has_guard: introspection.has_guard,
        experiment_has_rollback: introspection.has_rollback,
        experiment_has_steady_state: introspection.has_steady_state,
        experiment_fault_count: introspection.fault_count,
        trigger: Trigger::Manual,
    };
    let validator = validate(&candidate);
    let decision = evaluate(&policy.policy, &candidate, ambient, None, &validator);
    match decision.verdict {
        Verdict::Enact => GateOutcome::Enact {
            policy_hash: policy.policy_hash().to_string(),
            rules_evaluated: decision.rules_evaluated,
        },
        Verdict::Veto { rule } => GateOutcome::Veto { rule },
        Verdict::Downgrade { .. } => GateOutcome::NotEnact {
            verdict: "downgrade".into(),
        },
        Verdict::Propose { .. } => GateOutcome::NotEnact {
            verdict: "propose".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_classification_is_exact_or_separator_prefixed() {
        assert_eq!(env_class("prod"), EnvClass::Production);
        assert_eq!(env_class("PROD-eu"), EnvClass::Production);
        assert_eq!(env_class("production_2"), EnvClass::Production);
        assert_eq!(env_class("live.eu-west"), EnvClass::Production);
        assert_eq!(env_class("staging"), EnvClass::Staging);
        assert_eq!(env_class("STAGE-1"), EnvClass::Staging);
        assert_eq!(env_class("pre-prod"), EnvClass::Staging);
        assert_eq!(env_class("uat.eu"), EnvClass::Staging);
        assert_eq!(env_class("dev"), EnvClass::Other);
        assert_eq!(env_class("devprod"), EnvClass::Other); // no separator
        assert_eq!(env_class("prodly"), EnvClass::Other);
        assert_eq!(env_class(" prod "), EnvClass::Production); // trimmed
        assert_eq!(env_class(""), EnvClass::Other);
    }

    fn intro(fault_kinds: usize, has_rollback: bool, destructive: bool) -> Introspection {
        Introspection {
            probe_only: fault_kinds == 0,
            has_rollback,
            has_guard: false,
            has_steady_state: true,
            fault_count: fault_kinds,
            fault_kinds,
            destructive,
            first_fault: None,
        }
    }

    fn input(env: &str, intro: Introspection) -> TierInput {
        TierInput {
            env: env.into(),
            catalog_matched: false,
            introspection: intro,
        }
    }

    #[test]
    fn probe_only_and_catalog_are_t0() {
        assert_eq!(classify(&input("prod", intro(0, false, false))), Tier::T0);
        let mut catalog = input("prod", intro(1, true, true));
        catalog.catalog_matched = true;
        assert_eq!(classify(&catalog), Tier::T0);
    }

    #[test]
    fn standard_experiment_is_t1() {
        assert_eq!(classify(&input("dev", intro(1, true, false))), Tier::T1);
    }

    #[test]
    fn staging_or_destructive_is_t2() {
        assert_eq!(classify(&input("staging", intro(1, true, false))), Tier::T2);
        assert_eq!(classify(&input("dev", intro(1, true, true))), Tier::T2);
    }

    #[test]
    fn production_no_rollback_multi_fault_are_t3() {
        assert_eq!(classify(&input("prod", intro(1, true, false))), Tier::T3);
        assert_eq!(classify(&input("dev", intro(1, false, false))), Tier::T3);
        assert_eq!(classify(&input("dev", intro(2, true, false))), Tier::T3);
    }

    #[test]
    fn highest_triggered_tier_wins() {
        // staging + destructive would be T2, but production pushes to T3.
        assert_eq!(classify(&input("prod", intro(1, true, true))), Tier::T3);
        // staging + multi-fault: T3 wins over T2.
        assert_eq!(classify(&input("uat", intro(2, true, false))), Tier::T3);
    }

    #[test]
    fn tier_metadata() {
        assert_eq!(Tier::T3.quorum_required(), 2);
        assert_eq!(Tier::T1.quorum_required(), 1);
        assert_eq!(Tier::T1.ttl_ns(), 72 * 3_600_000_000_000);
        assert_eq!(Tier::T2.ttl_ns(), 24 * 3_600_000_000_000);
        assert_eq!(Tier::T3.ttl_ns(), 4 * 3_600_000_000_000);
        assert_eq!(Tier::T0.ttl_ns(), 0);
    }

    #[test]
    fn introspection_counts_kinds_and_flags() {
        let toon = r#"
title: multi-step experiment
steady_state_hypothesis:
  title: svc up
  probes[1]:
    - name: health
      activity_type: probe
      provider:
        type: process
        path: /bin/true
method[4]:
  - name: sleep a
    activity_type: action
    provider:
      type: process
      path: /bin/sleep
  - name: sleep b
    activity_type: action
    provider:
      type: process
      path: /bin/sleep
  - name: kill the thing
    activity_type: action
    provider:
      type: native
      plugin: chaos
      function: process_kill
  - name: check
    activity_type: probe
    provider:
      type: process
      path: /bin/true
rollbacks[1]:
  - name: undo
    activity_type: action
    provider:
      type: process
      path: /bin/true
"#;
        let experiment = tumult_core::engine::parse_experiment(toon).unwrap();
        let intro = introspect(&experiment);
        assert_eq!(intro.fault_count, 3);
        assert_eq!(intro.fault_kinds, 2); // /bin/sleep + chaos/process_kill
        assert!(!intro.probe_only);
        assert!(intro.has_rollback);
        assert!(intro.has_steady_state);
        assert!(intro.destructive); // "kill the thing" / process_kill
        assert_eq!(
            intro.first_fault,
            Some(("/bin/sleep".into(), String::new()))
        );

        let probe_only_toon = r#"
title: observe only
method[1]:
  - name: look
    activity_type: probe
    provider:
      type: process
      path: /bin/true
"#;
        let experiment = tumult_core::engine::parse_experiment(probe_only_toon).unwrap();
        let intro = introspect(&experiment);
        assert!(intro.probe_only);
        assert_eq!(intro.fault_count, 0);
        assert!(!intro.has_rollback);
        assert!(!intro.destructive);
    }

    #[test]
    fn gate_is_fail_closed_without_a_policy() {
        let ambient = AmbientContext {
            open_deviation_for_target: false,
            runs_today: 0,
            hours_since_last_run_on_service: None,
            within_business_hours: true,
            concurrent_experiments: 0,
            guard_telemetry_ok: None,
        };
        let outcome = evaluate_t3_gate(
            None,
            "run-1",
            &intro(1, true, false),
            "prod",
            None,
            &ambient,
        );
        assert!(matches!(outcome, GateOutcome::Unavailable { .. }));
    }
}
