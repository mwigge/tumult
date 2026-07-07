//! The deterministic safety gate: one pure function from `(policy,
//! candidate, ambient, autonomy, validator report)` to a verdict plus the
//! complete audit trail of every rule checked.
//!
//! # Verdict semantics
//!
//! * [`Verdict::Veto`] — a hard safety violation: autopilot disabled, open
//!   deviation on the target, another experiment already running, daily
//!   budget exhausted, or a hollow candidate (cannot falsify anything).
//!   Nothing downstream may run; the fired rule is named.
//! * [`Verdict::Propose`] — the candidate was never enact-eligible: the
//!   validator found enactability blockers (no playbook, no rollback,
//!   multi-fault). It joins the human approval queue as-is.
//! * [`Verdict::Downgrade`] — the candidate *would* enact but one or more
//!   bounded conditions failed (tier not enact-eligible, guard missing when
//!   required, telemetry pre-flight unverified, outside business hours,
//!   cooldown active, confidence only directional, autonomy not earned).
//!   Queued exactly like a proposal, but the reasons record what
//!   specifically blocked autonomy so the operator sees precisely what to
//!   change.
//! * [`Verdict::Enact`] — every rule passed; the engine may run.
//!
//! # Determinism
//!
//! The gate reads no clocks and holds no handles: every time- or
//! environment-shaped fact arrives pre-computed in
//! [`AmbientContext`](crate::candidate::AmbientContext). All rules are
//! evaluated in one fixed order ([`RULE_ORDER`]) with *no* short-circuiting,
//! so [`GateDecision::rules_evaluated`] always lists every rule with its
//! outcome — that vector is the audit record, and verdicts are
//! bit-reproducible from `(policy hash, inputs)`.

use serde::{Deserialize, Serialize};

use crate::candidate::{AmbientContext, AutonomyRecord, Candidate, ConfidenceTier};
use crate::ladder::{autonomy_earned, class_key};
use crate::policy::AutopilotPolicy;
use crate::validator::ValidatorReport;

/// Rule ids, veto class (any failure is a hard no).
pub const RULE_ENABLED: &str = "policy.enabled";
/// Never inject into an already-degraded target or its dependents.
pub const RULE_NO_OPEN_DEVIATION: &str = "ambient.no_open_deviation";
/// Global impact ledger: autopilot holds one fault at a time.
pub const RULE_NO_CONCURRENT: &str = "ambient.no_concurrent_experiment";
/// Daily run budget across all services.
pub const RULE_DAILY_BUDGET: &str = "budget.daily_runs_remaining";
/// Hollow candidates (cannot falsify) never run.
pub const RULE_NOT_HOLLOW: &str = "validator.not_hollow";
/// Enactability blockers cap the candidate at propose.
pub const RULE_ENACTABLE: &str = "validator.enactable";
/// Target tier must be listed in `enact_tiers`.
pub const RULE_TIER: &str = "tier.enact_allowed";
/// Guard must be present when policy requires one.
pub const RULE_GUARD_PRESENT: &str = "guard.present";
/// Guard telemetry pre-flight must have verified sight of the blast.
pub const RULE_TELEMETRY: &str = "guard.telemetry_verified";
/// Business-hours window, when policy restricts to it.
pub const RULE_BUSINESS_HOURS: &str = "schedule.business_hours";
/// Per-service cooldown between autopilot runs.
pub const RULE_COOLDOWN: &str = "cooldown.clear";
/// Enact requires high confidence.
pub const RULE_CONFIDENCE: &str = "confidence.high";
/// Enact requires earned (or pretrusted) autonomy for the fault class.
pub const RULE_AUTONOMY: &str = "autonomy.earned";

/// Every rule the gate checks, in evaluation order. The first five are the
/// veto class, the sixth is enactability (propose class), the rest are the
/// bounded (downgrade) class. This order is part of the audit contract:
/// `rules_evaluated` always matches it exactly.
pub const RULE_ORDER: [&str; 13] = [
    RULE_ENABLED,
    RULE_NO_OPEN_DEVIATION,
    RULE_NO_CONCURRENT,
    RULE_DAILY_BUDGET,
    RULE_NOT_HOLLOW,
    RULE_ENACTABLE,
    RULE_TIER,
    RULE_GUARD_PRESENT,
    RULE_TELEMETRY,
    RULE_BUSINESS_HOURS,
    RULE_COOLDOWN,
    RULE_CONFIDENCE,
    RULE_AUTONOMY,
];

/// The gate's answer for one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every rule passed; the engine may run the experiment.
    Enact,
    /// Would have enacted, but the named bounded conditions failed; queued
    /// for human approval with the exact blockers recorded.
    Downgrade {
        /// One human-readable reason per failed bounded condition.
        reasons: Vec<String>,
    },
    /// Never enact-eligible; queued for human approval.
    Propose {
        /// The validator's enactability blockers.
        reasons: Vec<String>,
    },
    /// Hard safety violation; nothing runs.
    Veto {
        /// The id of the rule that fired (see [`RULE_ORDER`]).
        rule: String,
    },
}

/// A verdict plus the complete, ordered audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateDecision {
    /// What the gate decided.
    pub verdict: Verdict,
    /// Every rule checked, `(rule id, passed)`, always the full
    /// [`RULE_ORDER`] in order — the audit record for the decision store.
    pub rules_evaluated: Vec<(String, bool)>,
}

/// One evaluated rule: id, outcome, and (when failed) the reason a human
/// should see.
struct Rule {
    id: &'static str,
    passed: bool,
    reason: Option<String>,
}

impl Rule {
    fn pass(id: &'static str) -> Self {
        Self {
            id,
            passed: true,
            reason: None,
        }
    }

    fn fail(id: &'static str, reason: String) -> Self {
        Self {
            id,
            passed: false,
            reason: Some(reason),
        }
    }

    fn check(id: &'static str, passed: bool, reason: impl FnOnce() -> String) -> Self {
        if passed {
            Self::pass(id)
        } else {
            Self::fail(id, reason())
        }
    }
}

/// Evaluate the gate. Deny by default: `Enact` requires every rule to pass;
/// verdict precedence is veto > propose > downgrade > enact. See the module
/// docs for the semantics and the determinism contract.
#[must_use]
pub fn evaluate(
    policy: &AutopilotPolicy,
    candidate: &Candidate,
    ambient: &AmbientContext,
    autonomy: Option<&AutonomyRecord>,
    validator: &ValidatorReport,
) -> GateDecision {
    let veto = veto_rules(policy, ambient, validator);
    let enactability = enactability_rule(validator);
    let bounded = bounded_rules(policy, candidate, ambient, autonomy);

    let verdict = if let Some(fired) = veto.iter().find(|rule| !rule.passed) {
        Verdict::Veto {
            rule: fired.id.to_string(),
        }
    } else if enactability.passed {
        let reasons: Vec<String> = bounded
            .iter()
            .filter(|rule| !rule.passed)
            .filter_map(|rule| rule.reason.clone())
            .collect();
        if reasons.is_empty() {
            Verdict::Enact
        } else {
            Verdict::Downgrade { reasons }
        }
    } else {
        Verdict::Propose {
            reasons: validator.blockers.clone(),
        }
    };

    let rules_evaluated = veto
        .into_iter()
        .chain(std::iter::once(enactability))
        .chain(bounded)
        .map(|rule| (rule.id.to_string(), rule.passed))
        .collect();

    GateDecision {
        verdict,
        rules_evaluated,
    }
}

/// Hard safety rules: any failure is a veto, first failure (in order) names
/// the fired rule.
fn veto_rules(
    policy: &AutopilotPolicy,
    ambient: &AmbientContext,
    validator: &ValidatorReport,
) -> Vec<Rule> {
    vec![
        Rule::check(RULE_ENABLED, policy.enabled, || {
            "autopilot is disabled by policy".to_string()
        }),
        Rule::check(
            RULE_NO_OPEN_DEVIATION,
            !ambient.open_deviation_for_target,
            || "target or a dependent has a recent open deviation".to_string(),
        ),
        Rule::check(
            RULE_NO_CONCURRENT,
            ambient.concurrent_experiments == 0,
            || {
                format!(
                    "{} experiment(s) already running — autopilot holds one fault at a time",
                    ambient.concurrent_experiments
                )
            },
        ),
        Rule::check(
            RULE_DAILY_BUDGET,
            ambient.runs_today < policy.max_runs_per_day,
            || {
                format!(
                    "daily budget exhausted: {} of {} runs used",
                    ambient.runs_today, policy.max_runs_per_day
                )
            },
        ),
        Rule::check(RULE_NOT_HOLLOW, validator.hollow.is_empty(), || {
            format!("hollow candidate: {}", validator.hollow.join("; "))
        }),
    ]
}

/// The propose-class rule: enactability blockers from the validator.
fn enactability_rule(validator: &ValidatorReport) -> Rule {
    Rule::check(RULE_ENACTABLE, validator.blockers.is_empty(), || {
        validator.blockers.join("; ")
    })
}

/// Bounded conditions: each failure downgrades an otherwise-enactable
/// candidate and tells the operator exactly what to change.
fn bounded_rules(
    policy: &AutopilotPolicy,
    candidate: &Candidate,
    ambient: &AmbientContext,
    autonomy: Option<&AutonomyRecord>,
) -> Vec<Rule> {
    let tier = candidate.tier.as_deref();
    let pretrusted = policy.is_pretrusted(&candidate.plugin, &candidate.action, tier);
    let (earned, autonomy_reason) = autonomy_earned(policy, autonomy, pretrusted);
    let class = class_key(&candidate.plugin, &candidate.action, tier);

    vec![
        Rule::check(RULE_TIER, policy.tier_allows_enact(tier), || match tier {
            Some(t) => format!("tier '{t}' is not in enact_tiers — capped at propose"),
            None => {
                "candidate has no tier — enact requires a tier listed in enact_tiers".to_string()
            }
        }),
        Rule::check(
            RULE_GUARD_PRESENT,
            !policy.require_guard || candidate.experiment_has_guard,
            || "experiment declares no guard and policy requires one".to_string(),
        ),
        Rule::check(
            RULE_TELEMETRY,
            ambient.guard_telemetry_ok == Some(true),
            || match ambient.guard_telemetry_ok {
                Some(false) => {
                    "guard telemetry pre-flight failed — the guard cannot observe the blast"
                        .to_string()
                }
                _ => "guard telemetry pre-flight not run — cannot confirm the guard observes \
                      the blast"
                    .to_string(),
            },
        ),
        Rule::check(
            RULE_BUSINESS_HOURS,
            !policy.business_hours_only || ambient.within_business_hours,
            || "outside business hours and policy allows enact only within them".to_string(),
        ),
        cooldown_rule(policy, candidate, ambient),
        Rule::check(
            RULE_CONFIDENCE,
            candidate.confidence == ConfidenceTier::High,
            || "confidence is directional — enact requires high confidence".to_string(),
        ),
        Rule::check(RULE_AUTONOMY, earned, || {
            format!("autonomy not earned for class {class}: {autonomy_reason}")
        }),
    ]
}

/// Per-service cooldown; `None` hours-since means no previous run, which is
/// always clear.
fn cooldown_rule(
    policy: &AutopilotPolicy,
    candidate: &Candidate,
    ambient: &AmbientContext,
) -> Rule {
    let required = f64::from(policy.cooldown_hours);
    match ambient.hours_since_last_run_on_service {
        Some(hours) if hours < required => Rule::fail(
            RULE_COOLDOWN,
            format!(
                "cooldown active: {hours:.1}h since the last autopilot run on {}, policy \
                 requires {required:.0}h",
                candidate.service_id
            ),
        ),
        _ => Rule::pass(RULE_COOLDOWN),
    }
}

// Behaviour tests live in `tests/gate.rs` and the replay corpus in
// `tests/replay.rs`; this module stays pure construction + selection logic.
