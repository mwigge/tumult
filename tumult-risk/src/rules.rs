//! The rules. Each one is a pure function from [`Inputs`] to zero or more
//! unranked [`Risk`]s; `lib.rs` ranks, sorts and applies suppressions.
//!
//! Rule names are stable identifiers (they are part of every risk id):
//!
//! | rule | fires when |
//! |---|---|
//! | `discipline_gap` | a target has never been exercised in chaos, load or dr |
//! | `stale_evidence` | a target's latest evidence in a discipline is older than 30 days (90: high) |
//! | `objective_breached` | the latest run of an experiment breached an objective |
//! | `recovery_target_missed` | the latest dr run measured an RTO above its target |
//! | `latest_run_failed` | the latest run of an experiment did not pass (and no breached objective already explains it) |
//! | `untested_dependency` | a service other services call has no experiments at all |
//! | `never_run_definition` | a registered definition has never run |
//! | `manual_exercise_overdue` | a planned manual exercise is past its due date (more than 30 days: high) |
//! | `single_replica` | a `Deployment` or `StatefulSet` manifest runs one replica |
//! | `no_resource_limits` | a container has no resource limits |
//! | `no_pod_disruption_budget` | no `PodDisruptionBudget` selects the workload |
//! | `no_readiness_probe` | a container has no readiness probe |

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use crate::model::{
    Inputs, Risk, Severity, TargetEvidence, Workload, DAY_NS, STALE_AFTER_NS, VERY_STALE_AFTER_NS,
};

/// Disciplines whose absence is a risk in itself. The other four only show
/// in the coverage matrix.
const CORE_KINDS: [&str; 3] = ["chaos", "performance", "robustness"];

/// Stable id: `r_` plus the first 16 hex chars of
/// SHA-256(rule | target | environment | subject).
#[must_use]
pub fn risk_id(
    rule: &str,
    target: Option<&str>,
    environment: Option<&str>,
    subject: &str,
) -> String {
    let mut h = Sha256::new();
    h.update(rule.as_bytes());
    h.update(b"|");
    h.update(target.unwrap_or("").as_bytes());
    h.update(b"|");
    h.update(environment.unwrap_or("").as_bytes());
    h.update(b"|");
    h.update(subject.as_bytes());
    let digest = h.finalize();
    let mut id = String::with_capacity(18);
    id.push_str("r_");
    for b in digest.iter().take(8) {
        write!(id, "{b:02x}").expect("writing to a String cannot fail");
    }
    id
}

fn is_critical(t: &TargetEvidence) -> bool {
    matches!(
        t.criticality
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("critical" | "high")
    )
}

fn days(ns: i64) -> i64 {
    ns / DAY_NS
}

struct Draft<'a> {
    rule: &'static str,
    severity: Severity,
    title: String,
    detail: String,
    target: Option<&'a str>,
    environment: Option<&'a str>,
    subject: String,
    recommended_kind: Option<&'static str>,
    evidence: Vec<String>,
}

fn build(d: Draft<'_>) -> Risk {
    Risk {
        id: risk_id(d.rule, d.target, d.environment, &d.subject),
        rule: d.rule.to_string(),
        severity: d.severity,
        rank: d.severity.base_rank(),
        title: d.title,
        detail: d.detail,
        target: d.target.map(str::to_owned),
        environment: d.environment.map(str::to_owned),
        subject: d.subject,
        recommended_kind: d.recommended_kind.map(str::to_owned),
        evidence: d.evidence,
        suppression: None,
    }
}

fn discipline_gaps(t: &TargetEvidence, out: &mut Vec<Risk>) {
    for kind in CORE_KINDS {
        if t.kinds.iter().any(|k| k.kind == kind) {
            continue;
        }
        let severity = match kind {
            "chaos" => Severity::High,
            "robustness" if is_critical(t) => Severity::High,
            _ => Severity::Medium,
        };
        out.push(build(Draft {
            rule: "discipline_gap",
            severity,
            title: format!("{} has no {kind} evidence", t.target),
            detail: format!(
                "No {kind} experiment has ever run against {} in {}. Its resilience in this \
                 discipline is unknown.",
                t.target,
                t.environment.as_deref().unwrap_or("any environment")
            ),
            target: Some(&t.target),
            environment: t.environment.as_deref(),
            subject: kind.to_string(),
            recommended_kind: Some(kind),
            evidence: vec![],
        }));
    }
}

fn stale_evidence(t: &TargetEvidence, as_of_ns: i64, out: &mut Vec<Risk>) {
    for k in &t.kinds {
        let age = as_of_ns - k.last_run_ns;
        if age <= STALE_AFTER_NS {
            continue;
        }
        let severity = if age > VERY_STALE_AFTER_NS {
            Severity::High
        } else {
            Severity::Medium
        };
        let kind: &'static str = crate::model::KINDS
            .iter()
            .find(|x| **x == k.kind)
            .copied()
            .unwrap_or("chaos");
        let mut risk = build(Draft {
            rule: "stale_evidence",
            severity,
            title: format!("{} {} evidence is {} days old", t.target, k.kind, days(age)),
            detail: format!(
                "The latest {} run against {} finished {} days ago ({} run(s) on record). \
                 Evidence older than 30 days no longer supports the score.",
                k.kind,
                t.target,
                days(age),
                k.runs
            ),
            target: Some(&t.target),
            environment: t.environment.as_deref(),
            subject: k.kind.clone(),
            recommended_kind: Some(kind),
            evidence: vec![format!("last_run_ns={}", k.last_run_ns)],
        });
        // Older evidence ranks higher, up to five points.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bump = (days(age) / 30).clamp(0, 5) as u32;
        risk.rank += bump;
        out.push(risk);
    }
}

fn experiment_rules(t: &TargetEvidence, out: &mut Vec<Risk>) {
    for e in &t.experiments {
        let mut breached = false;
        for v in e.verdicts.iter().filter(|v| v.state == "breached") {
            breached = true;
            let severity = match v.criticality.as_str() {
                "blocking" => Severity::High,
                "major" => Severity::Medium,
                _ => Severity::Low,
            };
            let observed = v
                .observed
                .map_or_else(|| "no observation".to_string(), |o| format!("{o}"));
            out.push(build(Draft {
                rule: "objective_breached",
                severity,
                title: format!("{} breached objective {}", e.name, v.nfr_id),
                detail: format!(
                    "The latest run of {} observed {observed} against a threshold of {} \
                     ({} objective).",
                    e.name, v.threshold, v.criticality
                ),
                target: Some(&t.target),
                environment: t.environment.as_deref(),
                subject: format!("{}/{}", e.name, v.nfr_id),
                recommended_kind: e.kind.as_deref().and_then(kind_const),
                evidence: e.evidence_id.iter().cloned().collect(),
            }));
        }
        if let (Some(target_s), Some(measured_s)) = (e.rto_target_s, e.rto_measured_s) {
            if measured_s > target_s {
                out.push(build(Draft {
                    rule: "recovery_target_missed",
                    severity: if is_critical(t) {
                        Severity::Critical
                    } else {
                        Severity::High
                    },
                    title: format!(
                        "{} recovered in {measured_s:.1}s against an RTO of {target_s:.1}s",
                        e.name
                    ),
                    detail: format!(
                        "The latest disaster-recovery run of {} measured a recovery time of \
                         {measured_s:.1}s; the declared recovery time objective is {target_s:.1}s.",
                        e.name
                    ),
                    target: Some(&t.target),
                    environment: t.environment.as_deref(),
                    subject: format!("{}/rto", e.name),
                    recommended_kind: Some("dr"),
                    evidence: e.evidence_id.iter().cloned().collect(),
                }));
            }
        }
        if !e.passed && !breached {
            out.push(build(Draft {
                rule: "latest_run_failed",
                severity: Severity::Medium,
                title: format!(
                    "{} {} on its latest run",
                    e.name,
                    e.outcome.as_deref().unwrap_or("did not complete")
                ),
                detail: format!(
                    "The latest run of {} against {} ended {}. The score counts it as a failure \
                     until a passing run replaces it.",
                    e.name,
                    t.target,
                    e.outcome.as_deref().unwrap_or("without an outcome")
                ),
                target: Some(&t.target),
                environment: t.environment.as_deref(),
                subject: e.name.clone(),
                recommended_kind: e.kind.as_deref().and_then(kind_const),
                evidence: e.evidence_id.iter().cloned().collect(),
            }));
        }
    }
}

fn kind_const(kind: &str) -> Option<&'static str> {
    crate::model::KINDS.iter().find(|k| **k == kind).copied()
}

fn untested_dependencies(inputs: &Inputs, out: &mut Vec<Risk>) {
    for d in &inputs.dependencies {
        let tested = inputs
            .targets
            .iter()
            .any(|t| t.target.eq_ignore_ascii_case(&d.service) && !t.kinds.is_empty());
        if tested {
            continue;
        }
        out.push(build(Draft {
            rule: "untested_dependency",
            severity: Severity::Medium,
            title: format!(
                "{} is called by {} service(s) but never tested",
                d.service, d.callers
            ),
            detail: format!(
                "Telemetry shows {} calls into {} from {} caller(s), yet no test targets it. \
                 A failure there propagates to every caller untested.",
                d.calls, d.service, d.callers
            ),
            target: Some(&d.service),
            environment: None,
            subject: "dependency".to_string(),
            recommended_kind: Some("chaos"),
            evidence: vec![
                format!("callers={}", d.callers),
                format!("calls={}", d.calls),
            ],
        }));
    }
}

fn never_run(inputs: &Inputs, out: &mut Vec<Risk>) {
    for d in inputs.definitions.iter().filter(|d| d.runs == 0) {
        out.push(build(Draft {
            rule: "never_run_definition",
            severity: Severity::Low,
            title: format!("{} is registered but has never run", d.name),
            detail: format!(
                "Definition {} ({}) sits in the registry with no runs. It produces no evidence.",
                d.name, d.id
            ),
            target: None,
            environment: None,
            subject: d.id.clone(),
            recommended_kind: None,
            evidence: vec![format!("registry_id={}", d.id)],
        }));
    }
}

fn overdue_exercises(inputs: &Inputs, out: &mut Vec<Risk>) {
    for p in inputs
        .plans
        .iter()
        .filter(|p| p.due_at_ns < inputs.as_of_ns)
    {
        let late = inputs.as_of_ns - p.due_at_ns;
        let severity = if late > STALE_AFTER_NS {
            Severity::High
        } else {
            Severity::Medium
        };
        let target = p.target.as_deref();
        let mut risk = build(Draft {
            rule: "manual_exercise_overdue",
            severity,
            title: format!("{} exercise {} is overdue", p.exercise_type, p.name),
            detail: format!(
                "The planned {} exercise {} against {} was due {} day(s) ago and no verified \
                 record fulfils it. Its owner is {}.",
                p.exercise_type,
                p.name,
                target.unwrap_or("no named target"),
                days(late),
                p.owner
            ),
            target,
            environment: p.environment.as_deref(),
            subject: p.id.clone(),
            recommended_kind: None,
            evidence: vec![
                format!("plan_id={}", p.id),
                format!("due_at_ns={}", p.due_at_ns),
            ],
        });
        // Longer overdue ranks higher, up to five points, as stale evidence.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bump = (days(late) / 30).clamp(0, 5) as u32;
        risk.rank += bump;
        out.push(risk);
    }
}

/// The label a manifest uses to say which environment it deploys to.
const ENVIRONMENT_LABEL: &str = "resilience.target.environment";

/// The target a workload stands for: its name, or its `app` /
/// `app.kubernetes.io/name` label, when either names a known target. One
/// name can be a target in several environments: the workload's
/// environment label picks among them, and without a match the first
/// target of that name stands in.
fn workload_target<'a>(w: &Workload, inputs: &'a Inputs) -> Option<&'a TargetEvidence> {
    let candidates = [
        Some(w.name.as_str()),
        w.labels.get("app").map(String::as_str),
        w.labels.get("app.kubernetes.io/name").map(String::as_str),
    ];
    let named = |t: &&TargetEvidence| {
        candidates
            .iter()
            .flatten()
            .any(|c| c.eq_ignore_ascii_case(&t.target))
    };
    let environment = w.labels.get(ENVIRONMENT_LABEL).map(String::as_str);
    let same_environment = inputs
        .targets
        .iter()
        .filter(named)
        .find(|t| environment.is_some() && t.environment.as_deref() == environment);
    same_environment.or_else(|| inputs.targets.iter().find(named))
}

/// Rank points a target's criticality adds to every finding against it.
fn criticality_bonus(t: &TargetEvidence) -> u32 {
    match t
        .criticality
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("critical") => 8,
        Some("high") => 5,
        Some("medium") => 2,
        _ => 0,
    }
}

/// The target a finding is filed against: the same name in the same
/// environment, so a critical production system does not lift the findings
/// of its namesake elsewhere.
fn target_of<'a>(r: &Risk, inputs: &'a Inputs) -> Option<&'a TargetEvidence> {
    let name = r.target.as_deref()?;
    inputs
        .targets
        .iter()
        .find(|t| t.target == name && t.environment == r.environment)
}

fn workload_rules(inputs: &Inputs, out: &mut Vec<Risk>) {
    for w in &inputs.workloads {
        let linked = workload_target(w, inputs);
        let target = linked.map(|t| t.target.as_str());
        let environment = linked.and_then(|t| t.environment.as_deref());
        let subject = w.qualified_name();
        let evidence = vec![format!("manifest={}", w.source)];
        let scalable = matches!(w.kind.as_str(), "Deployment" | "StatefulSet");
        if scalable && w.replicas.is_some_and(|r| r <= 1) {
            out.push(build(Draft {
                rule: "single_replica",
                severity: Severity::High,
                title: format!("{subject} runs a single replica"),
                detail: format!(
                    "{} {subject} declares {} replica(s); one pod eviction or node failure takes \
                     it down entirely.",
                    w.kind,
                    w.replicas.unwrap_or(0)
                ),
                target,
                environment,
                subject: subject.clone(),
                recommended_kind: Some("chaos"),
                evidence: evidence.clone(),
            }));
        }
        let unlimited: Vec<&str> = w
            .containers
            .iter()
            .filter(|c| !c.has_limits)
            .map(|c| c.name.as_str())
            .collect();
        if !unlimited.is_empty() {
            out.push(build(Draft {
                rule: "no_resource_limits",
                severity: Severity::Medium,
                title: format!("{subject} has containers without resource limits"),
                detail: format!(
                    "Container(s) {} in {subject} set no CPU or memory limits; a leak or a burst \
                     can starve every neighbour on the node.",
                    unlimited.join(", ")
                ),
                target,
                environment,
                subject: subject.clone(),
                recommended_kind: Some("performance"),
                evidence: evidence.clone(),
            }));
        }
        if scalable && !inputs.budgets.iter().any(|b| b.covers(w)) {
            out.push(build(Draft {
                rule: "no_pod_disruption_budget",
                severity: Severity::Medium,
                title: format!("{subject} has no PodDisruptionBudget"),
                detail: format!(
                    "No PodDisruptionBudget selects {subject}; a node drain or cluster upgrade may \
                     evict every replica at once."
                ),
                target,
                environment,
                subject: subject.clone(),
                recommended_kind: Some("chaos"),
                evidence: evidence.clone(),
            }));
        }
        let unprobed: Vec<&str> = w
            .containers
            .iter()
            .filter(|c| !c.has_readiness_probe)
            .map(|c| c.name.as_str())
            .collect();
        if !unprobed.is_empty() {
            out.push(build(Draft {
                rule: "no_readiness_probe",
                severity: Severity::Low,
                title: format!("{subject} has containers without a readiness probe"),
                detail: format!(
                    "Container(s) {} in {subject} declare no readiness probe; traffic reaches them \
                     before they can serve it and keeps reaching them when they cannot.",
                    unprobed.join(", ")
                ),
                target,
                environment,
                subject: subject.clone(),
                recommended_kind: Some("robustness"),
                evidence,
            }));
        }
    }
}

/// Every rule against every input, unranked and unsorted.
#[must_use]
pub fn evaluate(inputs: &Inputs) -> Vec<Risk> {
    let mut out = Vec::new();
    for t in &inputs.targets {
        discipline_gaps(t, &mut out);
        stale_evidence(t, inputs.as_of_ns, &mut out);
        experiment_rules(t, &mut out);
    }
    untested_dependencies(inputs, &mut out);
    never_run(inputs, &mut out);
    overdue_exercises(inputs, &mut out);
    workload_rules(inputs, &mut out);
    // Target criticality lifts every finding against that target.
    for r in &mut out {
        let bonus = target_of(r, inputs).map_or(0, criticality_bonus);
        r.rank = (r.rank + bonus).min(100);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{
        Container, Definition, Dependency, DisruptionBudget, ExperimentLatest, KindEvidence,
        PlannedExercise, Verdict,
    };

    const NOW: i64 = 2000 * DAY_NS;

    fn evidence(kind: &str, age_days: i64) -> KindEvidence {
        KindEvidence {
            kind: kind.into(),
            runs: 3,
            last_run_ns: NOW - age_days * DAY_NS,
            last_outcome: Some("Completed".into()),
        }
    }

    fn target(name: &str, criticality: Option<&str>, kinds: Vec<KindEvidence>) -> TargetEvidence {
        TargetEvidence {
            target: name.into(),
            environment: Some("prod".into()),
            criticality: criticality.map(str::to_owned),
            kinds,
            experiments: vec![],
        }
    }

    fn rules_of(risks: &[Risk]) -> Vec<&str> {
        risks.iter().map(|r| r.rule.as_str()).collect()
    }

    #[test]
    fn risk_ids_are_stable_and_distinct() {
        let a = risk_id("discipline_gap", Some("api"), Some("prod"), "chaos");
        let b = risk_id("discipline_gap", Some("api"), Some("prod"), "chaos");
        let c = risk_id("discipline_gap", Some("api"), Some("prod"), "robustness");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("r_") && a.len() == 18, "{a}");
    }

    #[test]
    fn criticality_bonus_follows_the_environment() {
        let mut demo = target("pay", None, vec![]);
        demo.environment = Some("demo".into());
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target("pay", Some("critical"), vec![]), demo],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let rank = |env: &str| {
            risks
                .iter()
                .find(|r| r.subject == "chaos" && r.environment.as_deref() == Some(env))
                .map(|r| r.rank)
        };
        let base = Severity::High.base_rank();
        assert_eq!(rank("prod"), Some(base + 8));
        assert_eq!(rank("demo"), Some(base), "demo is not critical");
    }

    #[test]
    fn workload_links_to_the_target_in_its_labelled_environment() {
        let mut demo = target("api", None, vec![evidence("chaos", 1)]);
        demo.environment = Some("demo".into());
        let mut labelled = workload("api", "Deployment", Some(1), vec![]);
        labelled.labels.insert(
            "resilience.target.environment".to_string(),
            "prod".to_string(),
        );
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![demo, target("api", None, vec![evidence("chaos", 1)])],
            workloads: vec![labelled],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let single = risks.iter().find(|r| r.rule == "single_replica").unwrap();
        assert_eq!(single.target.as_deref(), Some("api"));
        assert_eq!(single.environment.as_deref(), Some("prod"));
    }

    #[test]
    fn missing_core_disciplines_are_gaps_and_only_core_ones() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target("api", None, vec![evidence("chaos", 1)])],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let gaps: Vec<&Risk> = risks
            .iter()
            .filter(|r| r.rule == "discipline_gap")
            .collect();
        let subjects: Vec<&str> = gaps.iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(subjects, vec!["performance", "robustness"]);
        assert!(gaps
            .iter()
            .all(|r| r.recommended_kind.as_deref() == Some(r.subject.as_str())));
        assert_eq!(
            gaps[1].severity,
            Severity::Medium,
            "dr gap on an unclassified target"
        );
    }

    #[test]
    fn chaos_gap_is_high_and_dr_gap_escalates_for_critical_targets() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target("pay", Some("critical"), vec![])],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let by_subject = |s: &str| risks.iter().find(|r| r.subject == s).unwrap();
        assert_eq!(by_subject("chaos").severity, Severity::High);
        assert_eq!(by_subject("robustness").severity, Severity::High);
        assert_eq!(by_subject("performance").severity, Severity::Medium);
        // Criticality bonus: High base 70 + 8.
        assert_eq!(by_subject("chaos").rank, 78);
    }

    #[test]
    fn stale_evidence_escalates_with_age() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target(
                "api",
                None,
                vec![
                    evidence("chaos", 45),
                    evidence("performance", 120),
                    evidence("robustness", 3),
                ],
            )],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let stale: Vec<&Risk> = risks
            .iter()
            .filter(|r| r.rule == "stale_evidence")
            .collect();
        assert_eq!(stale.len(), 2);
        let chaos = stale.iter().find(|r| r.subject == "chaos").unwrap();
        let load = stale.iter().find(|r| r.subject == "performance").unwrap();
        assert_eq!(chaos.severity, Severity::Medium);
        assert_eq!(chaos.rank, 45 + 1);
        assert_eq!(load.severity, Severity::High);
        assert_eq!(load.rank, 70 + 4);
        assert!(chaos.title.contains("45 days old"), "{}", chaos.title);
        assert!(!rules_of(&risks).contains(&"discipline_gap"));
    }

    fn experiment(name: &str, passed: bool, verdicts: Vec<Verdict>) -> ExperimentLatest {
        ExperimentLatest {
            name: name.into(),
            kind: Some("robustness".into()),
            last_run_ns: NOW - DAY_NS,
            passed,
            outcome: Some(if passed { "Completed" } else { "Deviated" }.into()),
            evidence_id: Some("run-1".into()),
            verdicts,
            rto_target_s: None,
            rto_measured_s: None,
        }
    }

    fn verdict(id: &str, state: &str, criticality: &str) -> Verdict {
        Verdict {
            nfr_id: id.into(),
            kind: "availability".into(),
            state: state.into(),
            criticality: criticality.into(),
            threshold: 0.99,
            observed: Some(0.9),
            margin: Some(-0.09),
        }
    }

    #[test]
    fn breached_objectives_rank_by_criticality_and_explain_the_failure() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments.push(experiment(
            "failover",
            false,
            vec![
                verdict("avail", "breached", "blocking"),
                verdict("p95", "breached", "minor"),
                verdict("ok", "met", "major"),
            ],
        ));
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let breached: Vec<&Risk> = risks
            .iter()
            .filter(|r| r.rule == "objective_breached")
            .collect();
        assert_eq!(breached.len(), 2);
        assert_eq!(breached[0].severity, Severity::High);
        assert_eq!(breached[0].subject, "failover/avail");
        assert_eq!(breached[1].severity, Severity::Low);
        assert_eq!(breached[0].evidence, vec!["run-1".to_string()]);
        // The next test to run is of the discipline that breached.
        assert_eq!(breached[0].recommended_kind.as_deref(), Some("robustness"));
        // The deviation is explained by the breach: no duplicate failed-run risk.
        assert!(!rules_of(&risks).contains(&"latest_run_failed"));
    }

    #[test]
    fn unexplained_failed_run_is_its_own_risk() {
        let mut t = target("db", None, vec![evidence("chaos", 1)]);
        t.experiments
            .push(experiment("kill-primary", false, vec![]));
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let failed = risks
            .iter()
            .find(|r| r.rule == "latest_run_failed")
            .unwrap();
        assert_eq!(failed.severity, Severity::Medium);
        assert!(failed.title.contains("Deviated"), "{}", failed.title);
        assert_eq!(failed.recommended_kind.as_deref(), Some("robustness"));
    }

    /// A manual record of a type the house does not count as a test has
    /// no discipline: its findings stay and recommend none.
    #[test]
    fn a_failed_record_with_no_discipline_recommends_none() {
        let mut t = target("db", None, vec![]);
        let mut plain = experiment("tabletop", false, vec![]);
        plain.kind = None;
        let mut breaching = experiment(
            "walkthrough",
            false,
            vec![verdict("avail", "breached", "major")],
        );
        breaching.kind = None;
        t.experiments.extend([plain, breaching]);
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let of = |rule: &str| risks.iter().find(|r| r.rule == rule).unwrap();
        assert_eq!(of("latest_run_failed").subject, "tabletop");
        assert_eq!(of("latest_run_failed").recommended_kind, None);
        assert_eq!(of("objective_breached").subject, "walkthrough/avail");
        assert_eq!(of("objective_breached").recommended_kind, None);
    }

    #[test]
    fn missed_recovery_target_is_high_or_critical() {
        let mut t = target("db", Some("critical"), vec![evidence("robustness", 1)]);
        let mut e = experiment("failover", true, vec![]);
        e.rto_target_s = Some(30.0);
        e.rto_measured_s = Some(42.5);
        t.experiments.push(e);
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let missed = risks
            .iter()
            .find(|r| r.rule == "recovery_target_missed")
            .unwrap();
        assert_eq!(missed.severity, Severity::Critical);
        assert_eq!(missed.rank, 98);
        assert!(missed.title.contains("42.5s"), "{}", missed.title);
        assert_eq!(missed.recommended_kind.as_deref(), Some("dr"));
    }

    #[test]
    fn met_recovery_target_is_not_a_risk() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        let mut e = experiment("failover", true, vec![]);
        e.rto_target_s = Some(30.0);
        e.rto_measured_s = Some(12.0);
        t.experiments.push(e);
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        assert!(!rules_of(&evaluate(&inputs)).contains(&"recovery_target_missed"));
    }

    #[test]
    fn dependencies_without_experiments_are_flagged() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target("API", None, vec![evidence("chaos", 1)])],
            dependencies: vec![
                Dependency {
                    service: "api".into(),
                    callers: 2,
                    calls: 900,
                },
                Dependency {
                    service: "ledger".into(),
                    callers: 3,
                    calls: 400,
                },
            ],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let untested: Vec<&Risk> = risks
            .iter()
            .filter(|r| r.rule == "untested_dependency")
            .collect();
        assert_eq!(untested.len(), 1);
        assert_eq!(untested[0].target.as_deref(), Some("ledger"));
        assert_eq!(untested[0].recommended_kind.as_deref(), Some("chaos"));
    }

    #[test]
    fn registered_but_never_run_definitions_are_low() {
        let inputs = Inputs {
            as_of_ns: NOW,
            definitions: vec![
                Definition {
                    id: "reg-1".into(),
                    name: "pg-failover".into(),
                    runs: 4,
                },
                Definition {
                    id: "reg-2".into(),
                    name: "cache-stampede".into(),
                    runs: 0,
                },
            ],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].rule, "never_run_definition");
        assert_eq!(risks[0].severity, Severity::Low);
        assert_eq!(risks[0].subject, "reg-2");
    }

    fn workload(
        name: &str,
        kind: &str,
        replicas: Option<i64>,
        containers: Vec<Container>,
    ) -> Workload {
        Workload {
            name: name.into(),
            namespace: Some("prod".into()),
            kind: kind.into(),
            replicas,
            labels: BTreeMap::from([("app".to_string(), name.to_string())]),
            containers,
            source: format!("{name}.yaml"),
        }
    }

    fn container(name: &str, has_limits: bool, has_readiness_probe: bool) -> Container {
        Container {
            name: name.into(),
            has_limits,
            has_readiness_probe,
        }
    }

    #[test]
    fn workload_rules_fire_and_link_to_targets() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target(
                "api",
                Some("high"),
                vec![
                    evidence("chaos", 1),
                    evidence("performance", 1),
                    evidence("robustness", 1),
                ],
            )],
            workloads: vec![
                workload(
                    "api",
                    "Deployment",
                    Some(1),
                    vec![container("api", false, false)],
                ),
                workload(
                    "worker",
                    "DaemonSet",
                    None,
                    vec![container("w", true, true)],
                ),
            ],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let api: Vec<&Risk> = risks.iter().filter(|r| r.subject == "prod/api").collect();
        let rules: Vec<&str> = api.iter().map(|r| r.rule.as_str()).collect();
        assert_eq!(
            rules,
            vec![
                "single_replica",
                "no_resource_limits",
                "no_pod_disruption_budget",
                "no_readiness_probe"
            ]
        );
        assert!(api.iter().all(|r| r.target.as_deref() == Some("api")));
        assert!(api.iter().all(|r| r.environment.as_deref() == Some("prod")));
        // High base 70 + high-criticality bonus 5.
        assert_eq!(api[0].rank, 75);
        // The DaemonSet is healthy and has no replica or budget rules.
        assert!(risks.iter().all(|r| r.subject != "prod/worker"));
    }

    #[test]
    fn a_covering_budget_silences_the_pdb_rule() {
        let inputs = Inputs {
            as_of_ns: NOW,
            workloads: vec![workload(
                "api",
                "StatefulSet",
                Some(3),
                vec![container("api", true, true)],
            )],
            budgets: vec![DisruptionBudget {
                name: "api-pdb".into(),
                namespace: Some("prod".into()),
                selector: BTreeMap::from([("app".to_string(), "api".to_string())]),
            }],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        assert!(risks.is_empty(), "{risks:?}");
    }

    #[test]
    fn healthy_inputs_produce_no_risks() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target(
                "api",
                None,
                vec![
                    evidence("chaos", 1),
                    evidence("performance", 2),
                    evidence("robustness", 3),
                ],
            )],
            ..Default::default()
        };
        assert!(evaluate(&inputs).is_empty());
    }

    /// A plan of a failover of `db` in `prod`, due `days_ago` days before now.
    fn plan(id: &str, days_ago: i64) -> PlannedExercise {
        PlannedExercise {
            id: id.into(),
            name: "db failover".into(),
            exercise_type: "failover".into(),
            target: Some("db".into()),
            environment: Some("prod".into()),
            owner: "alice".into(),
            due_at_ns: NOW - days_ago * DAY_NS,
        }
    }

    #[test]
    fn a_plan_past_its_due_date_is_overdue_and_weighs_more_the_longer_it_waits() {
        let inputs = Inputs {
            as_of_ns: NOW,
            plans: vec![
                plan("due-yesterday", 1),
                plan("due-45-days-ago", 45),
                plan("due-400-days-ago", 400),
                plan("due-now", 0),
                plan("due-tomorrow", -1),
            ],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        let overdue: BTreeMap<&str, &Risk> = risks
            .iter()
            .filter(|r| r.rule == "manual_exercise_overdue")
            .map(|r| (r.subject.as_str(), r))
            .collect();
        assert_eq!(
            overdue.keys().copied().collect::<Vec<_>>(),
            ["due-400-days-ago", "due-45-days-ago", "due-yesterday"],
            "a plan is overdue only once its due date has passed"
        );

        let yesterday = overdue["due-yesterday"];
        assert_eq!(yesterday.severity, Severity::Medium);
        assert_eq!(yesterday.rank, Severity::Medium.base_rank());
        assert_eq!(yesterday.target.as_deref(), Some("db"));
        assert_eq!(yesterday.environment.as_deref(), Some("prod"));
        assert_eq!(yesterday.recommended_kind, None);
        assert!(yesterday.title.contains("overdue"), "{}", yesterday.title);
        assert!(
            yesterday.detail.contains("1 day(s) ago") && yesterday.detail.contains("alice"),
            "{}",
            yesterday.detail
        );
        assert_eq!(
            yesterday.evidence,
            [
                "plan_id=due-yesterday".to_string(),
                format!("due_at_ns={}", NOW - DAY_NS)
            ]
        );

        // More than 30 days past due is high, and older ranks higher, up
        // to five points, as stale evidence does.
        let late = overdue["due-45-days-ago"];
        assert_eq!(late.severity, Severity::High);
        assert_eq!(late.rank, Severity::High.base_rank() + 1);
        let later = overdue["due-400-days-ago"];
        assert_eq!(later.severity, Severity::High);
        assert_eq!(later.rank, Severity::High.base_rank() + 5);
        assert_ne!(late.id, later.id, "one finding per plan");
    }

    #[test]
    fn an_overdue_plan_weighs_more_against_a_critical_target() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![target(
                "db",
                Some("critical"),
                vec![
                    evidence("chaos", 1),
                    evidence("performance", 1),
                    evidence("robustness", 1),
                ],
            )],
            plans: vec![plan("due-yesterday", 1)],
            ..Default::default()
        };
        let risks = evaluate(&inputs);
        assert_eq!(rules_of(&risks), ["manual_exercise_overdue"]);
        assert_eq!(risks[0].rank, Severity::Medium.base_rank() + 8);
    }
}
