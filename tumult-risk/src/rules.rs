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
//! | `recovery_target_missed` | the latest dr run or a verified manual record measured an RTO above its target, or the run did not recover |
//! | `recovery_not_measured` | a target declares an RTO and its dr runs of the last 30 days all measured no recovery (none when a finding says its DR test is late: that finding names the runs) |
//! | `rpo_missed` | the latest dr run or a verified manual record lost more data than its RPO allows |
//! | `latest_run_failed` | the latest run of an experiment did not pass (and no breached objective already explains it) |
//! | `untested_dependency` | a service other services call has no experiments at all |
//! | `never_run_definition` | a registered definition has never run |
//! | `manual_exercise_overdue` | a planned manual exercise is past its due date (more than 30 days: high); a planned DR exercise names the runs `recovery_not_measured` would |
//! | `dr_test_overdue` | a target with no planned DR exercise is past the interval of its DR tests (more than 30 days: high; one step more on a critical target); it names the runs `recovery_not_measured` would |
//! | `single_replica` | a `Deployment` or `StatefulSet` manifest runs one replica |
//! | `no_resource_limits` | a container has no resource limits |
//! | `no_pod_disruption_budget` | no `PodDisruptionBudget` selects the workload |
//! | `no_readiness_probe` | a container has no readiness probe |

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use crate::model::{
    DrTestDue, ExperimentLatest, Inputs, PlannedExercise, Risk, Severity, TargetEvidence, Verdict,
    Workload, DAY_NS, STALE_AFTER_NS, VERY_STALE_AFTER_NS,
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
            out.push(build(Draft {
                rule: "objective_breached",
                severity,
                title: format!("{} breached objective {}", e.name, v.nfr_id),
                detail: breach_detail(e, v),
                target: Some(&t.target),
                environment: t.environment.as_deref(),
                subject: format!("{}/{}", e.name, v.nfr_id),
                recommended_kind: e.kind.as_deref().and_then(kind_const),
                evidence: e.evidence_id.iter().cloned().collect(),
            }));
        }
        out.extend(recovery_target_missed(t, e));
        out.extend(rpo_missed(t, e));
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

/// What the latest run saw of a breached objective: the value it
/// observed, or, for a run that did not recover, that it did not.
fn breach_detail(e: &ExperimentLatest, v: &Verdict) -> String {
    let against = format!(
        "against a threshold of {} ({} objective)",
        v.threshold, v.criticality
    );
    if v.not_recovered() {
        let watched = v
            .observed
            .map_or_else(String::new, |o| format!(", at least {o}s after the outage"));
        return format!(
            "The latest run of {} was not recovered when its watch ended{watched}, {against}.",
            e.name
        );
    }
    let observed = v
        .observed
        .map_or_else(|| "no observation".to_string(), |o| format!("{o}"));
    format!(
        "The latest run of {} observed {observed} {against}.",
        e.name
    )
}

/// How a title names the evidence: a manual record says so.
fn named(e: &ExperimentLatest) -> String {
    if e.manual {
        return format!("Manual record {}", e.name);
    }
    e.name.clone()
}

/// How a detail names the evidence: the latest run, or a manual record.
fn source(e: &ExperimentLatest) -> String {
    if e.manual {
        return format!("The verified manual record {}", e.name);
    }
    format!("The latest disaster-recovery run of {}", e.name)
}

/// `severity`, one step more on a critical target: the scale of every
/// finding about recovery.
fn on_target(severity: Severity, t: Option<&TargetEvidence>) -> Severity {
    if t.is_some_and(is_critical) {
        return raised(severity);
    }
    severity
}

/// `recovery_target_missed`: a recovery time above the RTO, or a run that
/// did not recover at all.
fn recovery_target_missed(t: &TargetEvidence, e: &ExperimentLatest) -> Option<Risk> {
    let target_s = e.rto_target_s?;
    let (title, detail) = match (e.rto_measured_s, e.unrecovered()) {
        (Some(measured_s), _) if measured_s > target_s => (
            format!(
                "{} recovered in {measured_s:.1}s against an RTO of {target_s:.1}s",
                named(e)
            ),
            format!(
                "{} measured a recovery time of {measured_s:.1}s; the declared recovery time \
                 objective is {target_s:.1}s.",
                source(e)
            ),
        ),
        (None, Some(v)) => {
            let watched = v.observed.map_or_else(String::new, |o| {
                format!(", at least {o:.1}s after the outage")
            });
            (
                format!(
                    "{} did not recover against an RTO of {target_s:.1}s",
                    named(e)
                ),
                format!(
                    "{} was not recovered when its watch ended{watched}; the declared recovery \
                     time objective is {target_s:.1}s.",
                    source(e)
                ),
            )
        }
        _ => return None,
    };
    Some(build(Draft {
        rule: "recovery_target_missed",
        severity: on_target(Severity::High, Some(t)),
        title,
        detail,
        target: Some(&t.target),
        environment: t.environment.as_deref(),
        subject: format!("{}/rto", e.name),
        recommended_kind: Some("dr"),
        evidence: e.evidence_id.iter().cloned().collect(),
    }))
}

/// `rpo_missed`: more data lost than the RPO allows.
fn rpo_missed(t: &TargetEvidence, e: &ExperimentLatest) -> Option<Risk> {
    let (target_s, measured_s) = (e.rpo_target_s?, e.rpo_measured_s?);
    if measured_s <= target_s {
        return None;
    }
    Some(build(Draft {
        rule: "rpo_missed",
        severity: on_target(Severity::High, Some(t)),
        title: format!(
            "{} lost {measured_s:.1}s of data against an RPO of {target_s:.1}s",
            named(e)
        ),
        detail: format!(
            "{} measured a recovery point of {measured_s:.1}s: the writes of the last \
             {measured_s:.1}s before the outage were lost. The declared recovery point objective \
             is {target_s:.1}s.",
            source(e)
        ),
        target: Some(&t.target),
        environment: t.environment.as_deref(),
        subject: format!("{}/rpo", e.name),
        recommended_kind: Some("dr"),
        evidence: e.evidence_id.iter().cloned().collect(),
    }))
}

/// Whether a measured recovery met its targets: the RTO, and the RPO when
/// one was measured against a target.
fn within_targets(e: &ExperimentLatest) -> bool {
    let rto = matches!(
        (e.rto_measured_s, e.rto_target_s),
        (Some(measured), Some(target)) if measured <= target
    );
    let rpo = match (e.rpo_measured_s, e.rpo_target_s) {
        (Some(measured), Some(target)) => measured <= target,
        _ => true,
    };
    rto && rpo
}

/// The dr runs of `t` of the last 30 days that declare an RTO, when each
/// of them measured no recovery: the RTO of `t` is not proved. Empty when
/// one measured a recovery, or when a verified manual record of that time
/// recovered within its targets, which proves the RTO too.
fn unproved_runs(t: &TargetEvidence, as_of_ns: i64) -> Vec<&ExperimentLatest> {
    let recent = |e: &&ExperimentLatest| {
        e.rto_target_s.is_some() && as_of_ns - e.last_run_ns <= STALE_AFTER_NS
    };
    let runs: Vec<&ExperimentLatest> = t
        .experiments
        .iter()
        .filter(|e| !e.manual)
        .filter(recent)
        .collect();
    let proved = runs.iter().any(|e| e.measured_recovery())
        || t.experiments
            .iter()
            .filter(|e| e.manual)
            .filter(recent)
            .any(within_targets);
    if proved {
        return Vec::new();
    }
    runs
}

/// `recovery_not_measured`: the target declares an RTO, and each of its dr
/// runs of the last 30 days measured no recovery.
fn recovery_not_measured(t: &TargetEvidence, as_of_ns: i64) -> Option<Risk> {
    let runs = unproved_runs(t, as_of_ns);
    if runs.is_empty() {
        return None;
    }
    Some(build(Draft {
        rule: "recovery_not_measured",
        severity: on_target(Severity::Low, Some(t)),
        title: format!("{} has no measured recovery in 30 days", t.target),
        detail: format!(
            "{} run(s) against {} in the last 30 days declare an RTO, and none measured a \
             recovery: no outage was seen, the method failed or the watch was cut short. The \
             RTO of {} is not proved.",
            runs.len(),
            t.target,
            t.target
        ),
        target: Some(&t.target),
        environment: t.environment.as_deref(),
        subject: "rto".to_string(),
        recommended_kind: Some("dr"),
        evidence: runs.iter().filter_map(|e| e.evidence_id.clone()).collect(),
    }))
}

/// Whether the row `d` of the DR register is that of `t`: its name in its
/// environment.
fn same_target(d: &DrTestDue, t: &TargetEvidence) -> bool {
    t.target == d.target && t.environment == d.environment
}

/// The target of a row of the DR register.
fn register_target<'a>(inputs: &'a Inputs, d: &DrTestDue) -> Option<&'a TargetEvidence> {
    inputs.targets.iter().find(|t| same_target(d, t))
}

/// The last DR test of `d` and when the next was due, once that is past.
fn past_due(d: &DrTestDue, as_of_ns: i64) -> Option<(i64, i64)> {
    let (Some(last), Some(due)) = (d.last_test_at_ns, d.next_due_at_ns) else {
        return None;
    };
    (due < as_of_ns).then_some((last, due))
}

/// Whether a finding says that the DR test of `t` is late: the
/// `dr_test_overdue` of its row in the register, or the
/// `manual_exercise_overdue` of the planned DR exercise that covers it.
fn late_test_reported(inputs: &Inputs, t: &TargetEvidence) -> bool {
    inputs
        .dr_register
        .iter()
        .filter(|d| same_target(d, t))
        .any(|d| match &d.plan_id {
            None => past_due(d, inputs.as_of_ns).is_some(),
            Some(plan) => inputs
                .plans
                .iter()
                .any(|p| p.id == *plan && p.due_at_ns < inputs.as_of_ns),
        })
}

/// One late DR test is one finding about its target: the finding that
/// says so takes in the runs `recovery_not_measured` would name.
fn take_in(risk: &mut Risk, runs: &[&ExperimentLatest]) {
    if runs.is_empty() {
        return;
    }
    write!(
        risk.detail,
        " {} DR run(s) of the last 30 days declare an RTO and measured no recovery.",
        runs.len()
    )
    .expect("writing to a String cannot fail");
    risk.evidence
        .extend(runs.iter().filter_map(|e| e.evidence_id.clone()));
}

/// The runs that measured no recovery of the targets whose DR test the
/// plan `p` covers.
fn unproved_in_plan<'a>(inputs: &'a Inputs, p: &PlannedExercise) -> Vec<&'a ExperimentLatest> {
    inputs
        .dr_register
        .iter()
        .filter(|d| d.plan_id.as_deref() == Some(p.id.as_str()))
        .filter_map(|d| register_target(inputs, d))
        .flat_map(|t| unproved_runs(t, inputs.as_of_ns))
        .collect()
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
        take_in(&mut risk, &unproved_in_plan(inputs, p));
        // Longer overdue ranks higher, up to five points, as stale evidence.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bump = (days(late) / 30).clamp(0, 5) as u32;
        risk.rank += bump;
        out.push(risk);
    }
}

/// One step more severe, for a finding against a critical target.
fn raised(severity: Severity) -> Severity {
    match severity {
        Severity::Low => Severity::Medium,
        Severity::Medium => Severity::High,
        Severity::High | Severity::Critical => Severity::Critical,
    }
}

/// Targets of the DR register whose next DR test is past due. A target
/// that a planned DR exercise covers is left to that plan: when the plan
/// is overdue, `manual_exercise_overdue` says so, and one overdue test is
/// one finding.
fn overdue_dr_tests(inputs: &Inputs, out: &mut Vec<Risk>) {
    for d in inputs.dr_register.iter().filter(|d| d.plan_id.is_none()) {
        let Some((last, due)) = past_due(d, inputs.as_of_ns) else {
            continue;
        };
        let late = inputs.as_of_ns - due;
        let severity = if late > STALE_AFTER_NS {
            Severity::High
        } else {
            Severity::Medium
        };
        let t = register_target(inputs, d);
        let mut risk = build(Draft {
            rule: "dr_test_overdue",
            severity: on_target(severity, t),
            title: format!("{} DR test is overdue", d.target),
            detail: format!(
                "The last DR test of {} in {} ran {} day(s) ago. One is due every {} days ({}), \
                 so the next was due {} day(s) ago. No planned DR exercise covers it.",
                d.target,
                d.environment.as_deref().unwrap_or("any environment"),
                days(inputs.as_of_ns - last),
                d.interval_days,
                d.interval_source,
                days(late)
            ),
            target: Some(&d.target),
            environment: d.environment.as_deref(),
            subject: "dr_test".to_string(),
            recommended_kind: Some("dr"),
            evidence: vec![
                format!(
                    "last_test={}",
                    d.last_test_id.as_deref().unwrap_or_default()
                ),
                format!("next_due_at_ns={due}"),
                format!("interval_days={} ({})", d.interval_days, d.interval_source),
            ],
        });
        let unproved = t.map_or_else(Vec::new, |t| unproved_runs(t, inputs.as_of_ns));
        take_in(&mut risk, &unproved);
        // Longer overdue ranks higher, up to five points, as an overdue plan.
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
        if !late_test_reported(inputs, t) {
            out.extend(recovery_not_measured(t, inputs.as_of_ns));
        }
    }
    untested_dependencies(inputs, &mut out);
    never_run(inputs, &mut out);
    overdue_exercises(inputs, &mut out);
    overdue_dr_tests(inputs, &mut out);
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
        Container, Definition, Dependency, DisruptionBudget, DrTestDue, ExperimentLatest,
        KindEvidence, PlannedExercise, Verdict,
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
            rpo_target_s: None,
            rpo_measured_s: None,
            manual: false,
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
            recovered: None,
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

    /// The `rto` verdict of a run that watched for `watched_s` after the
    /// outage and did not recover, as the runner writes it.
    fn not_recovered(target_s: f64, watched_s: f64) -> Verdict {
        Verdict {
            nfr_id: "rto".into(),
            kind: "rto".into(),
            state: "breached".into(),
            criticality: "blocking".into(),
            threshold: target_s,
            observed: Some(watched_s),
            margin: None,
            recovered: Some(false),
        }
    }

    /// The `rto` verdict of a run that measured no recovery: no outage
    /// seen, a failed method or a watch cut short.
    fn no_recovery_measured(target_s: f64) -> Verdict {
        Verdict {
            nfr_id: "rto".into(),
            kind: "rto".into(),
            state: "inconclusive".into(),
            criticality: "blocking".into(),
            threshold: target_s,
            observed: None,
            margin: None,
            recovered: None,
        }
    }

    /// A run of `name` that declares an RTO of `target_s`, its evidence
    /// id `run-<name>`, and `age_days` old.
    fn dr_run(
        name: &str,
        target_s: f64,
        age_days: i64,
        verdicts: Vec<Verdict>,
    ) -> ExperimentLatest {
        ExperimentLatest {
            rto_target_s: Some(target_s),
            last_run_ns: NOW - age_days * DAY_NS,
            evidence_id: Some(format!("run-{name}")),
            ..experiment(name, true, verdicts)
        }
    }

    fn scan_one(t: TargetEvidence) -> Vec<Risk> {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            ..Default::default()
        };
        evaluate(&inputs)
    }

    fn one_of<'a>(risks: &'a [Risk], rule: &str) -> &'a Risk {
        let found: Vec<&Risk> = risks.iter().filter(|r| r.rule == rule).collect();
        assert_eq!(found.len(), 1, "one {rule} in {risks:?}");
        found[0]
    }

    #[test]
    fn a_run_that_did_not_recover_missed_its_recovery_target() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        let mut run = dr_run("failover", 2.0, 1, vec![not_recovered(2.0, 29.1)]);
        run.passed = false;
        t.experiments.push(run);

        let risks = scan_one(t);

        let missed = one_of(&risks, "recovery_target_missed");
        assert_eq!(missed.severity, Severity::High);
        assert_eq!(missed.subject, "failover/rto");
        assert_eq!(missed.evidence, ["run-failover".to_string()]);
        assert!(missed.title.contains("did not recover"), "{}", missed.title);
        assert!(
            missed.detail.contains("at least 29.1s") && missed.detail.contains("2.0s"),
            "{}",
            missed.detail
        );
        let breached = one_of(&risks, "objective_breached");
        assert!(
            breached.detail.contains("not recovered") && breached.detail.contains("29.1"),
            "{}",
            breached.detail
        );
        assert!(!breached.detail.contains("observed"), "{}", breached.detail);
    }

    #[test]
    fn a_run_that_measured_no_recovery_missed_no_recovery_target() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments
            .push(dr_run("failover", 2.0, 1, vec![no_recovery_measured(2.0)]));

        let risks = scan_one(t);

        assert!(!rules_of(&risks).contains(&"recovery_target_missed"));
        assert!(!rules_of(&risks).contains(&"objective_breached"));
    }

    #[test]
    fn a_target_whose_dr_runs_all_measured_no_recovery_has_an_unproved_rto() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments.extend([
            dr_run("failover", 30.0, 1, vec![no_recovery_measured(30.0)]),
            dr_run("restore", 60.0, 3, vec![no_recovery_measured(60.0)]),
            // A run that declares no RTO is no DR run.
            experiment("latency", true, vec![]),
        ]);

        let risks = scan_one(t);

        let unproved = one_of(&risks, "recovery_not_measured");
        assert_eq!(unproved.severity, Severity::Low);
        assert_eq!(unproved.rank, Severity::Low.base_rank());
        assert_eq!(unproved.subject, "rto");
        assert_eq!(unproved.target.as_deref(), Some("db"));
        assert_eq!(unproved.environment.as_deref(), Some("prod"));
        assert_eq!(unproved.recommended_kind.as_deref(), Some("dr"));
        assert_eq!(
            unproved.evidence,
            ["run-failover".to_string(), "run-restore".to_string()]
        );
        assert!(unproved.title.contains("db"), "{}", unproved.title);
        assert!(
            unproved.detail.contains("2 run(s)") && unproved.detail.contains("30 days"),
            "{}",
            unproved.detail
        );
    }

    #[test]
    fn an_unproved_rto_weighs_medium_on_a_critical_target() {
        let mut t = target("pay", Some("critical"), vec![evidence("robustness", 1)]);
        t.experiments.push(dr_run(
            "failover",
            30.0,
            1,
            vec![no_recovery_measured(30.0)],
        ));

        let risks = scan_one(t);

        let unproved = one_of(&risks, "recovery_not_measured");
        assert_eq!(unproved.severity, Severity::Medium);
        assert_eq!(unproved.rank, Severity::Medium.base_rank() + 8);
    }

    /// A measured recovery proves the RTO, in time or not; a recovery that
    /// did not come back is a failed one, not an unmeasured one; runs
    /// older than the 30 day window and runs that declare no RTO do not
    /// count; and a verified manual record that recovered within its
    /// targets is recovery evidence for its target.
    #[test]
    fn an_rto_is_unproved_only_while_every_recent_dr_run_measured_no_recovery() {
        let unmeasured = || dr_run("failover", 30.0, 1, vec![no_recovery_measured(30.0)]);
        let measured = ExperimentLatest {
            rto_measured_s: Some(42.0),
            ..dr_run("restore", 30.0, 2, vec![])
        };
        let failed = dr_run("restore", 2.0, 2, vec![not_recovered(2.0, 29.1)]);
        let record = ExperimentLatest {
            rto_measured_s: Some(1200.0),
            rpo_target_s: Some(300.0),
            rpo_measured_s: Some(60.0),
            manual: true,
            ..dr_run("restore-drill", 3600.0, 5, vec![])
        };
        let cases = [
            ("a measured recovery", vec![unmeasured(), measured]),
            (
                "a recovery that did not come back",
                vec![unmeasured(), failed],
            ),
            ("a verified manual record", vec![unmeasured(), record]),
            (
                "only old runs",
                vec![dr_run(
                    "failover",
                    30.0,
                    31,
                    vec![no_recovery_measured(30.0)],
                )],
            ),
            (
                "no run with an RTO",
                vec![experiment("latency", true, vec![])],
            ),
        ];
        for (case, experiments) in cases {
            let mut t = target("db", None, vec![evidence("robustness", 1)]);
            t.experiments = experiments;
            let risks = scan_one(t);
            assert!(
                !rules_of(&risks).contains(&"recovery_not_measured"),
                "{case}: {risks:?}"
            );
        }
    }

    /// A recovery that took as long as its RTO met it.
    #[test]
    fn a_recovery_as_long_as_its_rto_missed_nothing() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments.push(ExperimentLatest {
            rto_measured_s: Some(30.0),
            ..dr_run("failover", 30.0, 1, vec![])
        });

        let risks = scan_one(t);

        assert!(
            !rules_of(&risks).contains(&"recovery_target_missed"),
            "{risks:?}"
        );
    }

    /// A verified manual record proves the RTO of its target only when it
    /// met both its targets: one in time that lost more data than its RPO
    /// allows proves nothing.
    #[test]
    fn a_manual_record_over_its_rpo_proves_no_rto() {
        let unmeasured = dr_run("failover", 30.0, 1, vec![no_recovery_measured(30.0)]);
        let record = |rpo_measured_s: f64| ExperimentLatest {
            rto_measured_s: Some(1200.0),
            rpo_target_s: Some(300.0),
            rpo_measured_s: Some(rpo_measured_s),
            manual: true,
            ..dr_run("restore-drill", 3600.0, 5, vec![])
        };
        for (rpo_measured_s, proves) in [(420.0, false), (300.0, true)] {
            let mut t = target("db", None, vec![evidence("robustness", 1)]);
            t.experiments = vec![unmeasured.clone(), record(rpo_measured_s)];
            let risks = scan_one(t);
            assert_eq!(
                rules_of(&risks).contains(&"recovery_not_measured"),
                !proves,
                "an RPO of {rpo_measured_s}s against 300s: {risks:?}"
            );
        }
    }

    #[test]
    fn a_run_that_lost_more_data_than_its_rpo_allows_missed_it() {
        let lossy = |name: &str, measured_s: f64| ExperimentLatest {
            rpo_target_s: Some(1.0),
            rpo_measured_s: Some(measured_s),
            ..dr_run(name, 30.0, 1, vec![])
        };
        let mut critical = target("ledger", Some("critical"), vec![evidence("robustness", 1)]);
        critical
            .experiments
            .extend([lossy("restore", 2.035), lossy("replica", 0.5)]);
        let mut plain = target("cache", None, vec![evidence("robustness", 1)]);
        plain.experiments.push(lossy("restore", 2.035));
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![critical, plain],
            ..Default::default()
        };

        let risks = evaluate(&inputs);

        let missed: Vec<&Risk> = risks.iter().filter(|r| r.rule == "rpo_missed").collect();
        assert_eq!(missed.len(), 2, "only the runs over the RPO: {missed:?}");
        let ledger = missed
            .iter()
            .find(|r| r.target.as_deref() == Some("ledger"))
            .unwrap();
        assert_eq!(ledger.severity, Severity::Critical);
        assert_eq!(ledger.subject, "restore/rpo");
        assert_eq!(ledger.evidence, ["run-restore".to_string()]);
        assert_eq!(ledger.recommended_kind.as_deref(), Some("dr"));
        assert!(
            ledger.title.contains("2.0s") && ledger.title.contains("1.0s"),
            "{}",
            ledger.title
        );
        assert!(
            ledger.detail.contains("2.0s") && ledger.detail.contains("1.0s"),
            "{}",
            ledger.detail
        );
        let cache = missed
            .iter()
            .find(|r| r.target.as_deref() == Some("cache"))
            .unwrap();
        assert_eq!(cache.severity, Severity::High);
    }

    #[test]
    fn a_manual_record_over_its_targets_gives_the_findings_of_a_run_and_says_so() {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments.push(ExperimentLatest {
            rto_measured_s: Some(4100.0),
            rpo_target_s: Some(300.0),
            rpo_measured_s: Some(420.0),
            manual: true,
            ..dr_run("db-failover-drill", 3600.0, 1, vec![])
        });

        let risks = scan_one(t);

        for rule in ["recovery_target_missed", "rpo_missed"] {
            let risk = one_of(&risks, rule);
            assert_eq!(risk.severity, Severity::High, "{rule}");
            assert_eq!(risk.evidence, ["run-db-failover-drill".to_string()]);
            assert!(risk.title.starts_with("Manual record "), "{}", risk.title);
            assert!(risk.detail.contains("manual record"), "{}", risk.detail);
        }
        let rto = one_of(&risks, "recovery_target_missed");
        assert!(
            rto.detail.contains("4100.0s") && rto.detail.contains("3600.0s"),
            "{}",
            rto.detail
        );
        let rpo = one_of(&risks, "rpo_missed");
        assert!(
            rpo.detail.contains("420.0s") && rpo.detail.contains("300.0s"),
            "{}",
            rpo.detail
        );
        assert!(!rules_of(&risks).contains(&"recovery_not_measured"));
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
                plan("due-30-days-ago", 30),
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
            [
                "due-30-days-ago",
                "due-400-days-ago",
                "due-45-days-ago",
                "due-yesterday"
            ],
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

        // The first 30 days past due are medium.
        let month = overdue["due-30-days-ago"];
        assert_eq!(month.severity, Severity::Medium);
        assert_eq!(month.rank, Severity::Medium.base_rank() + 1);

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

    /// The register row of `target` in `prod`: its last DR test ran
    /// `days_ago` days before now and one is due every 365 days. `plan`
    /// is the planned DR exercise that covers it.
    fn dr_due(target: &str, days_ago: i64, plan: Option<&str>) -> DrTestDue {
        let last = NOW - days_ago * DAY_NS;
        DrTestDue {
            target: target.into(),
            environment: Some("prod".into()),
            last_test_at_ns: Some(last),
            last_test_id: Some("manual:rec-1".into()),
            interval_days: 365,
            interval_source: "default".into(),
            next_due_at_ns: Some(last + 365 * DAY_NS),
            plan_id: plan.map(str::to_owned),
        }
    }

    fn overdue_tests(risks: &[Risk]) -> BTreeMap<&str, &Risk> {
        risks
            .iter()
            .filter(|r| r.rule == "dr_test_overdue")
            .map(|r| (r.target.as_deref().unwrap_or_default(), r))
            .collect()
    }

    #[test]
    fn a_target_past_the_interval_of_its_dr_tests_with_no_plan_is_overdue() {
        let never = DrTestDue {
            last_test_at_ns: None,
            last_test_id: None,
            next_due_at_ns: None,
            ..dr_due("never", 0, None)
        };
        let inputs = Inputs {
            as_of_ns: NOW,
            dr_register: vec![
                dr_due("late-1", 366, None),
                dr_due("late-45", 410, None),
                dr_due("late-400", 765, None),
                dr_due("due-now", 365, None),
                dr_due("fresh", 10, None),
                never,
            ],
            ..Default::default()
        };

        let risks = evaluate(&inputs);

        let overdue = overdue_tests(&risks);
        assert_eq!(
            overdue.keys().copied().collect::<Vec<_>>(),
            ["late-1", "late-400", "late-45"],
            "a target is overdue once its next DR test is past due"
        );
        let first = overdue["late-1"];
        assert_eq!(first.severity, Severity::Medium);
        assert_eq!(first.rank, Severity::Medium.base_rank());
        assert_eq!(first.environment.as_deref(), Some("prod"));
        assert_eq!(first.subject, "dr_test");
        assert_eq!(first.recommended_kind.as_deref(), Some("dr"));
        assert!(first.title.contains("overdue"), "{}", first.title);
        assert!(
            first.detail.contains("366 day(s) ago")
                && first.detail.contains("every 365 days")
                && first.detail.contains("1 day(s) ago"),
            "{}",
            first.detail
        );
        assert_eq!(
            first.evidence,
            [
                "last_test=manual:rec-1".to_string(),
                format!("next_due_at_ns={}", NOW - DAY_NS),
                "interval_days=365 (default)".to_string(),
            ]
        );

        // More than 30 days past due is high, and older ranks higher, up
        // to five points, as an overdue plan does.
        let late = overdue["late-45"];
        assert_eq!(late.severity, Severity::High);
        assert_eq!(late.rank, Severity::High.base_rank() + 1);
        let later = overdue["late-400"];
        assert_eq!(later.severity, Severity::High);
        assert_eq!(later.rank, Severity::High.base_rank() + 5);
    }

    #[test]
    fn an_overdue_target_with_a_planned_dr_exercise_is_left_to_the_plan() {
        let register = vec![dr_due("db", 400, Some("plan-1"))];
        let overdue_plan = Inputs {
            as_of_ns: NOW,
            dr_register: register.clone(),
            plans: vec![plan("plan-1", 1)],
            ..Default::default()
        };
        assert_eq!(
            rules_of(&evaluate(&overdue_plan)),
            ["manual_exercise_overdue"],
            "one finding for one overdue DR test"
        );

        let plan_not_yet_due = Inputs {
            as_of_ns: NOW,
            dr_register: register,
            plans: vec![plan("plan-1", -10)],
            ..Default::default()
        };
        assert!(evaluate(&plan_not_yet_due).is_empty());

        let no_plan = Inputs {
            as_of_ns: NOW,
            dr_register: vec![dr_due("db", 400, None)],
            ..Default::default()
        };
        assert_eq!(rules_of(&evaluate(&no_plan)), ["dr_test_overdue"]);
    }

    #[test]
    fn a_dr_test_thirty_days_past_due_is_still_medium() {
        // The last test ran 395 days ago: the next was due 30 days ago.
        let inputs = Inputs {
            as_of_ns: NOW,
            dr_register: vec![dr_due("db", 395, None)],
            ..Default::default()
        };

        let risks = evaluate(&inputs);

        let overdue = overdue_tests(&risks);
        assert_eq!(overdue["db"].severity, Severity::Medium, "{risks:?}");
        assert_eq!(overdue["db"].rank, Severity::Medium.base_rank() + 1);
    }

    #[test]
    fn an_overdue_dr_test_keeps_its_severity_when_only_another_target_is_critical() {
        let covered = vec![
            evidence("chaos", 1),
            evidence("performance", 1),
            evidence("robustness", 1),
        ];
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![
                target("db", Some("low"), covered.clone()),
                target("ledger", Some("critical"), covered),
            ],
            dr_register: vec![dr_due("db", 366, None)],
            ..Default::default()
        };

        let risks = evaluate(&inputs);

        let overdue = overdue_tests(&risks);
        assert_eq!(
            overdue["db"].severity,
            Severity::Medium,
            "the db is not critical, the ledger is: {risks:?}"
        );
    }

    #[test]
    fn an_overdue_dr_test_is_one_step_more_severe_on_a_critical_target() {
        let covered = vec![
            evidence("chaos", 1),
            evidence("performance", 1),
            evidence("robustness", 1),
        ];
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![
                target("db", Some("critical"), covered.clone()),
                target("cache", Some("high"), covered),
            ],
            dr_register: vec![dr_due("db", 366, None), dr_due("cache", 400, None)],
            ..Default::default()
        };

        let risks = evaluate(&inputs);

        let overdue = overdue_tests(&risks);
        assert_eq!(rules_of(&risks).len(), 2, "{risks:?}");
        assert_eq!(overdue["db"].severity, Severity::High);
        assert_eq!(overdue["db"].rank, Severity::High.base_rank() + 8);
        assert_eq!(overdue["cache"].severity, Severity::Critical);
        assert_eq!(
            overdue["cache"].rank,
            Severity::Critical.base_rank() + 1 + 5
        );
    }

    /// A verdict of the objective `nfr_id` of kind `kind`, breached, with
    /// `recovered` as the lake keeps it in `nfr_verdicts.recovered`.
    fn marked(nfr_id: &str, kind: &str, recovered: Option<bool>) -> Verdict {
        Verdict {
            nfr_id: nfr_id.into(),
            kind: kind.into(),
            state: "breached".into(),
            criticality: "blocking".into(),
            threshold: 30.0,
            observed: Some(45.0),
            margin: None,
            recovered,
        }
    }

    /// A run did not recover when the runner marked the verdict on its
    /// RTO (`rto` of kind `rto`) as not recovered, as the lake keeps it.
    /// A spike objective that was not back, an objective of kind `rto` of
    /// its own, and a breach the runner did not mark are no such run.
    #[test]
    fn only_the_rto_verdict_marked_not_recovered_is_a_run_that_did_not_recover() {
        let cases = [
            (
                "the RTO marked not recovered",
                marked("rto", "rto", Some(false)),
                true,
            ),
            (
                "the RTO marked recovered",
                marked("rto", "rto", Some(true)),
                false,
            ),
            ("the RTO not marked", marked("rto", "rto", None), false),
            (
                "a spike that was not back",
                marked("back", "recovery", Some(false)),
                false,
            ),
            (
                "an objective of kind rto",
                marked("rto-spike", "rto", Some(false)),
                false,
            ),
            (
                "a verdict named rto of another kind",
                marked("rto", "latency", Some(false)),
                false,
            ),
        ];
        for (case, verdict, want) in cases {
            let run = dr_run("failover", 30.0, 1, vec![verdict]);
            assert_eq!(run.unrecovered().is_some(), want, "{case}");
            assert_eq!(run.measured_recovery(), want, "{case}");
            let mut t = target("db", None, vec![evidence("robustness", 1)]);
            t.experiments.push(run);
            let risks = scan_one(t);
            let rules = rules_of(&risks);
            assert_eq!(
                rules.contains(&"recovery_target_missed"),
                want,
                "{case}: {risks:?}"
            );
            assert_eq!(
                rules.contains(&"recovery_not_measured"),
                !want,
                "{case}: {risks:?}"
            );
            let breached = one_of(&risks, "objective_breached");
            assert_eq!(
                breached.detail.contains("not recovered"),
                want,
                "{case}: {}",
                breached.detail
            );
            assert_eq!(
                breached.detail.contains("observed 45"),
                !want,
                "{case}: {}",
                breached.detail
            );
        }
    }

    /// The findings about the recovery of `target`, as `rule:subject`.
    fn recovery_findings_of(risks: &[Risk], target: &str) -> Vec<String> {
        let about = [
            "recovery_target_missed",
            "rpo_missed",
            "recovery_not_measured",
            "dr_test_overdue",
            "manual_exercise_overdue",
        ];
        let mut found: Vec<String> = risks
            .iter()
            .filter(|r| r.target.as_deref() == Some(target) && about.contains(&r.rule.as_str()))
            .map(|r| format!("{}:{}", r.rule, r.subject))
            .collect();
        found.sort();
        found
    }

    /// `db` in prod: its last DR test ran 400 days ago, and its one DR run
    /// of the last 30 days saw no outage.
    fn late_and_unmeasured(register: Vec<DrTestDue>, plans: Vec<PlannedExercise>) -> Vec<Risk> {
        let mut t = target("db", None, vec![evidence("robustness", 1)]);
        t.experiments.push(dr_run(
            "failover",
            30.0,
            1,
            vec![no_recovery_measured(30.0)],
        ));
        evaluate(&Inputs {
            as_of_ns: NOW,
            targets: vec![t],
            dr_register: register,
            plans,
            ..Default::default()
        })
    }

    /// A late DR test is one finding about the recovery of its target:
    /// `dr_test_overdue` takes in the runs of the last 30 days that
    /// measured no recovery, which `recovery_not_measured` would name.
    #[test]
    fn an_overdue_dr_test_takes_in_the_runs_that_measured_no_recovery() {
        let risks = late_and_unmeasured(vec![dr_due("db", 400, None)], vec![]);

        assert_eq!(
            recovery_findings_of(&risks, "db"),
            ["dr_test_overdue:dr_test"]
        );
        let overdue = one_of(&risks, "dr_test_overdue");
        assert_eq!(overdue.severity, Severity::High);
        assert_eq!(
            overdue.evidence,
            [
                "last_test=manual:rec-1".to_string(),
                format!("next_due_at_ns={}", NOW - 35 * DAY_NS),
                "interval_days=365 (default)".to_string(),
                "run-failover".to_string(),
            ]
        );
        assert!(
            overdue.detail.contains("1 DR run(s) of the last 30 days")
                && overdue.detail.contains("measured no recovery"),
            "{}",
            overdue.detail
        );
    }

    /// The plan that covers a late DR test is its one finding, and it
    /// takes in the runs too. While the plan is not yet past due, the runs
    /// that measured no recovery are a finding of their own.
    #[test]
    fn an_overdue_plan_takes_in_the_runs_that_measured_no_recovery() {
        let covered = vec![dr_due("db", 400, Some("plan-1"))];
        let risks = late_and_unmeasured(covered.clone(), vec![plan("plan-1", 1)]);

        assert_eq!(
            recovery_findings_of(&risks, "db"),
            ["manual_exercise_overdue:plan-1"]
        );
        let overdue = one_of(&risks, "manual_exercise_overdue");
        assert_eq!(
            overdue.evidence.last().map(String::as_str),
            Some("run-failover")
        );
        assert!(
            overdue.detail.contains("1 DR run(s) of the last 30 days"),
            "{}",
            overdue.detail
        );

        for days_ago in [0, -10] {
            let risks = late_and_unmeasured(covered.clone(), vec![plan("plan-1", days_ago)]);
            assert_eq!(
                recovery_findings_of(&risks, "db"),
                ["recovery_not_measured:rto"],
                "a plan due in {} day(s)",
                -days_ago
            );
        }
    }

    /// A late DR test of the target in another environment, or of
    /// another target, leaves the runs that measured no recovery a finding
    /// of their own, and takes none of them in.
    #[test]
    fn a_late_dr_test_elsewhere_takes_in_no_run() {
        let in_staging = |plan: Option<&str>| DrTestDue {
            environment: Some("staging".into()),
            ..dr_due("db", 400, plan)
        };
        let staging_plan = PlannedExercise {
            environment: Some("staging".into()),
            ..plan("plan-1", 1)
        };
        let unplanned =
            late_and_unmeasured(vec![in_staging(None), dr_due("cache", 400, None)], vec![]);
        let planned = late_and_unmeasured(vec![in_staging(Some("plan-1"))], vec![staging_plan]);

        assert_eq!(
            recovery_findings_of(&unplanned, "db"),
            ["dr_test_overdue:dr_test", "recovery_not_measured:rto"]
        );
        assert_eq!(
            recovery_findings_of(&unplanned, "cache"),
            ["dr_test_overdue:dr_test"]
        );
        assert_eq!(
            recovery_findings_of(&planned, "db"),
            [
                "manual_exercise_overdue:plan-1",
                "recovery_not_measured:rto"
            ]
        );
        let late = ["dr_test_overdue", "manual_exercise_overdue"];
        for r in unplanned
            .iter()
            .chain(&planned)
            .filter(|r| late.contains(&r.rule.as_str()))
        {
            assert!(
                !r.evidence.contains(&"run-failover".to_string())
                    && !r.detail.contains("measured no recovery"),
                "{r:?}"
            );
        }
    }
}
