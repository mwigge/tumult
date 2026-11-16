//! Properties over every valid parameter set: what is not backed by
//! ingested evidence never raises a score, a level or a gate, whatever
//! the `score:` section of the org file says. Tests only.

use std::collections::BTreeMap;

use tumult_core::compliance::{EntityProfile, REQUIREMENTS};

use super::coverage::Matrix;
use super::maturity::{self, Scope};
use super::model::{Function, IncidentFeed, Inputs, PlannedTest, Service, SloFeed, Tier, Workload};
use super::params::{HouseParams, PillarWeights, MAX_DAYS, MAX_WEIGHT};
use super::rollup::{self, NodeScore};
use super::status::{self, Standing};
use super::worked::{bank_a, firm_b, ns};

struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1_u64 << 53) as f64)
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n - 1)
    }

    /// A value in `low..=high`, on a bound one time in three.
    fn within(&mut self, low: f64, high: f64) -> f64 {
        match self.pick(6) {
            0 => low,
            1 => high,
            _ => low + (high - low) * self.unit(),
        }
    }

    /// A value above zero and at most `high`.
    fn above_zero(&mut self, high: f64) -> f64 {
        self.within(high * 1e-9, high)
    }

    fn days(&mut self) -> u32 {
        match self.pick(4) {
            0 => 1,
            1 => MAX_DAYS,
            _ => 1 + self.pick(MAX_DAYS as usize) as u32,
        }
    }

    fn level(&mut self, top: u8) -> u8 {
        1 + self.pick(usize::from(top)) as u8
    }

    fn row(&mut self) -> String {
        REQUIREMENTS[self.pick(REQUIREMENTS.len())].id.to_string()
    }

    /// `n` values in `low..=high`, ascending.
    fn ascending(&mut self, n: usize, low: f64, high: f64) -> Vec<f64> {
        let mut values: Vec<f64> = (0..n).map(|_| self.within(low, high)).collect();
        values.sort_by(f64::total_cmp);
        values
    }

    fn pillar_weights(&mut self) -> PillarWeights {
        let mut raw: Vec<f64> = (0..5).map(|_| self.unit()).collect();
        if self.pick(4) == 0 {
            let only = self.pick(5);
            raw = (0..5).map(|i| f64::from(u8::from(i == only))).collect();
        }
        let sum: f64 = raw.iter().sum();
        PillarWeights {
            conformance: raw[0] / sum,
            coverage: raw[1] / sum,
            slo: raw[2] / sum,
            recovery: raw[3] / sum,
            loop_: raw[4] / sum,
        }
    }

    /// A parameter set anywhere inside the bounds `validate` accepts.
    fn params(&mut self) -> HouseParams {
        let mut p = HouseParams::default();
        p.criticality.critical = self.above_zero(MAX_WEIGHT);
        p.criticality.important = self.above_zero(MAX_WEIGHT);
        p.criticality.other = self.above_zero(MAX_WEIGHT);
        let outcome = self.ascending(3, 0.0, 1.0);
        (p.outcome.failed, p.outcome.partial, p.outcome.passed) =
            (outcome[0], outcome[1], outcome[2]);
        let strength = self.ascending(3, 0.0, 1.0);
        (
            p.strength.indirect,
            p.strength.supporting,
            p.strength.direct,
        ) = (strength[0], strength[1], strength[2]);
        p.strength.manual_test = self.within(0.0, 1.0);
        p.freshness_decay_fraction = self.above_zero(1.0);
        p.existing_threshold = self.above_zero(1.0);
        p.operating_days = self.days();
        p.operating_share = self.above_zero(1.0);
        for weight in p.kind_weights.values_mut() {
            *weight = self.within(0.0, MAX_WEIGHT);
        }
        p.kind_weights
            .insert("robustness".into(), self.above_zero(MAX_WEIGHT));
        p.test_window_days = self.days();
        p.recovery_window_days = self.days();
        p.loop_window_days = self.days();
        p.loop_closure_weight = self.within(0.0, 1.0);
        p.loop_previously_tested_weight = 1.0 - p.loop_closure_weight;
        p.slo.blocking_weight = self.within(0.0, MAX_WEIGHT);
        p.slo.major_weight = self.within(0.0, MAX_WEIGHT);
        p.slo.minor_weight = self.within(0.0, MAX_WEIGHT);
        p.slo.low_budget = self.within(0.0, 1.0);
        p.slo.low_budget_credit = self.within(0.0, 1.0);
        p.pillar_weights.full = self.pillar_weights();
        p.pillar_weights.reduced = self.pillar_weights();
        p.conformance_ceiling.full = self.level(4);
        p.conformance_ceiling.reduced = self.level(4);
        p.targets.full = self.level(5);
        p.targets.full_tlpt_designated = self.level(5);
        p.targets.full_microenterprise = self.level(5);
        p.targets.simplified = self.level(5);
        p.targets.simplified_microenterprise = self.level(5);
        self.gates(&mut p);
        self.rollup(&mut p);
        p.conformance.window_days = self.days();
        for _ in 0..self.pick(6) {
            let expected = 2 + self.pick(2) as u8;
            p.expected_status.insert(self.row(), expected);
        }
        p.simplified_test_row = self.row();
        p.tlpt_row = self.row();
        p.management_report_rows = (0..self.pick(4)).map(|_| self.row()).collect();
        let bands = self.ascending(2, 1e-9, 1.0);
        (p.band_fair, p.band_good) = (bands[0], bands[1]);
        p
    }

    fn gates(&mut self, p: &mut HouseParams) {
        let g = &mut p.gates;
        let c1 = self.ascending(2, 1e-7, 100.0);
        (g.l2_c1, g.l3_c1) = (c1[0], c1[1]);
        g.l3_c2 = self.above_zero(100.0);
        g.l4_c3 = self.above_zero(100.0);
        g.l5_c4 = self.above_zero(100.0);
        let t = self.ascending(4, 1e-7, 100.0);
        (g.l2_coverage, g.l3_coverage, g.l4_coverage, g.l5_coverage) = (t[0], t[1], t[2], t[3]);
        let s = self.ascending(3, 1e-7, 100.0);
        (g.l3_slo, g.l4_slo, g.l5_slo) = (s[0], s[1], s[2]);
        let r = self.ascending(3, 1e-7, 100.0);
        (g.l3_recovery, g.l4_recovery, g.l5_recovery) = (r[0], r[1], r[2]);
        let l = self.ascending(2, 1e-7, 100.0);
        (g.l4_loop, g.l5_loop) = (l[0], l[1]);
        g.l3_slo_history_days = self.days();
        g.l3_max_unknown_share = self.within(0.0, 0.999_999);
    }

    fn rollup(&mut self, p: &mut HouseParams) {
        for share in p.rollup.own_share.values_mut() {
            *share = self.within(0.0, 1.0);
        }
        for pillars in p.rollup.own_pillars.values_mut() {
            *pillars = ["C", "T", "S", "R", "L"]
                .into_iter()
                .filter(|_| self.pick(2) == 0)
                .map(str::to_string)
                .collect();
        }
        p.rollup.unassigned_weight = self.within(0.0, MAX_WEIGHT);
        p.rollup.critical_inheritance = self.pick(2) == 0;
    }
}

fn valid(rng: &mut Lcg) -> HouseParams {
    let params = rng.params();
    if let Err(error) = params.validate() {
        panic!("the generator left the valid bounds: {error}");
    }
    params
}

const PROFILES: [(&str, &[&str]); 5] = [
    ("full", &[]),
    ("full", &["tlpt_designated"]),
    ("full", &["microenterprise"]),
    ("simplified", &[]),
    ("simplified", &["microenterprise"]),
];

/// An organisation as the org file alone describes it: functions with
/// their targets, plans, declarations and org units, services and
/// workloads that never reported, and not one ingested row.
fn declared_only(profile: &str, flags: &[&str], feeds_connected: bool) -> Inputs {
    let flags: Vec<String> = flags.iter().map(|flag| (*flag).to_string()).collect();
    let entity = EntityProfile::parse(profile, &flags).unwrap_or_else(|e| panic!("{e}"));
    let mut inputs = Inputs::empty(ns("2026-11-13"), entity);
    let declared: Vec<String> = REQUIREMENTS.iter().map(|r| r.id.to_string()).collect();
    let plan: Vec<PlannedTest> = ["chaos", "dr", "performance", "security"]
        .iter()
        .map(|kind| PlannedTest {
            kind: (*kind).to_string(),
            interval_days: 90,
        })
        .collect();
    let functions = [
        ("cif-0001", Tier::Critical, Some("retail/payments")),
        ("cif-0002", Tier::Important, Some("retail")),
        ("ops-0003", Tier::Other, None),
    ];
    for (id, tier, unit) in functions {
        inputs.functions.push(Function {
            rto_s: Some(3600.0),
            rpo_s: Some(300.0),
            last_assessed_ns: Some(inputs.as_of_ns),
            org_unit: unit.map(str::to_string),
            declared: declared.clone(),
            test_plan: plan.clone(),
            scheduled: true,
            register_codes: vec!["F1".into()],
            ..Function::new(id, tier)
        });
        inputs
            .services
            .push(Service::silent(&format!("{id}-svc"), &[id]));
        inputs.workloads.push(Workload {
            id: format!("wl:prod/deployment/{id}"),
            service: Some(format!("{id}-svc")),
        });
    }
    inputs.services.push(Service::silent("stray", &[]));
    if feeds_connected {
        inputs.slos = SloFeed::Connected {
            slos: Vec::new(),
            history_days: 0,
        };
        inputs.incidents = IncidentFeed::Connected(Vec::new());
    }
    inputs
}

fn spine(inputs: &Inputs, params: &HouseParams) -> BTreeMap<String, NodeScore> {
    let matrix = Matrix::build(inputs, params);
    let entity = maturity::assess(&matrix, Scope::Entity);
    rollup::assess(&matrix, &entity)
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect()
}

fn assert_nothing_is_credited(inputs: &Inputs, params: &HouseParams, case: &str) {
    let matrix = Matrix::build(inputs, params);
    let scopes = (0..inputs.functions.len())
        .map(Scope::Function)
        .chain([Scope::Entity]);
    for scope in scopes {
        let m = maturity::assess(&matrix, scope);
        assert_eq!((m.level, m.gate_level), (1, 1), "{case} {scope:?}");
        assert!(m.index == 1.0, "{case} {scope:?}: index {}", m.index);
        for gate in &m.gates {
            assert!(
                !gate.passed,
                "{case} {scope:?}: gate {} is open",
                gate.level
            );
        }
        let report = status::report(&matrix, scope.function());
        let met: Vec<&str> = report
            .rows
            .iter()
            .filter(|row| row.standing == Standing::Meets)
            .map(|row| row.req.id)
            .collect();
        assert!(
            met.is_empty(),
            "{case} {scope:?}: {met:?} meet with no evidence"
        );
        let coverage = matrix.portfolio(scope.function()).coverage;
        assert!(
            coverage.unwrap_or(0.0) == 0.0,
            "{case} {scope:?}: coverage {coverage:?}"
        );
    }
    for (id, node) in spine(inputs, params) {
        assert_eq!(node.level, 1, "{case} {id}");
        assert!(node.index == 1.0, "{case} {id}: index {}", node.index);
    }
}

#[test]
fn property_an_organisation_with_no_ingested_evidence_is_level_1_at_the_floor() {
    let mut rng = Lcg(23);
    let mut sets = vec![HouseParams::default()];
    sets.extend((0..150).map(|_| valid(&mut rng)));
    for (n, params) in sets.iter().enumerate() {
        for (profile, flags) in PROFILES {
            for feeds in [false, true] {
                let inputs = declared_only(profile, flags, feeds);
                let case = format!("set {n} {profile} {flags:?} feeds {feeds}");
                assert_nothing_is_credited(&inputs, params, &case);
            }
        }
    }
}

/// Adding a declared service that never reported, under any function or
/// under none, raises no index and no level anywhere on the spine, and
/// not the entity's own.
#[test]
fn property_a_silent_declared_service_never_raises_an_index_or_a_level() {
    let mut rng = Lcg(29);
    for (name, base) in [("firm_b", firm_b()), ("bank_a", bank_a())] {
        for n in 0..40 {
            let params = if n == 0 {
                HouseParams::default()
            } else {
                valid(&mut rng)
            };
            let before = spine(&base, &params);
            let was = maturity::assess(&Matrix::build(&base, &params), Scope::Entity);
            let mut inputs = base.clone();
            for s in 0..=rng.pick(3) {
                let at = rng.pick(base.functions.len() + 1);
                let functions: Vec<&str> = base
                    .functions
                    .get(at)
                    .map(|f| f.id.as_str())
                    .into_iter()
                    .collect();
                inputs
                    .services
                    .push(Service::silent(&format!("zz-silent-{s}"), &functions));
            }
            let after = spine(&inputs, &params);
            let is = maturity::assess(&Matrix::build(&inputs, &params), Scope::Entity);
            assert!(is.index <= was.index + 1e-9, "{name} set {n}: entity index");
            assert!(is.level <= was.level, "{name} set {n}: entity level");
            for (id, node) in &before {
                let now = &after[id];
                assert!(
                    now.index <= node.index + 1e-9,
                    "{name} set {n} {id}: {} > {}",
                    now.index,
                    node.index
                );
                assert!(now.level <= node.level, "{name} set {n} {id}: level");
            }
        }
    }
}
