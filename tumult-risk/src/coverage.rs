//! The targets × disciplines coverage matrix.

use crate::model::{
    Coverage, CoverageCell, CoverageStatus, Inputs, KindCoverage, TargetCoverage, TargetEvidence,
    KINDS, STALE_AFTER_NS,
};

/// Coverage state of `target` in `kind` as of `as_of_ns`.
#[must_use]
pub fn cell(target: &TargetEvidence, kind: &str, as_of_ns: i64) -> CoverageCell {
    let Some(ev) = target.kinds.iter().find(|k| k.kind == kind) else {
        return CoverageCell {
            kind: kind.to_string(),
            status: CoverageStatus::Missing,
            runs: 0,
            last_run_ns: None,
        };
    };
    let status = if as_of_ns - ev.last_run_ns > STALE_AFTER_NS {
        CoverageStatus::Stale
    } else {
        CoverageStatus::Covered
    };
    CoverageCell {
        kind: kind.to_string(),
        status,
        runs: ev.runs,
        last_run_ns: Some(ev.last_run_ns),
    }
}

/// The full matrix plus the per-discipline portfolio rollup.
#[must_use]
pub fn matrix(inputs: &Inputs) -> Coverage {
    let targets: Vec<TargetCoverage> = inputs
        .targets
        .iter()
        .map(|t| TargetCoverage {
            target: t.target.clone(),
            environment: t.environment.clone(),
            criticality: t.criticality.clone(),
            cells: KINDS.iter().map(|k| cell(t, k, inputs.as_of_ns)).collect(),
        })
        .collect();
    let portfolio = KINDS
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let mut kc = KindCoverage {
                kind: (*kind).to_string(),
                covered: 0,
                stale: 0,
                missing: 0,
                ratio: 0.0,
            };
            for t in &targets {
                match t.cells[i].status {
                    CoverageStatus::Covered => kc.covered += 1,
                    CoverageStatus::Stale => kc.stale += 1,
                    CoverageStatus::Missing => kc.missing += 1,
                }
            }
            if !targets.is_empty() {
                #[allow(clippy::cast_precision_loss)]
                {
                    kc.ratio = kc.covered as f64 / targets.len() as f64;
                }
            }
            kc
        })
        .collect();
    Coverage {
        kinds: KINDS.iter().map(|k| (*k).to_string()).collect(),
        targets,
        portfolio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KindEvidence, DAY_NS};

    fn target(name: &str, kinds: &[(&str, i64)]) -> TargetEvidence {
        TargetEvidence {
            target: name.into(),
            environment: Some("prod".into()),
            criticality: None,
            kinds: kinds
                .iter()
                .map(|(k, ts)| KindEvidence {
                    kind: (*k).to_string(),
                    runs: 2,
                    last_run_ns: *ts,
                    last_outcome: Some("Completed".into()),
                })
                .collect(),
            experiments: vec![],
        }
    }

    #[test]
    fn cells_are_covered_stale_or_missing() {
        let now = 1000 * DAY_NS;
        let t = target("api", &[("chaos", now - DAY_NS), ("dr", now - 45 * DAY_NS)]);
        assert_eq!(cell(&t, "chaos", now).status, CoverageStatus::Covered);
        assert_eq!(cell(&t, "dr", now).status, CoverageStatus::Stale);
        let missing = cell(&t, "load", now);
        assert_eq!(missing.status, CoverageStatus::Missing);
        assert_eq!(missing.runs, 0);
        assert_eq!(missing.last_run_ns, None);
    }

    #[test]
    fn matrix_rolls_up_per_discipline() {
        let now = 1000 * DAY_NS;
        let inputs = Inputs {
            as_of_ns: now,
            targets: vec![
                target("api", &[("chaos", now - DAY_NS)]),
                target("db", &[("chaos", now - 40 * DAY_NS), ("dr", now - DAY_NS)]),
            ],
            ..Default::default()
        };
        let cov = matrix(&inputs);
        assert_eq!(cov.kinds.len(), 7);
        assert_eq!(cov.targets.len(), 2);
        assert_eq!(cov.targets[0].cells.len(), 7);
        let chaos = cov.portfolio.iter().find(|k| k.kind == "chaos").unwrap();
        assert_eq!((chaos.covered, chaos.stale, chaos.missing), (1, 1, 0));
        assert!((chaos.ratio - 0.5).abs() < f64::EPSILON);
        let load = cov.portfolio.iter().find(|k| k.kind == "load").unwrap();
        assert_eq!((load.covered, load.stale, load.missing), (0, 0, 2));
        assert!(load.ratio.abs() < f64::EPSILON);
    }

    #[test]
    fn empty_portfolio_has_zero_ratios() {
        let cov = matrix(&Inputs::default());
        assert!(cov.targets.is_empty());
        assert!(cov.portfolio.iter().all(|k| k.ratio == 0.0));
    }
}
