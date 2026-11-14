//! House parameters of the DORA coverage and maturity scores.
//!
//! Nothing in this file is regulatory text. Every number is a house
//! choice: named here, documented in `docs/guides/dora-score.md`, and
//! overridable per deployment from the `score:` section of `org.yaml`
//! (any subset of fields; missing fields keep the defaults below).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tumult_core::compliance::requirement;

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
    /// Fixed at 1, the bottom of the scale: the index of an unknown node
    /// and what it enters its parent's mean with. The key is kept so an
    /// org file that names it still reads; the roll-up never reads it,
    /// and any other value is refused.
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
    /// Expected evidence status per requirement id, overriding the
    /// registry default: 2 (existing) or 3 (operating). Designed is a
    /// declaration and is never an expectation.
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
            ("tlpt", "security"),
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

/// Tolerance when a value is compared with a threshold: a value within
/// this distance below the threshold holds. It absorbs the rounding of
/// the products `outcome · strength · freshness` (0.75 is not always
/// exactly 0.75 in binary) and nothing else.
pub const VALUE_EPS: f64 = 1e-9;

/// The largest weight a parameter may give. Weights only matter
/// relative to each other, and the sums they enter stay far from the
/// end of the floating-point range.
pub const MAX_WEIGHT: f64 = 1000.0;

/// The longest window or streak a parameter may ask for, in days: ten
/// years. Past it a window is no window, and the arithmetic on
/// nanoseconds has no room left.
pub const MAX_DAYS: u32 = 3650;

/// Whether a value reaches a threshold, within [`VALUE_EPS`]. Nothing
/// reaches nothing: a value of zero holds at no threshold, so a zeroed
/// threshold cannot turn the absence of evidence into a pass.
#[must_use]
pub fn reaches(value: f64, threshold: f64) -> bool {
    value > 0.0 && value + VALUE_EPS >= threshold
}

impl HouseParams {
    /// Whether a cell value reaches the Existing threshold, within
    /// [`VALUE_EPS`].
    #[must_use]
    pub fn holds(&self, value: f64) -> bool {
        reaches(value, self.existing_threshold)
    }

    /// Check that the parameters describe a usable model: every share
    /// and factor within 0..=1, every threshold above zero, every gate
    /// threshold a percentage above zero and no gate easier than the one
    /// below it, weights that sum to one where the formula needs it,
    /// levels and day counts in range, and every name one the model
    /// knows.
    ///
    /// # Errors
    /// Names the first parameter that is out of range.
    pub fn validate(&self) -> Result<(), String> {
        self.check_shares()?;
        self.check_weights()?;
        self.check_gates()?;
        self.check_levels_and_days()?;
        self.check_order()?;
        self.check_names()
    }

    fn check_shares(&self) -> Result<(), String> {
        let (o, s, w) = (&self.outcome, &self.strength, &self.pillar_weights);
        // A threshold of zero is reached by nothing at all.
        for (name, value) in [
            ("existing_threshold", self.existing_threshold),
            ("operating_share", self.operating_share),
            ("band_good", self.band_good),
            ("band_fair", self.band_fair),
        ] {
            above_zero(name, value, 1.0)?;
        }
        // An unknown share of one lets a wholly unknown entity through.
        let unknown = self.gates.l3_max_unknown_share;
        if !(unknown.is_finite() && (0.0..1.0).contains(&unknown)) {
            return Err(format!(
                "score.gates.l3_max_unknown_share must be at least 0 and below 1, got {unknown}"
            ));
        }
        let mut shares = vec![
            ("outcome.passed".to_string(), o.passed),
            ("outcome.partial".to_string(), o.partial),
            ("outcome.failed".to_string(), o.failed),
            ("strength.direct".to_string(), s.direct),
            ("strength.supporting".to_string(), s.supporting),
            ("strength.indirect".to_string(), s.indirect),
            ("strength.manual_test".to_string(), s.manual_test),
            ("slo.low_budget".to_string(), self.slo.low_budget),
            (
                "slo.low_budget_credit".to_string(),
                self.slo.low_budget_credit,
            ),
            ("loop_closure_weight".to_string(), self.loop_closure_weight),
            (
                "loop_previously_tested_weight".to_string(),
                self.loop_previously_tested_weight,
            ),
        ];
        for (set, weights) in [("full", &w.full), ("reduced", &w.reduced)] {
            for (pillar, value) in weights.named() {
                shares.push((format!("pillar_weights.{set}.{pillar}"), value));
            }
        }
        for (kind, omega) in &self.rollup.own_share {
            shares.push((format!("rollup.own_share.{kind}"), *omega));
        }
        for (name, value) in shares {
            within(&name, value, 0.0, 1.0)?;
        }
        Ok(())
    }

    fn check_weights(&self) -> Result<(), String> {
        let c = &self.criticality;
        for (name, value, top) in [
            ("criticality.critical", c.critical, MAX_WEIGHT),
            ("criticality.important", c.important, MAX_WEIGHT),
            ("criticality.other", c.other, MAX_WEIGHT),
            // Above 1 the fade outlasts the window it is a fraction of.
            (
                "freshness_decay_fraction",
                self.freshness_decay_fraction,
                1.0,
            ),
        ] {
            if !(value.is_finite() && value > 0.0 && value <= top) {
                return Err(format!(
                    "score.{name} must be above 0 and at most {top}, got {value}"
                ));
            }
        }
        let mut weights = vec![
            ("slo.blocking_weight".to_string(), self.slo.blocking_weight),
            ("slo.major_weight".to_string(), self.slo.major_weight),
            ("slo.minor_weight".to_string(), self.slo.minor_weight),
            (
                "rollup.unassigned_weight".to_string(),
                self.rollup.unassigned_weight,
            ),
        ];
        for (kind, weight) in &self.kind_weights {
            weights.push((format!("kind_weights.{kind}"), *weight));
        }
        for (name, value) in weights {
            within(&name, value, 0.0, MAX_WEIGHT)?;
        }
        if !self.kind_weights.values().any(|weight| *weight > 0.0) {
            return Err("score.kind_weights must weigh at least one discipline".into());
        }
        let w = &self.pillar_weights;
        for (set, weights) in [("full", &w.full), ("reduced", &w.reduced)] {
            let sum: f64 = weights.named().iter().map(|(_, value)| value).sum();
            if (sum - 1.0).abs() > WEIGHT_SUM_EPS {
                return Err(format!(
                    "score.pillar_weights.{set} must sum to 1, got {sum}"
                ));
            }
        }
        let sum = self.loop_closure_weight + self.loop_previously_tested_weight;
        if (sum - 1.0).abs() > WEIGHT_SUM_EPS {
            return Err(format!(
                "score.loop_closure_weight and score.loop_previously_tested_weight must sum \
                 to 1, got {sum}"
            ));
        }
        Ok(())
    }

    fn check_gates(&self) -> Result<(), String> {
        let g = &self.gates;
        // Each chain is one pillar across the levels that gate on it.
        let chains: [&[(&str, f64)]; 8] = [
            &[("l2_c1", g.l2_c1), ("l3_c1", g.l3_c1)],
            &[("l3_c2", g.l3_c2)],
            &[("l4_c3", g.l4_c3)],
            &[("l5_c4", g.l5_c4)],
            &[
                ("l2_coverage", g.l2_coverage),
                ("l3_coverage", g.l3_coverage),
                ("l4_coverage", g.l4_coverage),
                ("l5_coverage", g.l5_coverage),
            ],
            &[
                ("l3_slo", g.l3_slo),
                ("l4_slo", g.l4_slo),
                ("l5_slo", g.l5_slo),
            ],
            &[
                ("l3_recovery", g.l3_recovery),
                ("l4_recovery", g.l4_recovery),
                ("l5_recovery", g.l5_recovery),
            ],
            &[("l4_loop", g.l4_loop), ("l5_loop", g.l5_loop)],
        ];
        for chain in chains {
            for (name, value) in chain {
                above_zero(&format!("gates.{name}"), *value, 100.0)?;
            }
        }
        for chain in chains {
            for pair in chain.windows(2) {
                let ((below, low), (name, value)) = (pair[0], pair[1]);
                if value < low {
                    return Err(format!(
                        "score.gates.{name} must be at least score.gates.{below} ({low}), \
                         got {value}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_levels_and_days(&self) -> Result<(), String> {
        let days = [
            ("operating_days", self.operating_days),
            ("test_window_days", self.test_window_days),
            ("recovery_window_days", self.recovery_window_days),
            ("loop_window_days", self.loop_window_days),
            ("conformance.window_days", self.conformance.window_days),
            ("gates.l3_slo_history_days", self.gates.l3_slo_history_days),
        ];
        for (name, value) in days {
            if !(1..=MAX_DAYS).contains(&value) {
                return Err(format!(
                    "score.{name} must be within 1..={MAX_DAYS}, got {value}"
                ));
            }
        }
        let (t, c) = (&self.targets, &self.conformance_ceiling);
        let levels = [
            ("targets.full", t.full, 5),
            ("targets.full_tlpt_designated", t.full_tlpt_designated, 5),
            ("targets.full_microenterprise", t.full_microenterprise, 5),
            ("targets.simplified", t.simplified, 5),
            (
                "targets.simplified_microenterprise",
                t.simplified_microenterprise,
                5,
            ),
            ("conformance_ceiling.full", c.full, 4),
            ("conformance_ceiling.reduced", c.reduced, 4),
        ];
        for (name, value, top) in levels {
            if !(1..=top).contains(&value) {
                return Err(format!(
                    "score.{name} must be within 1..={top}, got {value}"
                ));
            }
        }
        self.check_unknown_floor()?;
        for (id, status) in &self.expected_status {
            if !(2..=3).contains(status) {
                return Err(format!(
                    "score.expected_status.{id} must be 2 (existing) or 3 (operating): a \
                     declaration alone meets no expectation, got {status}"
                ));
            }
        }
        Ok(())
    }

    /// A node nothing was ingested for sits at the bottom of the scale;
    /// a higher floor would hand it a score from the org file.
    fn check_unknown_floor(&self) -> Result<(), String> {
        let floor = self.rollup.unknown_floor;
        within("rollup.unknown_floor", floor, 1.0, 5.0)?;
        if floor > 1.0 {
            return Err(format!(
                "score.rollup.unknown_floor is fixed at 1 (a node without ingested evidence \
                 cannot score above the bottom of the scale), got {floor}"
            ));
        }
        Ok(())
    }

    /// Weaker evidence never counts for more than stronger evidence.
    fn check_order(&self) -> Result<(), String> {
        let (o, s) = (&self.outcome, &self.strength);
        let pairs = [
            ("outcome.failed", o.failed, "outcome.partial", o.partial),
            ("outcome.partial", o.partial, "outcome.passed", o.passed),
            (
                "strength.indirect",
                s.indirect,
                "strength.supporting",
                s.supporting,
            ),
            (
                "strength.supporting",
                s.supporting,
                "strength.direct",
                s.direct,
            ),
            ("band_fair", self.band_fair, "band_good", self.band_good),
        ];
        for (name, value, above, top) in pairs {
            if value > top {
                return Err(format!(
                    "score.{name} must be at most score.{above} ({top}), got {value}"
                ));
            }
        }
        Ok(())
    }

    fn check_names(&self) -> Result<(), String> {
        let unknown_row =
            |name: &str, id: &str| format!("score.{name}: '{id}' is not a requirement id");
        for id in self.expected_status.keys() {
            if requirement(id).is_none() {
                return Err(unknown_row(&format!("expected_status.{id}"), id));
            }
        }
        for (name, id) in [
            ("simplified_test_row", &self.simplified_test_row),
            ("tlpt_row", &self.tlpt_row),
        ] {
            if requirement(id).is_none() {
                return Err(unknown_row(name, id));
            }
        }
        for id in &self.management_report_rows {
            if requirement(id).is_none() {
                return Err(unknown_row("management_report_rows", id));
            }
        }
        for kind in self.rollup.own_share.keys() {
            node_kind(&format!("rollup.own_share.{kind}"), kind)?;
        }
        for (kind, pillars) in &self.rollup.own_pillars {
            let name = format!("rollup.own_pillars.{kind}");
            node_kind(&name, kind)?;
            if let Some(letter) = pillars
                .iter()
                .find(|p| !PILLAR_LETTERS.contains(&p.as_str()))
            {
                return Err(format!(
                    "score.{name}: '{letter}' is not one of {PILLAR_LETTERS:?}"
                ));
            }
        }
        for (exercise, kind) in &self.manual_kinds {
            if !self.kind_weights.contains_key(kind) {
                return Err(format!(
                    "score.manual_kinds.{exercise}: '{kind}' has no weight in score.kind_weights"
                ));
            }
        }
        for (level, keys) in self.conformance.levels().iter().enumerate() {
            // A level with no key is reached by anything that was seen.
            if keys.is_empty() {
                return Err(format!(
                    "score.conformance.level{} must name at least one attribute key",
                    level + 1
                ));
            }
            if keys.iter().any(|key| key.trim().is_empty()) {
                return Err(format!(
                    "score.conformance.level{}: an attribute key is empty",
                    level + 1
                ));
            }
        }
        Ok(())
    }

    /// Every parameter that differs from the house default, by its
    /// dotted path in the `score:` section, sorted by path. A key the
    /// defaults have and the parameters lack shows the value `null`.
    #[must_use]
    pub fn overrides(&self) -> Vec<Override> {
        let own = serde_json::to_value(self).unwrap_or(Value::Null);
        let default = serde_json::to_value(Self::default()).unwrap_or(Value::Null);
        let mut out = Vec::new();
        diff("", &own, &default, &mut out);
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }
}

/// One parameter set to something other than the house default.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Override {
    /// Dotted path below `score:`.
    pub key: String,
    pub value: Value,
    pub default: Value,
}

fn diff(path: &str, own: &Value, default: &Value, out: &mut Vec<Override>) {
    if own == default {
        return;
    }
    let (Some(own_map), Some(default_map)) = (own.as_object(), default.as_object()) else {
        out.push(Override {
            key: path.to_string(),
            value: own.clone(),
            default: default.clone(),
        });
        return;
    };
    let keys: std::collections::BTreeSet<&String> =
        own_map.keys().chain(default_map.keys()).collect();
    for key in keys {
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        diff(
            &child,
            own_map.get(key).unwrap_or(&Value::Null),
            default_map.get(key).unwrap_or(&Value::Null),
            out,
        );
    }
}

/// How far a set of weights may be from summing to one.
const WEIGHT_SUM_EPS: f64 = 1e-6;
/// Node kinds the roll-up knows.
const NODE_KINDS: [&str; 6] = [
    "workload",
    "service",
    "component",
    "system",
    "entity",
    "org",
];
/// Pillar letters a node can observe.
const PILLAR_LETTERS: [&str; 5] = ["C", "T", "S", "R", "L"];

fn within(name: &str, value: f64, low: f64, high: f64) -> Result<(), String> {
    if value.is_finite() && (low..=high).contains(&value) {
        return Ok(());
    }
    Err(format!(
        "score.{name} must be within {low}..={high}, got {value}"
    ))
}

fn above_zero(name: &str, value: f64, high: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value <= high {
        return Ok(());
    }
    Err(format!(
        "score.{name} must be above 0 and at most {high}, got {value}"
    ))
}

fn node_kind(name: &str, kind: &str) -> Result<(), String> {
    if NODE_KINDS.contains(&kind) {
        return Ok(());
    }
    Err(format!(
        "score.{name}: '{kind}' is not one of {NODE_KINDS:?}"
    ))
}

impl PillarWeights {
    /// The five weights with their names in the `score:` section.
    #[must_use]
    pub fn named(&self) -> [(&'static str, f64); 5] {
        [
            ("conformance", self.conformance),
            ("coverage", self.coverage),
            ("slo", self.slo),
            ("recovery", self.recovery),
            ("loop", self.loop_),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_existing_threshold_holds_within_the_stated_tolerance() {
        let p = HouseParams::default();
        assert!(p.holds(0.75));
        assert!(p.holds(1.0 * 0.75 * 1.0));
        // 0.1 · 7.5 and 3 · 0.25 − 2e-10 are 0.75 up to rounding.
        assert!(p.holds(0.75 - VALUE_EPS / 2.0));
        assert!(!p.holds(0.75 - 2.0 * VALUE_EPS));
        assert!(!p.holds(0.7499));
        assert!(!p.holds(f64::NAN));
    }

    /// Validation refuses a zero threshold; should one get past it,
    /// a cell with no evidence at all still does not hold.
    #[test]
    fn nothing_never_holds_whatever_the_threshold() {
        let zeroed = HouseParams {
            existing_threshold: 0.0,
            ..HouseParams::default()
        };
        assert!(!zeroed.holds(0.0));
        assert!(zeroed.holds(0.01));
    }

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

    /// The error of validating the overrides in `yaml`.
    fn refused(yaml: &str) -> String {
        let params: HouseParams =
            serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"));
        params.validate().expect_err(&format!("accepted:\n{yaml}"))
    }

    fn assert_refused(cases: &[(&str, &str)]) {
        for (yaml, names) in cases {
            let error = refused(yaml);
            assert!(error.contains(names), "{yaml} gave: {error}");
        }
    }

    #[test]
    fn every_share_and_factor_is_finite_and_within_0_to_1() {
        assert_refused(&[
            ("existing_threshold: -0.1\n", "score.existing_threshold"),
            ("existing_threshold: .nan\n", "score.existing_threshold"),
            ("operating_share: .inf\n", "score.operating_share"),
            ("band_good: 1.2\n", "score.band_good"),
            ("band_fair: -1\n", "score.band_fair"),
            ("outcome: {passed: 1.5}\n", "score.outcome.passed"),
            ("outcome: {partial: -0.5}\n", "score.outcome.partial"),
            ("outcome: {failed: 2}\n", "score.outcome.failed"),
            ("strength: {direct: 3}\n", "score.strength.direct"),
            (
                "strength: {supporting: 1.01}\n",
                "score.strength.supporting",
            ),
            ("strength: {indirect: -0.01}\n", "score.strength.indirect"),
            ("strength: {manual_test: 7}\n", "score.strength.manual_test"),
            ("slo: {low_budget: 1.5}\n", "score.slo.low_budget"),
            (
                "slo: {low_budget_credit: -1}\n",
                "score.slo.low_budget_credit",
            ),
            ("loop_closure_weight: 1.5\n", "score.loop_closure_weight"),
            (
                "gates: {l3_max_unknown_share: 50}\n",
                "score.gates.l3_max_unknown_share",
            ),
            (
                "rollup: {own_share: {service: 1.5}}\n",
                "score.rollup.own_share.service",
            ),
            (
                "pillar_weights: {full: {conformance: -0.2, coverage: 0.65, slo: 0.25, \
                 recovery: 0.2, loop: 0.1}}\n",
                "score.pillar_weights.full.conformance",
            ),
        ]);
    }

    const GATE_NAMES: [&str; 17] = [
        "l2_c1",
        "l2_coverage",
        "l3_c1",
        "l3_c2",
        "l3_coverage",
        "l3_slo",
        "l3_recovery",
        "l4_c3",
        "l4_coverage",
        "l4_slo",
        "l4_recovery",
        "l4_loop",
        "l5_c4",
        "l5_coverage",
        "l5_slo",
        "l5_recovery",
        "l5_loop",
    ];

    /// An unknown node enters at 1 and nowhere else: the key reads, and
    /// any other value is refused by name.
    #[test]
    fn the_unknown_floor_is_fixed_at_1() {
        let named: HouseParams = serde_yaml::from_str("rollup: {unknown_floor: 1.0}\n").unwrap();
        named.validate().unwrap();
        for value in ["1.5", "3", "5.0"] {
            let error = refused(&format!("rollup: {{unknown_floor: {value}}}\n"));
            assert!(
                error.contains("score.rollup.unknown_floor is fixed at 1"),
                "{value} gave: {error}"
            );
        }
    }

    /// Designed is a declaration in the org file: it is never what the
    /// house expects of a row.
    #[test]
    fn an_expected_status_is_existing_or_operating() {
        for accepted in ["2", "3"] {
            let yaml = format!("expected_status: {{dora.art12.2: {accepted}}}\n");
            let params: HouseParams = serde_yaml::from_str(&yaml).unwrap();
            params.validate().unwrap();
        }
        for value in ["0", "1", "4"] {
            let error = refused(&format!("expected_status: {{dora.art12.2: {value}}}\n"));
            assert!(
                error.contains("score.expected_status.dora.art12.2 must be 2 (existing) or 3"),
                "{value} gave: {error}"
            );
        }
    }

    /// A weight is a ratio between things of one kind. Past the cap the
    /// sums it enters overflow, and the index is no number at all.
    #[test]
    fn every_weight_is_capped_and_finite() {
        let mut params = HouseParams::default();
        params.criticality.critical = MAX_WEIGHT;
        params.slo.blocking_weight = MAX_WEIGHT;
        params.rollup.unassigned_weight = MAX_WEIGHT;
        for weight in params.kind_weights.values_mut() {
            *weight = MAX_WEIGHT;
        }
        params.validate().unwrap();
        let names = [
            (
                "criticality: {critical: VALUE}",
                "score.criticality.critical",
            ),
            (
                "criticality: {important: VALUE}",
                "score.criticality.important",
            ),
            ("criticality: {other: VALUE}", "score.criticality.other"),
            ("kind_weights: {dr: VALUE}", "score.kind_weights.dr"),
            ("kind_weights: {chaos: VALUE}", "score.kind_weights.chaos"),
            ("slo: {blocking_weight: VALUE}", "score.slo.blocking_weight"),
            ("slo: {major_weight: VALUE}", "score.slo.major_weight"),
            ("slo: {minor_weight: VALUE}", "score.slo.minor_weight"),
            (
                "rollup: {unassigned_weight: VALUE}",
                "score.rollup.unassigned_weight",
            ),
        ];
        for (yaml, name) in names {
            for value in ["1e308", "1000.5", ".inf", "-.inf", ".nan"] {
                let error = refused(&format!("{}\n", yaml.replace("VALUE", value)));
                assert!(error.contains(name), "{yaml} {value} gave: {error}");
            }
        }
    }

    /// Evidence past its window fades: a decay fraction above 1 would
    /// keep it for good.
    #[test]
    fn the_decay_fraction_is_at_most_1() {
        for value in ["1.5", "1e308", ".inf"] {
            let error = refused(&format!("freshness_decay_fraction: {value}\n"));
            assert!(
                error.contains("score.freshness_decay_fraction"),
                "{value} gave: {error}"
            );
        }
        let top: HouseParams = serde_yaml::from_str("freshness_decay_fraction: 1.0\n").unwrap();
        top.validate().unwrap();
    }

    /// A level with no key of its own is reached by every service that
    /// was seen at all: a conformance level nothing was ingested for.
    #[test]
    fn every_conformance_level_names_a_key() {
        for level in 1..=4 {
            let error = refused(&format!("conformance: {{level{level}: []}}\n"));
            assert!(
                error.contains(&format!("score.conformance.level{level} must name")),
                "level {level} gave: {error}"
            );
        }
    }

    #[test]
    fn every_gate_threshold_is_a_percentage() {
        for name in GATE_NAMES {
            for bad in ["-1", "100.5", ".nan"] {
                let error = refused(&format!("gates: {{{name}: {bad}}}\n"));
                assert!(
                    error.contains(&format!("score.gates.{name} ")),
                    "{name}: {bad} gave: {error}"
                );
            }
        }
    }

    /// A threshold of zero is reached by nothing at all, an unknown
    /// share of one lets a wholly unknown entity through, and a window
    /// of more than ten years is no window.
    #[test]
    fn a_zero_threshold_a_full_unknown_share_and_an_endless_window_are_refused() {
        assert_refused(&[
            ("existing_threshold: 0\n", "score.existing_threshold"),
            ("operating_share: 0\n", "score.operating_share"),
            ("band_good: 0\n", "score.band_good"),
            ("band_fair: 0\n", "score.band_fair"),
            (
                "gates: {l3_max_unknown_share: 1}\n",
                "score.gates.l3_max_unknown_share",
            ),
            ("operating_days: 3651\n", "score.operating_days"),
            ("test_window_days: 3651\n", "score.test_window_days"),
            (
                "recovery_window_days: 4000000000\n",
                "score.recovery_window_days",
            ),
            ("loop_window_days: 36500\n", "score.loop_window_days"),
            (
                "conformance: {window_days: 4294967295}\n",
                "score.conformance.window_days",
            ),
            (
                "gates: {l3_slo_history_days: 3651}\n",
                "score.gates.l3_slo_history_days",
            ),
        ]);
        for name in GATE_NAMES {
            let error = refused(&format!("gates: {{{name}: 0}}\n"));
            assert!(
                error.contains(&format!("score.gates.{name} ")),
                "{name}: 0 gave: {error}"
            );
        }
        // The bounds themselves are valid.
        let edge: HouseParams = serde_yaml::from_str(
            "existing_threshold: 1\noperating_share: 1\noperating_days: 3650\n\
             gates: {l3_max_unknown_share: 0, l3_slo_history_days: 3650}\n",
        )
        .unwrap();
        assert_eq!(edge.validate(), Ok(()));
    }

    #[test]
    fn a_gate_is_never_easier_than_the_one_below_it() {
        assert_refused(&[
            ("gates: {l3_coverage: 10}\n", "score.gates.l3_coverage"),
            ("gates: {l4_coverage: 50}\n", "score.gates.l4_coverage"),
            ("gates: {l5_coverage: 70}\n", "score.gates.l5_coverage"),
            ("gates: {l3_c1: 40}\n", "score.gates.l3_c1"),
            ("gates: {l4_slo: 70}\n", "score.gates.l4_slo"),
            ("gates: {l5_slo: 85}\n", "score.gates.l5_slo"),
            ("gates: {l4_recovery: 50}\n", "score.gates.l4_recovery"),
            ("gates: {l5_recovery: 70}\n", "score.gates.l5_recovery"),
            ("gates: {l5_loop: 50}\n", "score.gates.l5_loop"),
        ]);
        // Lowering a whole chain in order is a valid house choice.
        let lowered: HouseParams = serde_yaml::from_str(
            "gates: {l2_coverage: 10, l3_coverage: 20, l4_coverage: 30, l5_coverage: 40}\n",
        )
        .unwrap();
        lowered.validate().unwrap();
    }

    #[test]
    fn pillar_and_loop_weights_sum_to_one() {
        assert_refused(&[
            (
                "pillar_weights: {full: {conformance: 0.2, coverage: 0.25, slo: 0.25, \
                 recovery: 0.2, loop: 0.2}}\n",
                "score.pillar_weights.full must sum to 1",
            ),
            (
                "pillar_weights: {reduced: {conformance: 0.2, coverage: 0.3, slo: 0.15, \
                 recovery: 0.2, loop: 0.05}}\n",
                "score.pillar_weights.reduced must sum to 1",
            ),
            (
                "loop_closure_weight: 0.5\n",
                "score.loop_closure_weight and score.loop_previously_tested_weight must sum to 1",
            ),
        ]);
    }

    #[test]
    fn weights_are_finite_and_not_negative_and_some_must_be_positive() {
        assert_refused(&[
            ("criticality: {critical: 0}\n", "score.criticality.critical"),
            (
                "criticality: {important: -2}\n",
                "score.criticality.important",
            ),
            ("criticality: {other: .inf}\n", "score.criticality.other"),
            (
                "freshness_decay_fraction: 0\n",
                "score.freshness_decay_fraction",
            ),
            (
                "freshness_decay_fraction: .nan\n",
                "score.freshness_decay_fraction",
            ),
            ("kind_weights: {chaos: -1}\n", "score.kind_weights.chaos"),
            ("kind_weights: {chaos: .inf}\n", "score.kind_weights.chaos"),
            ("kind_weights: {chaos: 0}\n", "score.kind_weights"),
            ("slo: {blocking_weight: -3}\n", "score.slo.blocking_weight"),
            ("slo: {major_weight: .nan}\n", "score.slo.major_weight"),
            ("slo: {minor_weight: -1}\n", "score.slo.minor_weight"),
            (
                "rollup: {unassigned_weight: -1}\n",
                "score.rollup.unassigned_weight",
            ),
        ]);
    }

    #[test]
    fn levels_and_day_counts_are_in_range() {
        assert_refused(&[
            ("operating_days: 0\n", "score.operating_days"),
            ("test_window_days: 0\n", "score.test_window_days"),
            ("recovery_window_days: 0\n", "score.recovery_window_days"),
            ("loop_window_days: 0\n", "score.loop_window_days"),
            (
                "conformance: {window_days: 0}\n",
                "score.conformance.window_days",
            ),
            (
                "gates: {l3_slo_history_days: 0}\n",
                "score.gates.l3_slo_history_days",
            ),
            ("targets: {full: 0}\n", "score.targets.full"),
            (
                "targets: {full_tlpt_designated: 6}\n",
                "score.targets.full_tlpt_designated",
            ),
            ("targets: {simplified: 9}\n", "score.targets.simplified"),
            (
                "conformance_ceiling: {full: 5}\n",
                "score.conformance_ceiling.full",
            ),
            (
                "conformance_ceiling: {reduced: 0}\n",
                "score.conformance_ceiling.reduced",
            ),
            (
                "rollup: {unknown_floor: 0.5}\n",
                "score.rollup.unknown_floor",
            ),
            (
                "rollup: {unknown_floor: 5.5}\n",
                "score.rollup.unknown_floor",
            ),
            (
                "expected_status: {dora.art24.6: 0}\n",
                "score.expected_status.dora.art24.6",
            ),
        ]);
    }

    #[test]
    fn a_weaker_outcome_or_mapping_never_counts_for_more() {
        assert_refused(&[
            ("outcome: {failed: 0.9}\n", "score.outcome.failed"),
            ("outcome: {partial: 0.4}\n", "score.outcome.partial"),
            ("strength: {indirect: 0.9}\n", "score.strength.indirect"),
            ("strength: {supporting: 0.1}\n", "score.strength.supporting"),
            ("band_fair: 0.9\n", "score.band_fair"),
        ]);
    }

    #[test]
    fn names_the_model_does_not_know_are_refused() {
        assert_refused(&[
            (
                "expected_status: {dora.art99.9: 2}\n",
                "score.expected_status.dora.art99.9",
            ),
            (
                "simplified_test_row: rts1774.art400\n",
                "score.simplified_test_row",
            ),
            ("tlpt_row: dora.art26.99\n", "score.tlpt_row"),
            (
                "management_report_rows: [dora.art13.5, nope]\n",
                "score.management_report_rows",
            ),
            (
                "rollup: {own_share: {cluster: 0.5}}\n",
                "score.rollup.own_share.cluster",
            ),
            (
                "rollup: {own_pillars: {cluster: [T]}}\n",
                "score.rollup.own_pillars.cluster",
            ),
            (
                "rollup: {own_pillars: {service: [X]}}\n",
                "score.rollup.own_pillars.service",
            ),
            (
                "manual_kinds: {drill: teleport}\n",
                "score.manual_kinds.drill",
            ),
            (
                "conformance: {level1: [\"\"]}\n",
                "score.conformance.level1",
            ),
        ]);
        assert!(serde_yaml::from_str::<HouseParams>("gates: {l6_loop: 1}\n").is_err());
        assert!(serde_yaml::from_str::<HouseParams>("outcome: {maybe: 1}\n").is_err());
    }

    #[test]
    fn overrides_list_every_changed_key_with_its_default() {
        assert!(HouseParams::default().overrides().is_empty());
        let p: HouseParams = serde_yaml::from_str(
            "operating_days: 90\ngates: {l2_coverage: 10, l3_coverage: 20}\n\
             kind_weights: {chaos: 5}\nexpected_status: {dora.art24.6: 2}\n",
        )
        .unwrap();
        let seen: Vec<(String, String, String)> = p
            .overrides()
            .into_iter()
            .map(|o| (o.key, o.value.to_string(), o.default.to_string()))
            .collect();
        let want = [
            ("expected_status.dora.art24.6", "2", "null"),
            ("gates.l2_coverage", "10.0", "25.0"),
            ("gates.l3_coverage", "20.0", "60.0"),
            ("kind_weights.dr", "null", "3.0"),
            ("kind_weights.load", "null", "2.0"),
            ("kind_weights.nfr", "null", "1.0"),
            ("kind_weights.performance", "null", "1.0"),
            ("kind_weights.robustness", "null", "1.0"),
            ("kind_weights.security", "null", "1.0"),
            ("kind_weights.chaos", "5.0", "3.0"),
            ("operating_days", "90", "180"),
        ];
        let mut want: Vec<(String, String, String)> = want
            .iter()
            .map(|(k, v, d)| ((*k).to_string(), (*v).to_string(), (*d).to_string()))
            .collect();
        want.sort();
        assert_eq!(seen, want);
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
