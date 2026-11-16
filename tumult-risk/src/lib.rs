//! Passive resilience risk scanning and the continuous posture score.
//!
//! Harness and Gremlin lead with *risks and coverage gaps* and only then
//! with fault injection; this crate gives Tumult the same opening move
//! without running a single fault. It is pure: no database, no cluster
//! access. Callers assemble [`Inputs`] from the lake (targets, their latest
//! evidence per discipline, objective verdicts, call-graph dependencies,
//! registered definitions), from a manifests directory ([`manifests`]) and
//! from the suppression table, then call [`scan`] for a ranked [`Report`]
//! and [`posture::compute`] for the continuous score.
//!
//! Risk ids are stable hashes of rule, target, environment and subject, so
//! a suppression recorded today still matches the same finding next week.

// `posture::Posture` and friends read better than the alternatives.
#![allow(clippy::module_name_repetitions)]

pub mod coverage;
pub mod manifests;
pub mod model;
pub mod posture;
pub mod rules;

pub use model::{
    Coverage, CoverageCell, CoverageStatus, Definition, Dependency, DisruptionBudget,
    ExperimentLatest, Inputs, KindCoverage, KindEvidence, Report, Risk, Severity, Summary,
    Suppression, TargetCoverage, TargetEvidence, Verdict, Workload, KINDS,
};
pub use posture::{Components, Posture, TargetPosture};

/// Evaluate every rule, rank the findings, apply suppressions and attach
/// the coverage matrix.
#[must_use]
pub fn scan(inputs: &Inputs) -> Report {
    let mut all = rules::evaluate(inputs);
    for r in &mut all {
        r.suppression = inputs
            .suppressions
            .iter()
            .filter(|s| s.risk_id == r.id && s.active_at(inputs.as_of_ns))
            .max_by_key(|s| s.at_ns)
            .cloned();
    }
    // Highest rank first; ties break on severity then id so the order is
    // stable between scans.
    all.sort_by(|a, b| {
        b.rank
            .cmp(&a.rank)
            .then(b.severity.cmp(&a.severity))
            .then(a.id.cmp(&b.id))
    });
    let (suppressed, risks): (Vec<Risk>, Vec<Risk>) =
        all.into_iter().partition(|r| r.suppression.is_some());
    let mut summary = Summary {
        total: risks.len(),
        suppressed: suppressed.len(),
        ..Default::default()
    };
    for r in &risks {
        match r.severity {
            Severity::Critical => summary.critical += 1,
            Severity::High => summary.high += 1,
            Severity::Medium => summary.medium += 1,
            Severity::Low => summary.low += 1,
        }
    }
    Report {
        as_of_ns: inputs.as_of_ns,
        risks,
        suppressed,
        summary,
        coverage: coverage::matrix(inputs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DAY_NS;

    const NOW: i64 = 2000 * DAY_NS;

    fn bare_target(name: &str, criticality: Option<&str>) -> TargetEvidence {
        TargetEvidence {
            target: name.into(),
            environment: Some("prod".into()),
            criticality: criticality.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn report_ranks_highest_first_and_counts_by_severity() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![
                bare_target("pay", Some("critical")),
                bare_target("blog", None),
            ],
            ..Default::default()
        };
        let report = scan(&inputs);
        assert_eq!(report.as_of_ns, NOW);
        assert_eq!(report.summary.total, 6);
        assert_eq!(report.summary.high, 3, "{:?}", report.summary);
        assert_eq!(report.summary.medium, 3);
        assert_eq!(report.summary.suppressed, 0);
        let ranks: Vec<u32> = report.risks.iter().map(|r| r.rank).collect();
        let mut sorted = ranks.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(ranks, sorted);
        assert_eq!(report.risks[0].target.as_deref(), Some("pay"));
        assert_eq!(report.coverage.targets.len(), 2);
    }

    #[test]
    fn active_suppressions_move_risks_aside_and_expired_ones_do_not() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![bare_target("blog", None)],
            ..Default::default()
        };
        let first = scan(&inputs);
        let silenced = first.risks[0].id.clone();
        let expired = first.risks[1].id.clone();
        let inputs = Inputs {
            suppressions: vec![
                Suppression {
                    risk_id: silenced.clone(),
                    reason: "accepted until Q1".into(),
                    actor: "op".into(),
                    at_ns: NOW - DAY_NS,
                    expires_at_ns: Some(NOW + 10 * DAY_NS),
                },
                Suppression {
                    risk_id: expired.clone(),
                    reason: "old".into(),
                    actor: "op".into(),
                    at_ns: NOW - 40 * DAY_NS,
                    expires_at_ns: Some(NOW - DAY_NS),
                },
            ],
            ..inputs
        };
        let report = scan(&inputs);
        assert_eq!(report.summary.total, 2);
        assert_eq!(report.summary.suppressed, 1);
        assert_eq!(report.suppressed[0].id, silenced);
        assert_eq!(
            report.suppressed[0].suppression.as_ref().unwrap().reason,
            "accepted until Q1"
        );
        assert!(report.risks.iter().any(|r| r.id == expired));
        assert!(report.risks.iter().all(|r| r.suppression.is_none()));
    }

    #[test]
    fn report_round_trips_through_json() {
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![bare_target("blog", None)],
            ..Default::default()
        };
        let report = scan(&inputs);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["summary"]["total"], 3);
        assert_eq!(json["risks"][0]["severity"], "high");
        assert_eq!(json["coverage"]["kinds"].as_array().unwrap().len(), 4);
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }
}
