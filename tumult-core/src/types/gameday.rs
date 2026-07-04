//! `GameDay` types (Phase 8 — coordinated experiment campaigns).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::definition::{LoadConfig, RegulatoryMapping};
use super::journal::Journal;
use super::results::LoadResult;

/// A single experiment reference within a `GameDay`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameDayExperiment {
    pub path: PathBuf,
    #[serde(default)]
    pub compliance_maps: Vec<String>,
}

/// Scoring thresholds for `GameDay` resilience evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringConfig {
    /// Minimum experiment pass rate (0.0-1.0) for compliance.
    #[serde(default = "default_pass_threshold")]
    pub pass_threshold: f64,
    /// Maximum acceptable MTTR in seconds.
    #[serde(default = "default_mttr_target")]
    pub mttr_target_s: f64,
    /// Whether full recovery is required for compliance.
    #[serde(default)]
    pub recovery_required: bool,
}

fn default_pass_threshold() -> f64 {
    0.75
}
fn default_mttr_target() -> f64 {
    30.0
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            pass_threshold: default_pass_threshold(),
            mttr_target_s: default_mttr_target(),
            recovery_required: false,
        }
    }
}

/// A `GameDay` — a coordinated campaign of experiments under shared load.
///
/// Runs multiple experiments in sequence while a load generator
/// hammers the target system. Produces an aggregate resilience score
/// mapped to regulatory compliance articles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameDay {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regulatory: Option<RegulatoryMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<LoadConfig>,
    pub experiments: Vec<GameDayExperiment>,
    #[serde(default)]
    pub scoring: ScoringConfig,
}

/// Resilience score components for a `GameDay`.
///
/// Each component is a value between 0.0 and 1.0. The overall score
/// is a weighted mean: `pass_rate` (0.30) + recovery (0.25) +
/// `load_impact` (0.25) + compliance (0.20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResilienceScore {
    /// Fraction of experiments that completed successfully.
    pub pass_rate: f64,
    /// Recovery compliance: 1.0 if all MTTR < target, scales down.
    pub recovery_compliance: f64,
    /// Load impact tolerance: 1.0 if degradation within bounds.
    pub load_impact_tolerance: f64,
    /// Fraction of mapped compliance articles that are met.
    pub compliance_coverage: f64,
    /// Weighted overall score (0.0-1.0).
    pub overall: f64,
}

impl ResilienceScore {
    /// Computes the weighted overall score from the four components.
    #[must_use]
    pub fn compute(pass_rate: f64, recovery: f64, load_impact: f64, compliance: f64) -> Self {
        let overall = pass_rate * 0.30 + recovery * 0.25 + load_impact * 0.25 + compliance * 0.20;
        Self {
            pass_rate,
            recovery_compliance: recovery,
            load_impact_tolerance: load_impact,
            compliance_coverage: compliance,
            overall,
        }
    }

    /// Interprets the overall score as a compliance status string.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.overall >= 0.90 {
            "COMPLIANT"
        } else if self.overall >= 0.75 {
            "PARTIAL"
        } else {
            "NON-COMPLIANT"
        }
    }
}

/// Journal output for a completed `GameDay`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameDayJournal {
    pub gameday_id: String,
    pub title: String,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub duration_s: f64,
    pub experiment_journals: Vec<Journal>,
    pub load_result: Option<LoadResult>,
    pub resilience_score: ResilienceScore,
    pub compliance_status: String,
    pub regulatory: Option<RegulatoryMapping>,
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn gameday_type_round_trips() {
        let gameday = GameDay {
            title: "Q2 PG Resilience".into(),
            description: Some("Quarterly test".into()),
            tags: vec!["postgres".into(), "dora".into()],
            regulatory: None,
            load: Some(LoadConfig {
                tool: LoadTool::K6,
                script: PathBuf::from("load.js"),
                vus: Some(10),
                duration_s: Some(60.0),
                thresholds: HashMap::new(),
            }),
            experiments: vec![
                GameDayExperiment {
                    path: PathBuf::from("exp1.toon"),
                    compliance_maps: vec!["DORA-Art25".into()],
                },
                GameDayExperiment {
                    path: PathBuf::from("exp2.toon"),
                    compliance_maps: vec!["DORA-Art25".into(), "DORA-Art11".into()],
                },
            ],
            scoring: ScoringConfig {
                pass_threshold: 0.75,
                mttr_target_s: 30.0,
                recovery_required: true,
            },
        };
        let decoded: GameDay = toon_round_trip(&gameday);
        assert_eq!(decoded, gameday);
    }

    #[test]
    fn resilience_score_all_pass() {
        let score = ResilienceScore::compute(1.0, 1.0, 1.0, 1.0);
        assert!((score.overall - 1.0).abs() < f64::EPSILON);
        assert_eq!(score.status(), "COMPLIANT");
    }

    #[test]
    fn resilience_score_partial() {
        let score = ResilienceScore::compute(0.75, 0.80, 0.70, 0.80);
        assert!(score.overall >= 0.75);
        assert!(score.overall < 0.90);
        assert_eq!(score.status(), "PARTIAL");
    }

    #[test]
    fn resilience_score_non_compliant() {
        let score = ResilienceScore::compute(0.50, 0.50, 0.50, 0.50);
        assert!(score.overall < 0.75);
        assert_eq!(score.status(), "NON-COMPLIANT");
    }

    #[test]
    fn scoring_config_defaults() {
        let config = ScoringConfig::default();
        assert!((config.pass_threshold - 0.75).abs() < f64::EPSILON);
        assert!((config.mttr_target_s - 30.0).abs() < f64::EPSILON);
        assert!(!config.recovery_required);
    }
}
