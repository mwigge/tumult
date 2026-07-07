//! Anti-hollow validation, run *before* the gate.
//!
//! From the agentic-QE lesson: an experiment that cannot falsify anything
//! is worse than none, because it produces evidence-shaped noise. The
//! validator splits its findings into two buckets with different gate
//! consequences:
//!
//! * **hollow** — the experiment cannot falsify anything (no steady-state
//!   probe, or it injects no fault at all). Hollow candidates are *vetoed*
//!   by the gate: running one would mint worthless evidence.
//! * **blockers** — the candidate is falsifiable but not safely enactable
//!   (no playbook resolved, no rollback, more than one fault — v2.15 bounds
//!   worst-case impact to guard-halting single-fault runs). Blocked
//!   candidates cap at *propose*: a human may still choose to run them.
//!
//! When no playbook resolved there is no experiment to introspect, so the
//! hollow checks are skipped — the `experiment_has_*` flags on the
//! candidate are meaningless without one. Guard *presence* is deliberately
//! not checked here: whether a guard is required is policy
//! (`require_guard`), so it belongs to the gate.

use crate::candidate::Candidate;

/// The validator's findings for one candidate; input to the gate.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidatorReport {
    /// Reasons the candidate cannot falsify anything (gate: veto).
    pub hollow: Vec<String>,
    /// Reasons the candidate is not safely enactable (gate: cap at propose).
    pub blockers: Vec<String>,
    /// `hollow` and `blockers` both empty — eligible to reach `Enact` if
    /// every gate rule also passes.
    pub enactable: bool,
}

/// Run the anti-hollow checks on a candidate. Pure and total: never fails,
/// always returns the full list of findings.
#[must_use]
pub fn validate(candidate: &Candidate) -> ValidatorReport {
    let mut hollow = Vec::new();
    let mut blockers = Vec::new();

    if candidate.playbook_experiment.is_none() {
        blockers.push(format!(
            "no playbook for {}::{}",
            candidate.plugin, candidate.action
        ));
    } else {
        if !candidate.experiment_has_steady_state {
            hollow
                .push("no steady-state probe — the experiment cannot falsify anything".to_string());
        }
        if candidate.experiment_fault_count == 0 {
            hollow.push("experiment injects no fault — nothing to observe".to_string());
        }
        if candidate.experiment_fault_count > 1 {
            blockers.push(format!(
                "experiment injects {} faults — v2.15 enacts single-fault only",
                candidate.experiment_fault_count
            ));
        }
        if !candidate.experiment_has_rollback {
            blockers.push("no rollback declared — worst-case impact unbounded".to_string());
        }
    }

    let enactable = hollow.is_empty() && blockers.is_empty();
    ValidatorReport {
        hollow,
        blockers,
        enactable,
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::candidate::{Candidate, ConfidenceTier, Trigger};

    fn candidate() -> Candidate {
        Candidate {
            id: "c-1".to_string(),
            service_id: "svc:demo-app".to_string(),
            tier: Some("service".to_string()),
            plugin: "tumult-net".to_string(),
            action: "inject_latency".to_string(),
            article_id: "compliance:DORA/Art.25".to_string(),
            score: 1.0,
            reasons: Vec::new(),
            confidence: ConfidenceTier::High,
            playbook_experiment: Some("demo/experiments/demo-net.toon".to_string()),
            experiment_has_guard: true,
            experiment_has_rollback: true,
            experiment_has_steady_state: true,
            experiment_fault_count: 1,
            trigger: Trigger::Manual,
        }
    }

    #[test]
    fn clean_candidate_is_enactable_with_no_findings() {
        let report = validate(&candidate());
        assert!(report.hollow.is_empty());
        assert!(report.blockers.is_empty());
        assert!(report.enactable);
    }

    #[test]
    fn missing_playbook_is_a_blocker_not_hollow() {
        let mut c = candidate();
        c.playbook_experiment = None;
        // Flags are meaningless without a playbook; make them worst-case to
        // prove they are ignored.
        c.experiment_has_steady_state = false;
        c.experiment_fault_count = 0;
        let report = validate(&c);
        assert_eq!(
            report.blockers,
            vec!["no playbook for tumult-net::inject_latency".to_string()]
        );
        assert!(report.hollow.is_empty());
        assert!(!report.enactable);
    }

    #[test]
    fn missing_steady_state_is_hollow() {
        let mut c = candidate();
        c.experiment_has_steady_state = false;
        let report = validate(&c);
        assert_eq!(report.hollow.len(), 1);
        assert!(report.hollow[0].contains("steady-state"));
        assert!(!report.enactable);
    }

    #[test]
    fn zero_faults_is_hollow() {
        let mut c = candidate();
        c.experiment_fault_count = 0;
        let report = validate(&c);
        assert!(report.hollow.iter().any(|r| r.contains("no fault")));
        assert!(!report.enactable);
    }

    #[test]
    fn multi_fault_is_a_blocker() {
        let mut c = candidate();
        c.experiment_fault_count = 3;
        let report = validate(&c);
        assert!(report.blockers.iter().any(|r| r.contains("3 faults")));
        assert!(report.hollow.is_empty());
        assert!(!report.enactable);
    }

    #[test]
    fn missing_rollback_is_a_blocker() {
        let mut c = candidate();
        c.experiment_has_rollback = false;
        let report = validate(&c);
        assert!(report.blockers.iter().any(|r| r.contains("rollback")));
        assert!(!report.enactable);
    }
}
