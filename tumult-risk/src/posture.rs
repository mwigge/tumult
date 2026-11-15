//! The continuous posture score.
//!
//! The compliance score (100 / 75 / 50 / 0 from the latest run, with a
//! 30-day cliff) stays what the regulatory reports use. Posture is the
//! continuous companion the executive page shows next to it:
//!
//! ```text
//! posture = 0.35 · objective margin   (met objectives, weighted by criticality)
//!         + 0.30 · discipline coverage (chaos, performance, robustness weigh most; stale counts half)
//!         + 0.20 · evidence freshness  (half-life 30 days, no cliff)
//!         + 0.15 · recovery            (measured RTO against its target)
//! ```
//!
//! A component with nothing to measure drops out and the remaining weights
//! renormalise, so a target with no objectives is scored on the other
//! three rather than penalised for silence. The portfolio is the
//! criticality-weighted mean over targets.

use serde::{Deserialize, Serialize};

use crate::model::{criticality_weight, CoverageStatus, Inputs, TargetEvidence, KINDS};

const W_NFR: f64 = 0.35;
const W_COVERAGE: f64 = 0.30;
const W_FRESHNESS: f64 = 0.20;
const W_RECOVERY: f64 = 0.15;

/// Freshness half-life: evidence this old is worth half.
const HALF_LIFE_NS: f64 = 30.0 * 86_400.0 * 1e9;

/// The four components of one target's posture, each 0–100 or absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Components {
    pub nfr: Option<f64>,
    pub coverage: f64,
    pub freshness: Option<f64>,
    pub recovery: Option<f64>,
}

/// One target's posture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetPosture {
    pub target: String,
    pub environment: Option<String>,
    pub criticality: Option<String>,
    pub score: f64,
    pub band: String,
    pub components: Components,
}

/// The portfolio posture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Posture {
    pub score: f64,
    pub band: String,
    pub targets: Vec<TargetPosture>,
}

/// Band for a score, the same thresholds as the compliance score: above 70
/// good, 50 to 70 fair, below 50 poor.
#[must_use]
pub fn band(score: f64) -> &'static str {
    if score > 70.0 {
        "good"
    } else if score >= 50.0 {
        "fair"
    } else {
        "poor"
    }
}

fn kind_weight(kind: &str) -> f64 {
    match kind {
        "chaos" | "performance" | "robustness" => 3.0,
        _ => 1.0,
    }
}

fn verdict_weight(criticality: &str) -> f64 {
    match criticality {
        "blocking" => 3.0,
        "major" => 2.0,
        _ => 1.0,
    }
}

fn nfr_component(t: &TargetEvidence) -> Option<f64> {
    let mut weight = 0.0;
    let mut met = 0.0;
    for v in t.experiments.iter().flat_map(|e| e.verdicts.iter()) {
        let w = verdict_weight(&v.criticality);
        match v.state.as_str() {
            "met" => {
                weight += w;
                met += w;
            }
            "breached" => weight += w,
            _ => {}
        }
    }
    (weight > 0.0).then(|| 100.0 * met / weight)
}

fn coverage_component(t: &TargetEvidence, as_of_ns: i64) -> f64 {
    let total: f64 = KINDS.iter().map(|k| kind_weight(k)).sum();
    let earned: f64 = KINDS
        .iter()
        .map(|k| {
            let w = kind_weight(k);
            match crate::coverage::cell(t, k, as_of_ns).status {
                CoverageStatus::Covered => w,
                CoverageStatus::Stale => w / 2.0,
                CoverageStatus::Missing => 0.0,
            }
        })
        .sum();
    100.0 * earned / total
}

fn freshness_component(t: &TargetEvidence, as_of_ns: i64) -> Option<f64> {
    let latest = t.kinds.iter().map(|k| k.last_run_ns).max()?;
    #[allow(clippy::cast_precision_loss)]
    let age = (as_of_ns - latest).max(0) as f64;
    Some(100.0 * (-(std::f64::consts::LN_2) * age / HALF_LIFE_NS).exp())
}

fn recovery_component(t: &TargetEvidence) -> Option<f64> {
    let scores: Vec<f64> = t
        .experiments
        .iter()
        .filter_map(|e| Some((e.rto_target_s?, e.rto_measured_s?)))
        .filter(|(target, _)| *target > 0.0)
        .map(|(target, measured)| {
            if measured <= target {
                100.0
            } else {
                (100.0 * (1.0 - (measured - target) / target)).max(0.0)
            }
        })
        .collect();
    if scores.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(scores.iter().sum::<f64>() / scores.len() as f64)
}

/// Combine the components, renormalising over the ones present.
#[must_use]
pub fn combine(c: &Components) -> f64 {
    let mut weight = W_COVERAGE;
    let mut sum = W_COVERAGE * c.coverage;
    for (w, v) in [
        (W_NFR, c.nfr),
        (W_FRESHNESS, c.freshness),
        (W_RECOVERY, c.recovery),
    ] {
        if let Some(v) = v {
            weight += w;
            sum += w * v;
        }
    }
    (sum / weight).clamp(0.0, 100.0)
}

/// Posture of one target as of `as_of_ns`.
#[must_use]
pub fn target_posture(t: &TargetEvidence, as_of_ns: i64) -> TargetPosture {
    let components = Components {
        nfr: nfr_component(t),
        coverage: coverage_component(t, as_of_ns),
        freshness: freshness_component(t, as_of_ns),
        recovery: recovery_component(t),
    };
    let score = combine(&components);
    TargetPosture {
        target: t.target.clone(),
        environment: t.environment.clone(),
        criticality: t.criticality.clone(),
        score,
        band: band(score).to_string(),
        components,
    }
}

/// Portfolio posture: criticality-weighted mean over targets.
#[must_use]
pub fn compute(inputs: &Inputs) -> Posture {
    let targets: Vec<TargetPosture> = inputs
        .targets
        .iter()
        .map(|t| target_posture(t, inputs.as_of_ns))
        .collect();
    let (sum, weight) = targets.iter().fold((0.0, 0.0), |(s, w), t| {
        let cw = criticality_weight(t.criticality.as_deref());
        (s + cw * t.score, w + cw)
    });
    let score = if weight > 0.0 { sum / weight } else { 0.0 };
    Posture {
        score,
        band: band(score).to_string(),
        targets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DAY_NS;
    use crate::model::{ExperimentLatest, KindEvidence, Verdict};

    const NOW: i64 = 2000 * DAY_NS;

    fn evidence(kind: &str, age_days: i64) -> KindEvidence {
        KindEvidence {
            kind: kind.into(),
            runs: 1,
            last_run_ns: NOW - age_days * DAY_NS,
            last_outcome: None,
        }
    }

    fn verdict(state: &str, criticality: &str) -> Verdict {
        Verdict {
            nfr_id: "x".into(),
            kind: "latency".into(),
            state: state.into(),
            criticality: criticality.into(),
            threshold: 1.0,
            observed: None,
            margin: None,
        }
    }

    fn experiment(verdicts: Vec<Verdict>, rto: Option<(f64, f64)>) -> ExperimentLatest {
        ExperimentLatest {
            name: "e".into(),
            kind: "robustness".into(),
            last_run_ns: NOW,
            passed: true,
            outcome: None,
            evidence_id: None,
            verdicts,
            rto_target_s: rto.map(|r| r.0),
            rto_measured_s: rto.map(|r| r.1),
        }
    }

    #[test]
    fn bands_match_the_compliance_thresholds() {
        assert_eq!(band(70.1), "good");
        assert_eq!(band(70.0), "fair");
        assert_eq!(band(50.0), "fair");
        assert_eq!(band(49.9), "poor");
    }

    #[test]
    fn full_coverage_fresh_evidence_scores_one_hundred() {
        let t = TargetEvidence {
            target: "api".into(),
            kinds: KINDS.iter().map(|k| evidence(k, 0)).collect(),
            ..Default::default()
        };
        let p = target_posture(&t, NOW);
        assert!((p.components.coverage - 100.0).abs() < 1e-9);
        assert!((p.components.freshness.unwrap() - 100.0).abs() < 1e-9);
        assert_eq!(p.components.nfr, None);
        assert_eq!(p.components.recovery, None);
        assert!((p.score - 100.0).abs() < 1e-9, "{p:?}");
        assert_eq!(p.band, "good");
    }

    #[test]
    fn coverage_weights_core_disciplines_and_halves_stale_evidence() {
        // chaos fresh (3), robustness stale (1.5) out of 10.
        let t = TargetEvidence {
            target: "api".into(),
            kinds: vec![evidence("chaos", 1), evidence("robustness", 45)],
            ..Default::default()
        };
        let c = coverage_component(&t, NOW);
        assert!((c - 100.0 * 4.5 / 10.0).abs() < 1e-9, "{c}");
    }

    #[test]
    fn freshness_decays_with_a_thirty_day_half_life() {
        let t = TargetEvidence {
            target: "api".into(),
            kinds: vec![evidence("chaos", 30), evidence("performance", 90)],
            ..Default::default()
        };
        // The newest evidence (30 days) drives freshness.
        let f = freshness_component(&t, NOW).unwrap();
        assert!((f - 50.0).abs() < 1e-6, "{f}");
        let none = TargetEvidence {
            target: "x".into(),
            ..Default::default()
        };
        assert_eq!(freshness_component(&none, NOW), None);
    }

    #[test]
    fn objectives_weigh_by_criticality() {
        let t = TargetEvidence {
            target: "api".into(),
            experiments: vec![experiment(
                vec![
                    verdict("met", "blocking"),
                    verdict("breached", "major"),
                    verdict("inconclusive", "blocking"),
                ],
                None,
            )],
            ..Default::default()
        };
        // met 3 of (3 + 2) weighted; inconclusive ignored.
        let n = nfr_component(&t).unwrap();
        assert!((n - 60.0).abs() < 1e-9, "{n}");
    }

    #[test]
    fn recovery_scores_measured_against_target() {
        let within = TargetEvidence {
            target: "a".into(),
            experiments: vec![experiment(vec![], Some((30.0, 12.0)))],
            ..Default::default()
        };
        assert_eq!(recovery_component(&within), Some(100.0));
        let over = TargetEvidence {
            target: "b".into(),
            experiments: vec![experiment(vec![], Some((30.0, 45.0)))],
            ..Default::default()
        };
        assert!((recovery_component(&over).unwrap() - 50.0).abs() < 1e-9);
        let far = TargetEvidence {
            target: "c".into(),
            experiments: vec![experiment(vec![], Some((30.0, 90.0)))],
            ..Default::default()
        };
        assert_eq!(recovery_component(&far), Some(0.0));
    }

    #[test]
    fn combine_renormalises_over_present_components() {
        let only_coverage = Components {
            coverage: 40.0,
            ..Default::default()
        };
        assert!((combine(&only_coverage) - 40.0).abs() < 1e-9);
        let all = Components {
            nfr: Some(100.0),
            coverage: 100.0,
            freshness: Some(100.0),
            recovery: Some(0.0),
        };
        assert!((combine(&all) - 85.0).abs() < 1e-9);
    }

    #[test]
    fn portfolio_is_criticality_weighted() {
        let fresh = TargetEvidence {
            target: "crit".into(),
            criticality: Some("critical".into()),
            kinds: KINDS.iter().map(|k| evidence(k, 0)).collect(),
            ..Default::default()
        };
        let empty = TargetEvidence {
            target: "low".into(),
            criticality: Some("low".into()),
            ..Default::default()
        };
        let inputs = Inputs {
            as_of_ns: NOW,
            targets: vec![fresh, empty],
            ..Default::default()
        };
        let p = compute(&inputs);
        // (3 · 100 + 1 · 0) / 4.
        assert!((p.score - 75.0).abs() < 1e-9, "{p:?}");
        assert_eq!(p.band, "good");
        assert_eq!(p.targets.len(), 2);
        assert!(compute(&Inputs::default()).score.abs() < f64::EPSILON);
    }
}
