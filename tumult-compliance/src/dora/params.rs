//! House parameters of the DORA coverage and maturity scores.
//!
//! Nothing in this file is regulatory text. Every number is a house
//! choice: named here, documented in `docs/guides/dora-score.md`, and
//! overridable per deployment from the `score:` section of `org.yaml`
//! (any subset of fields; missing fields keep the defaults below).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Weight of a function by criticality tier (`c_f`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CriticalityWeights {
    pub critical: f64,
    pub important: f64,
    pub other: f64,
}

impl Default for CriticalityWeights {
    fn default() -> Self {
        Self {
            critical: 3.0,
            important: 2.0,
            other: 1.5,
        }
    }
}

/// Outcome factor `o` of one piece of evidence. Inconclusive evidence is
/// not evidence and never reaches the score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutcomeFactors {
    pub passed: f64,
    pub partial: f64,
    pub failed: f64,
}

impl Default for OutcomeFactors {
    fn default() -> Self {
        Self {
            passed: 1.0,
            partial: 0.75,
            failed: 0.5,
        }
    }
}

/// Strength factor `s` of the mapping from evidence to requirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StrengthFactors {
    pub direct: f64,
    pub supporting: f64,
    pub indirect: f64,
    /// Cap applied to a verified manual record standing in for an
    /// automated test.
    pub manual_test: f64,
}

impl Default for StrengthFactors {
    fn default() -> Self {
        Self {
            direct: 1.0,
            supporting: 0.75,
            indirect: 0.25,
            manual_test: 0.75,
        }
    }
}

/// Weights of the five maturity pillars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PillarWeights {
    pub conformance: f64,
    pub coverage: f64,
    pub slo: f64,
    pub recovery: f64,
    #[serde(rename = "loop")]
    pub loop_: f64,
}

/// Pillar weights per rule set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PillarWeightSets {
    /// Full framework.
    pub full: PillarWeights,
    /// Simplified framework or microenterprise.
    pub reduced: PillarWeights,
}

impl Default for PillarWeightSets {
    fn default() -> Self {
        Self {
            full: PillarWeights {
                conformance: 0.20,
                coverage: 0.25,
                slo: 0.25,
                recovery: 0.20,
                loop_: 0.10,
            },
            reduced: PillarWeights {
                conformance: 0.20,
                coverage: 0.30,
                slo: 0.15,
                recovery: 0.30,
                loop_: 0.05,
            },
        }
    }
}

/// Highest conformance level that counts toward the conformance pillar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConformanceCeiling {
    pub full: u8,
    pub reduced: u8,
}

impl Default for ConformanceCeiling {
    fn default() -> Self {
        Self {
            full: 4,
            reduced: 2,
        }
    }
}

/// House target level per profile. No authority has set these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TargetLevels {
    pub full: u8,
    pub full_tlpt_designated: u8,
    pub full_microenterprise: u8,
    pub simplified: u8,
    pub simplified_microenterprise: u8,
}

impl Default for TargetLevels {
    fn default() -> Self {
        Self {
            full: 3,
            full_tlpt_designated: 4,
            full_microenterprise: 2,
            simplified: 3,
            simplified_microenterprise: 2,
        }
    }
}

/// Thresholds of the level gates. Pillar thresholds are on the 0–100
/// scale; `c` thresholds are the share `C_k` of services at conformance
/// level `k` or better.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GateThresholds {
    pub l2_c1: f64,
    pub l2_coverage: f64,
    pub l3_c1: f64,
    pub l3_c2: f64,
    pub l3_coverage: f64,
    pub l3_slo: f64,
    pub l3_slo_history_days: u32,
    pub l3_recovery: f64,
    pub l3_max_unknown_share: f64,
    pub l4_c3: f64,
    pub l4_coverage: f64,
    pub l4_slo: f64,
    pub l4_recovery: f64,
    pub l4_loop: f64,
    pub l5_c4: f64,
    pub l5_coverage: f64,
    pub l5_slo: f64,
    pub l5_recovery: f64,
    pub l5_loop: f64,
}

impl Default for GateThresholds {
    fn default() -> Self {
        Self {
            l2_c1: 50.0,
            l2_coverage: 25.0,
            l3_c1: 80.0,
            l3_c2: 50.0,
            l3_coverage: 60.0,
            l3_slo: 80.0,
            l3_slo_history_days: 180,
            l3_recovery: 60.0,
            l3_max_unknown_share: 0.5,
            l4_c3: 80.0,
            l4_coverage: 80.0,
            l4_slo: 90.0,
            l4_recovery: 80.0,
            l4_loop: 60.0,
            l5_c4: 90.0,
            l5_coverage: 90.0,
            l5_slo: 95.0,
            l5_recovery: 90.0,
            l5_loop: 85.0,
        }
    }
}

/// SLO pillar parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SloParams {
    pub blocking_weight: f64,
    pub major_weight: f64,
    pub minor_weight: f64,
    /// A met SLO with less error budget remaining than this earns only
    /// `low_budget_credit`.
    pub low_budget: f64,
    pub low_budget_credit: f64,
}

impl Default for SloParams {
    fn default() -> Self {
        Self {
            blocking_weight: 3.0,
            major_weight: 2.0,
            minor_weight: 1.0,
            low_budget: 0.25,
            low_budget_credit: 0.75,
        }
    }
}

/// Per-node roll-up parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RollupParams {
    /// Share `ω` of a node's own score in its index, by node kind; the
    /// rest comes from the weighted mean of its children.
    pub own_share: BTreeMap<String, f64>,
    /// Pillars a node of each kind can observe itself (`C`, `T`, `S`,
    /// `R`, `L`).
    pub own_pillars: BTreeMap<String, Vec<String>>,
    /// Score an unknown child enters its parent's mean with, and the
    /// level it caps a parent at when critical.
    pub unknown_floor: f64,
    /// Weight of each service that maps to no function.
    pub unassigned_weight: f64,
    /// Whether every node under a critical function is itself critical.
    /// When false, only the function node caps its parents.
    pub critical_inheritance: bool,
}

impl Default for RollupParams {
    fn default() -> Self {
        let share = [
            ("workload", 1.0),
            ("service", 0.5),
            ("component", 0.3),
            ("system", 0.3),
            ("entity", 0.5),
            ("org", 0.0),
        ];
        let pillars = [
            ("workload", "CTR"),
            ("service", "CTS"),
            ("component", "TR"),
            ("system", "TR"),
            ("entity", "TSRL"),
            ("org", ""),
        ];
        Self {
            own_share: share
                .into_iter()
                .map(|(kind, omega)| (kind.to_string(), omega))
                .collect(),
            own_pillars: pillars
                .into_iter()
                .map(|(kind, set)| {
                    let list = set.chars().map(|c| c.to_string()).collect();
                    (kind.to_string(), list)
                })
                .collect(),
            unknown_floor: 1.0,
            unassigned_weight: 1.0,
            critical_inheritance: true,
        }
    }
}

/// Resource-attribute keys a service must carry to reach each conformance
/// level, and the window in which it must have been seen. Each level
/// needs its own keys and every key of the levels below it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConformanceParams {
    pub window_days: u32,
    /// L1, ICT spine.
    pub level1: Vec<String>,
    /// L2, release identity.
    pub level2: Vec<String>,
    /// L3, loop integration.
    pub level3: Vec<String>,
    /// L4, evidence-grade telemetry.
    pub level4: Vec<String>,
}

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|key| (*key).to_string()).collect()
}

impl Default for ConformanceParams {
    fn default() -> Self {
        Self {
            window_days: 90,
            level1: keys(&[
                "resilience.target.system",
                "resilience.target.technology",
                "resilience.target.environment",
                "ict.schema.version",
                "ict.entity.id",
                "ict.entity.tier",
                "ict.entity.owner",
            ]),
            level2: keys(&["service.version", "resilience.target.component"]),
            level3: keys(&["ict.change.id"]),
            level4: keys(&["ict.evidence.hash", "ict.control.ids"]),
        }
    }
}

impl ConformanceParams {
    /// Key sets in level order (index 0 is level 1).
    #[must_use]
    pub fn levels(&self) -> [&[String]; 4] {
        [&self.level1, &self.level2, &self.level3, &self.level4]
    }
}

/// Every house parameter of the two scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HouseParams {
    pub criticality: CriticalityWeights,
    pub outcome: OutcomeFactors,
    pub strength: StrengthFactors,
    /// Past its window `F`, evidence halves every `F ·` this fraction.
    pub freshness_decay_fraction: f64,
    /// `v` at or above this is "Existing".
    pub existing_threshold: f64,
    /// Unbroken days at or above the threshold that make "Operating".
    pub operating_days: u32,
    /// Share of bound functions (by criticality weight) that must be
    /// Operating for a requirement, and for the level-3 gate, to count as
    /// Operating.
    pub operating_share: f64,
    /// Weight of each test discipline in the coverage pillar.
    pub kind_weights: BTreeMap<String, f64>,
    /// Discipline of each manual exercise type; types not listed are
    /// records, not tests.
    pub manual_kinds: BTreeMap<String, String>,
    pub test_window_days: u32,
    pub recovery_window_days: u32,
    pub loop_window_days: u32,
    pub loop_closure_weight: f64,
    pub loop_previously_tested_weight: f64,
    pub slo: SloParams,
    pub pillar_weights: PillarWeightSets,
    pub conformance_ceiling: ConformanceCeiling,
    pub targets: TargetLevels,
    pub gates: GateThresholds,
    pub rollup: RollupParams,
    pub conformance: ConformanceParams,
    /// Expected evidence status per requirement id (1–3), overriding the
    /// registry default.
    pub expected_status: BTreeMap<String, u8>,
    /// The row that stands for "yearly continuity or restore test" in the
    /// simplified coverage pillar.
    pub simplified_test_row: String,
    /// Rows whose signed record counts as the yearly management-body
    /// report the level-4 gate asks for.
    pub management_report_rows: Vec<String>,
    /// The threat-led penetration test row the level-5 gate reads when
    /// the entity is designated.
    pub tlpt_row: String,
    /// Coverage at or above this is band `good`.
    pub band_good: f64,
    /// Coverage at or above this is band `fair`; below is `weak`.
    pub band_fair: f64,
}

impl Default for HouseParams {
    fn default() -> Self {
        let kind_weights = [
            ("chaos", 3.0),
            ("dr", 3.0),
            ("load", 2.0),
            ("performance", 1.0),
            ("robustness", 1.0),
            ("security", 1.0),
            ("nfr", 1.0),
        ];
        let manual_kinds = [
            ("failover", "dr"),
            ("drill", "dr"),
            ("gameday", "chaos"),
            ("pentest", "security"),
        ];
        Self {
            criticality: CriticalityWeights::default(),
            outcome: OutcomeFactors::default(),
            strength: StrengthFactors::default(),
            freshness_decay_fraction: 0.25,
            existing_threshold: 0.75,
            operating_days: 180,
            operating_share: 0.8,
            kind_weights: kind_weights
                .into_iter()
                .map(|(kind, weight)| (kind.to_string(), weight))
                .collect(),
            manual_kinds: manual_kinds
                .into_iter()
                .map(|(exercise, kind)| (exercise.to_string(), kind.to_string()))
                .collect(),
            test_window_days: 365,
            recovery_window_days: 365,
            loop_window_days: 365,
            loop_closure_weight: 0.75,
            loop_previously_tested_weight: 0.25,
            slo: SloParams::default(),
            pillar_weights: PillarWeightSets::default(),
            conformance_ceiling: ConformanceCeiling::default(),
            targets: TargetLevels::default(),
            gates: GateThresholds::default(),
            rollup: RollupParams::default(),
            conformance: ConformanceParams::default(),
            expected_status: BTreeMap::new(),
            simplified_test_row: "rts1774.art40".to_string(),
            management_report_rows: keys(&["dora.art13.5", "dora.art6.5", "dora.art16.2"]),
            tlpt_row: "dora.art26.1".to_string(),
            band_good: 0.8,
            band_fair: 0.5,
        }
    }
}

impl HouseParams {
    /// Check that the parameters describe a usable model.
    ///
    /// # Errors
    /// Names the first parameter that is out of range.
    pub fn validate(&self) -> Result<(), String> {
        let unit = [
            ("existing_threshold", self.existing_threshold),
            ("operating_share", self.operating_share),
            ("band_good", self.band_good),
            ("band_fair", self.band_fair),
            (
                "gates.l3_max_unknown_share",
                self.gates.l3_max_unknown_share,
            ),
        ];
        for (name, value) in unit {
            if !(0.0..=1.0).contains(&value) {
                return Err(format!("score.{name} must be within 0..=1, got {value}"));
            }
        }
        if self.freshness_decay_fraction <= 0.0 {
            return Err("score.freshness_decay_fraction must be positive".into());
        }
        if self.operating_days == 0 {
            return Err("score.operating_days must be at least 1".into());
        }
        for (kind, omega) in &self.rollup.own_share {
            if !(0.0..=1.0).contains(omega) {
                return Err(format!(
                    "score.rollup.own_share.{kind} must be within 0..=1, got {omega}"
                ));
            }
        }
        for (id, status) in &self.expected_status {
            if !(1..=3).contains(status) {
                return Err(format!(
                    "score.expected_status.{id} must be 1, 2 or 3, got {status}"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_house_values() {
        let p = HouseParams::default();
        assert_eq!(p.criticality.critical, 3.0);
        assert_eq!(p.criticality.important, 2.0);
        assert_eq!(p.criticality.other, 1.5);
        assert_eq!(p.operating_days, 180);
        assert_eq!(p.existing_threshold, 0.75);
        assert_eq!(p.kind_weights["chaos"], 3.0);
        assert_eq!(p.kind_weights["dr"], 3.0);
        assert_eq!(p.kind_weights["load"], 2.0);
        assert_eq!(p.kind_weights.len(), 7);
        assert_eq!(p.rollup.own_share["service"], 0.5);
        assert_eq!(p.rollup.own_share["component"], 0.3);
        assert_eq!(p.rollup.own_pillars["entity"], ["T", "S", "R", "L"]);
        assert_eq!(p.targets.full, 3);
        assert_eq!(p.targets.full_tlpt_designated, 4);
        assert_eq!(p.simplified_test_row, "rts1774.art40");
        p.validate().unwrap();
    }

    #[test]
    fn a_partial_override_keeps_every_other_default() {
        let p: HouseParams = serde_yaml::from_str(
            "operating_days: 90\nrollup:\n  critical_inheritance: false\ntargets:\n  full: 4\n",
        )
        .unwrap();
        assert_eq!(p.operating_days, 90);
        assert!(!p.rollup.critical_inheritance);
        assert_eq!(p.rollup.unknown_floor, 1.0);
        assert_eq!(p.targets.full, 4);
        assert_eq!(p.targets.simplified, 3);
        assert_eq!(p.gates, GateThresholds::default());
    }

    #[test]
    fn unknown_and_out_of_range_parameters_are_refused() {
        assert!(serde_yaml::from_str::<HouseParams>("operating_dayz: 90\n").is_err());
        let p: HouseParams = serde_yaml::from_str("operating_share: 1.5\n").unwrap();
        assert!(p.validate().unwrap_err().contains("operating_share"));
        let p: HouseParams = serde_yaml::from_str("expected_status:\n  dora.art24.6: 4\n").unwrap();
        assert!(p.validate().unwrap_err().contains("dora.art24.6"));
    }
}
