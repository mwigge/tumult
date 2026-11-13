//! Maturity: five pillars, an index, and a gated internal level.
//!
//! The pillars are each 0–100 and are evaluated on a scope, either the
//! legal entity or one function:
//!
//! - **C** conformance: criticality-weighted telemetry conformance level
//!   of the services, over the profile's ceiling `Lmax`.
//! - **T** test coverage: discipline coverage of the critical and
//!   important functions (full), the yearly restore test (simplified), or
//!   the function's own plan (microenterprise).
//! - **S** SLO attainment, weighted by criticality.
//! - **R** recovery: share of functions whose latest measured recovery
//!   met its target.
//! - **L** loop closure: incidents and findings closed by a passing test.
//!
//! ```text
//! index = 1 + 4 · Σ w_k · X_k / (100 · Σ w_k)        over pillars not excluded
//! level = min(highest gate passed, floor(index))
//! ```
//!
//! A pillar that is `unknown` counts as 0; one that is excluded (not
//! declared, no events, out of scope) leaves the sum. The level is an
//! internal scale: no regulation defines it.

use std::collections::BTreeMap;

use tumult_core::compliance::{Requirement, CORE_WEIGHT_MIN, GATING_WEIGHT};

use super::coverage::{recovery_met, CellState, Ctx, Matrix};
use super::model::{
    day_of, format_day, Evidence, Function, IncidentFeed, Service, Slo, SloCriticality, SloFeed,
};
use super::params::{HouseParams, PillarWeights};
use super::status::{self, Standing, Status};

/// What the pillars and gates are evaluated on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The legal entity: every function.
    Entity,
    /// One function, by index into `Inputs::functions`.
    Function(usize),
}

impl Scope {
    /// The function index, when the scope is one function.
    #[must_use]
    pub fn function(self) -> Option<usize> {
        match self {
            Self::Entity => None,
            Self::Function(index) => Some(index),
        }
    }
}

/// Why a pillar is left out of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Excluded {
    /// The feed is connected and nothing happened.
    NoEvents,
    /// The entity declares nothing of this kind and its profile does not
    /// require it.
    NotDeclared,
    /// Nothing in scope is bound by the pillar.
    OutOfScope,
}

impl Excluded {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoEvents => "no_events",
            Self::NotDeclared => "not_declared",
            Self::OutOfScope => "out_of_scope",
        }
    }
}

/// One pillar's value or the reason it has none.
#[derive(Debug, Clone, PartialEq)]
pub enum Pillar {
    Value(f64),
    /// Not observable; counts as 0 in the index and fails a gate.
    Unknown(String),
    /// Left out of the index; passes the gates of levels 2 and 3.
    Excluded(Excluded),
}

impl Pillar {
    /// The value, when there is one.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        match self {
            Self::Value(v) => Some(*v),
            Self::Unknown(_) | Self::Excluded(_) => None,
        }
    }

    /// What the pillar contributes to the index: its value, 0 when
    /// unknown, nothing when excluded.
    #[must_use]
    pub fn counted(&self) -> Option<f64> {
        match self {
            Self::Value(v) => Some(*v),
            Self::Unknown(_) => Some(0.0),
            Self::Excluded(_) => None,
        }
    }

    /// `value`, `unknown`, or the exclusion name.
    #[must_use]
    pub fn state(&self) -> &'static str {
        match self {
            Self::Value(_) => "value",
            Self::Unknown(_) => "unknown",
            Self::Excluded(why) => why.as_str(),
        }
    }

    /// Whether the pillar reaches a threshold. An excluded pillar passes
    /// unless the gate demands a value; an unknown one never does.
    fn reaches(&self, threshold: f64, value_required: bool) -> bool {
        match self {
            Self::Value(v) => *v + 1e-9 >= threshold,
            Self::Unknown(_) => false,
            Self::Excluded(_) => !value_required,
        }
    }
}

/// The five pillars of a scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Pillars {
    pub conformance: Pillar,
    pub coverage: Pillar,
    pub slo: Pillar,
    pub recovery: Pillar,
    pub loop_: Pillar,
    /// `C_1`…`C_4`: weighted share of services at or above each
    /// conformance level, 0–100. `None` when no service is in scope.
    pub shares: Option<[f64; 4]>,
}

/// `1 + 4 · Σ w·X / (100 · Σ w)` over the pillars that count. `None`
/// when every pillar is excluded.
#[must_use]
pub fn index_of(parts: &[(f64, &Pillar)]) -> Option<f64> {
    let mut sum = 0.0;
    let mut weights = 0.0;
    for (weight, pillar) in parts {
        let Some(value) = pillar.counted() else {
            continue;
        };
        sum += weight * value;
        weights += weight;
    }
    (weights > 0.0).then(|| 1.0 + 4.0 * sum / (100.0 * weights))
}

/// The profile's pillar weights.
#[must_use]
pub fn weights<'p>(matrix: &Matrix<'p>) -> &'p PillarWeights {
    let sets = &matrix.ctx.params.pillar_weights;
    if matrix.ctx.inputs.entity.is_reduced() {
        return &sets.reduced;
    }
    &sets.full
}

/// The conformance ceiling `Lmax` of the profile.
#[must_use]
pub fn ceiling(matrix: &Matrix<'_>) -> u8 {
    let ceiling = &matrix.ctx.params.conformance_ceiling;
    if matrix.ctx.inputs.entity.is_reduced() {
        return ceiling.reduced;
    }
    ceiling.full
}

/// The level the house expects of the entity's profile.
#[must_use]
pub fn target(matrix: &Matrix<'_>) -> u8 {
    let entity = &matrix.ctx.inputs.entity;
    let targets = &matrix.ctx.params.targets;
    let micro = entity.flags.microenterprise;
    let simplified = entity.profile == tumult_core::compliance::Profile::Simplified;
    match (simplified, micro) {
        (true, true) => targets.simplified_microenterprise,
        (true, false) => targets.simplified,
        (false, true) => targets.full_microenterprise,
        (false, false) if entity.flags.tlpt_designated => targets.full_tlpt_designated,
        (false, false) => targets.full,
    }
}

fn functions_in<'a>(matrix: &Matrix<'a>, scope: Scope) -> Vec<&'a Function> {
    let functions = &matrix.ctx.inputs.functions;
    match scope {
        Scope::Entity => functions.iter().collect(),
        Scope::Function(index) => functions.get(index).into_iter().collect(),
    }
}

/// Functions the full-profile pillars read: critical and important ones
/// for the entity, the function itself for a function scope.
fn bound_functions<'a>(matrix: &Matrix<'a>, scope: Scope) -> Vec<&'a Function> {
    let all = functions_in(matrix, scope);
    if scope != Scope::Entity {
        return all;
    }
    all.into_iter()
        .filter(|f| f.tier.is_critical_or_important())
        .collect()
}

/// `C` and the `C_k` shares over a set of weighted services.
#[must_use]
pub fn conformance_of(services: &[(&Service, f64)], lmax: u8) -> (Pillar, Option<[f64; 4]>) {
    let total: f64 = services.iter().map(|(_, c)| c).sum();
    if services.is_empty() || total <= 0.0 || lmax == 0 {
        return (Pillar::Unknown("no service in scope".into()), None);
    }
    let reached: f64 = services
        .iter()
        .map(|(s, c)| c * f64::from(s.level.min(lmax)))
        .sum();
    let mut shares = [0.0; 4];
    for (k, share) in shares.iter_mut().enumerate() {
        let level = u8::try_from(k + 1).unwrap_or(u8::MAX);
        let at: f64 = services
            .iter()
            .filter(|(s, _)| s.level >= level)
            .map(|(_, c)| c)
            .sum();
        *share = 100.0 * at / total;
    }
    let value = 100.0 * reached / (total * f64::from(lmax));
    (Pillar::Value(value), Some(shares))
}

fn conformance(matrix: &Matrix<'_>, scope: Scope) -> (Pillar, Option<[f64; 4]>) {
    let inputs = matrix.ctx.inputs;
    let params = matrix.ctx.params;
    let services: Vec<(&Service, f64)> = match scope {
        Scope::Entity => inputs
            .services
            .iter()
            .map(|s| {
                let weight = inputs
                    .service_weight(s, params)
                    .unwrap_or(params.rollup.unassigned_weight);
                (s, weight)
            })
            .collect(),
        Scope::Function(index) => {
            let function = &inputs.functions[index];
            matrix
                .ctx
                .services_of(&function.id)
                .map(|s| (s, function.tier.weight(params)))
                .collect()
        }
    };
    conformance_of(&services, ceiling(matrix))
}

/// Discipline coverage 0–1 of a set of test evidence: for each test
/// discipline the best `φ · o · s` inside the test window, weighted by
/// the discipline weights.
pub fn discipline_coverage<'e>(ctx: &Ctx<'_>, evidence: impl Iterator<Item = &'e Evidence>) -> f64 {
    let as_of_ns = ctx.inputs.as_of_ns;
    let mut best: BTreeMap<&str, f64> = BTreeMap::new();
    for e in evidence.filter(|e| e.at_ns <= as_of_ns) {
        let Some(kind) = e.kind.as_deref() else {
            continue;
        };
        let v = ctx.test_value(e, as_of_ns, ctx.params.test_window_days);
        let held = best.entry(kind).or_insert(0.0);
        *held = held.max(v);
    }
    let total: f64 = ctx.params.kind_weights.values().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let covered: f64 = ctx
        .params
        .kind_weights
        .iter()
        .map(|(kind, weight)| weight * best.get(kind.as_str()).copied().unwrap_or(0.0))
        .sum();
    covered / total
}

fn weighted_share(
    matrix: &Matrix<'_>,
    functions: &[&Function],
    value: impl Fn(&Function) -> f64,
) -> Pillar {
    let params = matrix.ctx.params;
    let total: f64 = functions.iter().map(|f| f.tier.weight(params)).sum();
    if total <= 0.0 {
        return Pillar::Excluded(Excluded::OutOfScope);
    }
    let sum: f64 = functions
        .iter()
        .map(|f| f.tier.weight(params) * value(f))
        .sum();
    Pillar::Value(100.0 * sum / total)
}

fn coverage_full(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let ctx = &matrix.ctx;
    let functions = bound_functions(matrix, scope);
    weighted_share(matrix, &functions, |f| {
        discipline_coverage(ctx, ctx.evidence_for(&f.id, ctx.inputs.as_of_ns))
    })
}

fn coverage_simplified(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let ctx = &matrix.ctx;
    let id = ctx.params.simplified_test_row.as_str();
    let Some(req) = tumult_core::compliance::requirement(id) else {
        return Pillar::Unknown(format!("row {id} is not in the registry"));
    };
    let functions = functions_in(matrix, scope);
    weighted_share(matrix, &functions, |f| {
        let cell = ctx.evaluate(req, f, ctx.inputs.as_of_ns);
        if cell.state != CellState::Scored {
            return 0.0;
        }
        cell.v
    })
}

fn coverage_micro(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let ctx = &matrix.ctx;
    let mut planned = 0.0;
    let mut executed = 0.0;
    for function in functions_in(matrix, scope) {
        let tests = function.test_plan.len() as f64;
        let Some(share) = ctx.plan_share(function, ctx.inputs.as_of_ns) else {
            continue;
        };
        planned += tests;
        executed += tests * share;
    }
    if planned <= 0.0 {
        return Pillar::Value(0.0);
    }
    Pillar::Value(100.0 * executed / planned)
}

fn coverage(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let entity = &matrix.ctx.inputs.entity;
    if entity.flags.microenterprise {
        return coverage_micro(matrix, scope);
    }
    if entity.is_reduced() {
        return coverage_simplified(matrix, scope);
    }
    coverage_full(matrix, scope)
}

fn slo_weight(slo: &Slo, params: &HouseParams) -> f64 {
    match slo.criticality {
        SloCriticality::Blocking => params.slo.blocking_weight,
        SloCriticality::Major => params.slo.major_weight,
        SloCriticality::Minor => params.slo.minor_weight,
    }
}

fn slo_credit(slo: &Slo, params: &HouseParams) -> f64 {
    if !slo.met {
        return 0.0;
    }
    if slo.remaining < params.slo.low_budget {
        return params.slo.low_budget_credit;
    }
    1.0
}

/// `S` over a set of SLOs, each with the criticality weight of what it
/// protects. `None` when the set is empty.
#[must_use]
pub fn slo_attainment(slos: &[(&Slo, f64)], params: &HouseParams) -> Option<f64> {
    let total: f64 = slos
        .iter()
        .map(|(slo, c)| c * slo_weight(slo, params))
        .sum();
    if total <= 0.0 {
        return None;
    }
    let met: f64 = slos
        .iter()
        .map(|(slo, c)| c * slo_weight(slo, params) * slo_credit(slo, params))
        .sum();
    Some(100.0 * met / total)
}

fn slo_subject_weight(matrix: &Matrix<'_>, slo: &Slo) -> f64 {
    let inputs = matrix.ctx.inputs;
    let params = matrix.ctx.params;
    let by_function = slo
        .function
        .as_deref()
        .and_then(|id| inputs.function(id))
        .map(|f| f.tier.weight(params));
    let by_service = || {
        let name = slo.service.as_deref()?;
        let service = inputs.services.iter().find(|s| s.name == name)?;
        inputs.service_weight(service, params)
    };
    by_function
        .or_else(by_service)
        .unwrap_or(params.rollup.unassigned_weight)
}

fn slo_in_scope(matrix: &Matrix<'_>, slo: &Slo, scope: Scope) -> bool {
    let Scope::Function(index) = scope else {
        return true;
    };
    let function = &matrix.ctx.inputs.functions[index];
    if slo.function.as_deref() == Some(function.id.as_str()) {
        return true;
    }
    slo.function.is_none()
        && slo
            .service
            .as_deref()
            .is_some_and(|name| matrix.ctx.services_of(&function.id).any(|s| s.name == name))
}

/// The pillar an absent SLO registry yields: unknown under the full
/// profile, not declared under a reduced one.
fn slo_absent(matrix: &Matrix<'_>, reason: &str) -> Pillar {
    if matrix.ctx.inputs.entity.is_reduced() {
        return Pillar::Excluded(Excluded::NotDeclared);
    }
    Pillar::Unknown(reason.into())
}

fn slo(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let SloFeed::Connected { slos, .. } = &matrix.ctx.inputs.slos else {
        return slo_absent(matrix, "SLO registry not connected");
    };
    let scoped: Vec<(&Slo, f64)> = slos
        .iter()
        .filter(|slo| slo_in_scope(matrix, slo, scope))
        .map(|slo| (slo, slo_subject_weight(matrix, slo)))
        .collect();
    match slo_attainment(&scoped, matrix.ctx.params) {
        Some(value) => Pillar::Value(value),
        None => slo_absent(matrix, "no SLO declared"),
    }
}

/// Whether the latest recovery measured for a function inside the
/// recovery window met its targets; `None` when none was measured.
#[must_use]
pub fn latest_recovery_met(ctx: &Ctx<'_>, function: &Function) -> Option<bool> {
    let as_of_ns = ctx.inputs.as_of_ns;
    let window = i64::from(ctx.params.recovery_window_days);
    ctx.evidence_for(&function.id, as_of_ns)
        .filter(|e| day_of(as_of_ns) - day_of(e.at_ns) <= window)
        .filter_map(|e| Some((e, recovery_met(e, function)?)))
        .max_by_key(|(e, _)| (e.at_ns, e.id.as_str()))
        .map(|(_, met)| met)
}

fn recovery(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let ctx = &matrix.ctx;
    let functions: Vec<&Function> = functions_in(matrix, scope)
        .into_iter()
        .filter(|f| f.tier.is_critical_or_important() || f.rto_s.is_some())
        .filter(|f| scope != Scope::Entity || f.tier.is_critical_or_important())
        .collect();
    weighted_share(matrix, &functions, |f| {
        if latest_recovery_met(ctx, f) == Some(true) {
            return 1.0;
        }
        0.0
    })
}

/// `L` from counts: closure of incidents and findings, blended with the
/// share of incidents whose failure mode had been tested before.
#[must_use]
pub fn loop_closure(
    incidents: usize,
    incidents_closed: usize,
    previously_tested: usize,
    findings: usize,
    findings_closed: usize,
    params: &HouseParams,
) -> Option<f64> {
    let items = incidents + findings;
    if items == 0 {
        return None;
    }
    let closure = 100.0 * (incidents_closed + findings_closed) as f64 / items as f64;
    if incidents == 0 {
        return Some(closure);
    }
    let tested = 100.0 * previously_tested as f64 / incidents as f64;
    Some(params.loop_closure_weight * closure + params.loop_previously_tested_weight * tested)
}

fn loop_pillar(matrix: &Matrix<'_>, scope: Scope) -> Pillar {
    let ctx = &matrix.ctx;
    let IncidentFeed::Connected(incidents) = &ctx.inputs.incidents else {
        return Pillar::Unknown("incident feed not connected".into());
    };
    let as_of_ns = ctx.inputs.as_of_ns;
    let window = ctx.params.loop_window_days;
    let functions = functions_in(matrix, scope);
    let scoped: Vec<_> = incidents
        .iter()
        .filter(|i| i.at_ns <= as_of_ns)
        .filter(|i| day_of(as_of_ns) - day_of(i.at_ns) <= i64::from(window))
        .filter(|i| i.functions.is_empty() || functions.iter().any(|f| i.functions.contains(&f.id)))
        .collect();
    let mut findings: BTreeMap<&str, bool> = BTreeMap::new();
    for function in &functions {
        for (finding, closed) in ctx.findings(&function.id, as_of_ns, Some(window)) {
            findings.insert(finding.id.as_str(), closed);
        }
    }
    let value = loop_closure(
        scoped.len(),
        scoped.iter().filter(|i| i.closed).count(),
        scoped.iter().filter(|i| i.previously_tested).count(),
        findings.len(),
        findings.values().filter(|closed| **closed).count(),
        ctx.params,
    );
    match value {
        Some(value) => Pillar::Value(value),
        None => Pillar::Excluded(Excluded::NoEvents),
    }
}

/// The five pillars of a scope.
#[must_use]
pub fn pillars(matrix: &Matrix<'_>, scope: Scope) -> Pillars {
    let (conformance, shares) = conformance(matrix, scope);
    Pillars {
        conformance,
        coverage: coverage(matrix, scope),
        slo: slo(matrix, scope),
        recovery: recovery(matrix, scope),
        loop_: loop_pillar(matrix, scope),
        shares,
    }
}

/// One gate level with what fails it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateCheck {
    pub level: u8,
    pub passed: bool,
    pub failing: Vec<String>,
}

/// What each conformance level adds, as named in a blocked-by reason.
pub const CONFORMANCE_LABELS: [&str; 4] = [
    "ICT spine (L1)",
    "release identity (L2)",
    "loop integration (L3)",
    "evidence-grade telemetry (L4)",
];

/// Collects the failing conditions of one gate level.
struct Gate<'m, 'a> {
    matrix: &'m Matrix<'a>,
    scope: Scope,
    pillars: &'m Pillars,
    report: &'m status::StatusReport,
    failing: Vec<String>,
}

impl Gate<'_, '_> {
    fn params(&self) -> &HouseParams {
        self.matrix.ctx.params
    }

    fn share(&mut self, k: usize, threshold: f64) {
        let Some(shares) = self.pillars.shares else {
            self.failing.push("no service in scope".into());
            return;
        };
        let share = shares[k - 1];
        if share + 1e-9 >= threshold {
            return;
        }
        self.failing.push(format!(
            "{} on {share:.0} % of services",
            CONFORMANCE_LABELS[k - 1]
        ));
    }

    fn pillar(&mut self, name: &str, pillar: &Pillar, threshold: f64, value_required: bool) {
        if pillar.reaches(threshold, value_required) {
            return;
        }
        let shown = match pillar {
            Pillar::Value(v) => format!("{v:.0}"),
            Pillar::Unknown(reason) => format!("unknown ({reason})"),
            Pillar::Excluded(why) => why.as_str().replace('_', " "),
        };
        self.failing
            .push(format!("{name} is {shown}, needs {threshold:.0}"));
    }

    fn fact(&mut self, holds: bool, failing: &str) {
        if !holds {
            self.failing.push(failing.into());
        }
    }

    fn slo_history(&mut self) {
        let needed = self.params().gates.l3_slo_history_days;
        let SloFeed::Connected { history_days, .. } = &self.matrix.ctx.inputs.slos else {
            return;
        };
        if *history_days >= needed {
            return;
        }
        self.failing.push(format!(
            "SLO history is {history_days} days, needs {needed}"
        ));
    }

    /// (row, function) pairs of the gating rows for the critical and
    /// important functions in scope.
    fn gating_pairs(&self) -> Vec<(&'static Requirement, status::CellStatus)> {
        let functions = &self.matrix.ctx.inputs.functions;
        self.report
            .rows
            .iter()
            .filter(|row| row.req.weight >= GATING_WEIGHT)
            .flat_map(|row| row.cells.iter().map(move |cell| (row.req, *cell)))
            .filter(|(_, cell)| functions[cell.function].tier.is_critical_or_important())
            .collect()
    }

    fn gating_rows(&mut self) {
        let pairs = self.gating_pairs();
        let functions = &self.matrix.ctx.inputs.functions;
        let mut total = 0.0;
        let mut operating = 0.0;
        let mut waiting = None;
        for (req, cell) in &pairs {
            let function = &functions[cell.function];
            let Some(rank) = cell.status.rank() else {
                if cell.status == Status::Unknown {
                    self.failing.push(format!(
                        "{} cannot be observed for {}",
                        req.cite, function.id
                    ));
                }
                continue;
            };
            if rank < 2 {
                self.failing.push(format!(
                    "{} has no current evidence for {}",
                    req.cite, function.id
                ));
                continue;
            }
            let weight = self.matrix.weight(cell.function);
            total += weight;
            if rank == 3 {
                operating += weight;
                continue;
            }
            waiting = waiting.or(Some((*req, *cell)));
        }
        let Some((req, cell)) = waiting else {
            return;
        };
        if total > 0.0 && operating / total + 1e-9 >= self.params().operating_share {
            return;
        }
        self.failing.push(self.waiting_reason(req, &cell));
    }

    fn waiting_reason(&self, req: &Requirement, cell: &status::CellStatus) -> String {
        let inputs = self.matrix.ctx.inputs;
        let function = &inputs.functions[cell.function];
        let same_tier = inputs
            .functions
            .iter()
            .filter(|f| f.tier == function.tier)
            .count();
        let which = if same_tier == 1 {
            format!("the {} function", function.tier.as_str())
        } else {
            format!("the {} function {}", function.tier.as_str(), function.id)
        };
        let days = self.params().operating_days;
        let reached = day_of(inputs.as_of_ns)
            + i64::from(status::days_until_operating(
                cell.streak_days,
                self.params(),
            ));
        format!(
            "{} evidence for {which} is {} days old; {days} days of unbroken evidence is reached on {}",
            gating_noun(req),
            cell.streak_days,
            format_day(reached)
        )
    }

    fn unknown_share(&mut self) {
        let limit = self.params().gates.l3_max_unknown_share;
        let unknown = self.matrix.portfolio(self.scope.function()).unknown;
        if unknown <= limit + 1e-9 {
            return;
        }
        self.failing
            .push(format!("unknown share {unknown:.2} exceeds {limit:.2}"));
    }

    /// No row at or above `min_weight` may fall short of its expected
    /// status or be unobservable.
    fn no_gap(&mut self, min_weight: u8) {
        let short: Vec<&str> = self
            .report
            .rows
            .iter()
            .filter(|row| row.req.weight >= min_weight)
            .filter(|row| matches!(row.standing, Standing::Gap | Standing::Unknown))
            .map(|row| row.req.cite)
            .collect();
        if short.is_empty() {
            return;
        }
        let shown: Vec<&str> = short.iter().take(3).copied().collect();
        let more = short.len() - shown.len();
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        self.failing
            .push(format!("evidence gap on {}{tail}", shown.join(", ")));
    }

    fn management_report(&mut self) {
        let ctx = &self.matrix.ctx;
        let as_of_ns = ctx.inputs.as_of_ns;
        let window = i64::from(ctx.params.test_window_days);
        let rows = &ctx.params.management_report_rows;
        let recorded = ctx.inputs.evidence.iter().any(|e| {
            e.at_ns <= as_of_ns
                && day_of(as_of_ns) - day_of(e.at_ns) <= window
                && e.cited.iter().any(|id| rows.contains(id))
        });
        self.fact(
            recorded,
            "no management-body report recorded in the last year",
        );
    }

    fn tlpt(&mut self) {
        let ctx = &self.matrix.ctx;
        if !ctx.inputs.entity.flags.tlpt_designated {
            return;
        }
        let held = self
            .report
            .row(&ctx.params.tlpt_row)
            .and_then(|row| row.status.rank())
            .is_some_and(|rank| rank >= 2);
        self.fact(held, "no threat-led penetration test on record");
    }

    fn registered(&mut self) {
        let inputs = self.matrix.ctx.inputs;
        let registered = match self.scope {
            Scope::Entity => inputs.functions_registered(),
            Scope::Function(index) => inputs.functions[index].registered,
        };
        self.fact(registered, "functions not registered");
    }

    fn level(&mut self, level: u8) {
        let g = self.params().gates.clone();
        let p = self.pillars.clone();
        let full = !self.matrix.ctx.inputs.entity.is_reduced();
        let facts = self.matrix.ctx.inputs.facts.clone();
        match level {
            2 => {
                self.registered();
                self.share(1, g.l2_c1);
                self.pillar("test coverage (T)", &p.coverage, g.l2_coverage, false);
            }
            3 => {
                self.share(1, g.l3_c1);
                self.share(2, g.l3_c2);
                self.pillar("test coverage (T)", &p.coverage, g.l3_coverage, false);
                if full {
                    self.pillar("SLO attainment (S)", &p.slo, g.l3_slo, false);
                    self.slo_history();
                }
                self.pillar("recovery (R)", &p.recovery, g.l3_recovery, false);
                self.gating_rows();
                self.unknown_share();
            }
            4 => {
                self.share(3, g.l4_c3);
                self.pillar("test coverage (T)", &p.coverage, g.l4_coverage, false);
                self.pillar("SLO attainment (S)", &p.slo, g.l4_slo, true);
                self.pillar("recovery (R)", &p.recovery, g.l4_recovery, false);
                self.pillar("loop closure (L)", &p.loop_, g.l4_loop, true);
                self.no_gap(CORE_WEIGHT_MIN);
                self.fact(facts.alert_rules_exported, "alert rules not exported");
                self.management_report();
            }
            _ => {
                self.share(4, g.l5_c4);
                self.pillar("test coverage (T)", &p.coverage, g.l5_coverage, false);
                self.pillar("SLO attainment (S)", &p.slo, g.l5_slo, true);
                self.pillar("recovery (R)", &p.recovery, g.l5_recovery, false);
                self.pillar("loop closure (L)", &p.loop_, g.l5_loop, true);
                self.no_gap(0);
                self.fact(
                    facts.forecasts_with_provenance,
                    "no forecast with provenance",
                );
                self.fact(
                    facts.posture_delta_two_quarters,
                    "no posture delta over two quarters",
                );
                self.tlpt();
            }
        }
    }
}

/// What the evidence behind a gating row is called in a blocked-by
/// reason.
fn gating_noun(req: &Requirement) -> &'static str {
    match req.id {
        "rts1774.art40" => "restore-test",
        "dora.art11.6.switchover" => "switchover-test",
        "dora.art12.6" => "recovery",
        "dora.art25.3" => "planned-test",
        "dora.art26.1" => "threat-led penetration test",
        _ => "test",
    }
}

/// Gate levels 2–5 of a scope, each with what fails it.
#[must_use]
pub fn gates(matrix: &Matrix<'_>, scope: Scope, pillars: &Pillars) -> Vec<GateCheck> {
    let report = status::report(matrix, scope.function());
    (2..=5)
        .map(|level| {
            let mut gate = Gate {
                matrix,
                scope,
                pillars,
                report: &report,
                failing: Vec::new(),
            };
            gate.level(level);
            GateCheck {
                level,
                passed: gate.failing.is_empty(),
                failing: gate.failing,
            }
        })
        .collect()
}

/// Highest level whose gate, and every gate below it, passes.
#[must_use]
pub fn gate_level(gates: &[GateCheck]) -> u8 {
    let mut level = 1;
    for gate in gates {
        if !gate.passed {
            break;
        }
        level = gate.level;
    }
    level
}

/// The maturity of a scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Maturity {
    /// Internal level 1–5: `min(gate level, floor(index))`.
    pub level: u8,
    pub index: f64,
    pub target: u8,
    pub gate_level: u8,
    /// The gates hold the level below what the index alone would give.
    pub capped: bool,
    /// The gate level that is blocked, and its first failing condition.
    pub blocked: Option<(u8, String)>,
    pub pillars: Pillars,
    pub weights: PillarWeights,
    pub gates: Vec<GateCheck>,
    /// Portfolio unknown share of the scope.
    pub unknown_share: f64,
    /// Profile label printed with the level.
    pub label: String,
}

impl Maturity {
    /// `level n of target m (profile)`.
    #[must_use]
    pub fn headline(&self) -> String {
        format!(
            "level {} of target {} ({})",
            self.level, self.target, self.label
        )
    }

    /// The blocked-by sentence, when a gate is blocked.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        let (level, reason) = self.blocked.as_ref()?;
        if self.level >= self.target {
            return Some(format!("next level blocked by: {reason}"));
        }
        Some(format!("level {level} blocked by: {reason}"))
    }
}

/// Floor of an index as a level 1–5.
#[must_use]
pub fn floor_level(index: f64) -> u8 {
    // 4.999999 from binary error must not read as 4.
    let floored = (index + 1e-9).floor().clamp(1.0, 5.0);
    floored as u8
}

/// Pillars, index, gates and level of a scope.
#[must_use]
pub fn assess(matrix: &Matrix<'_>, scope: Scope) -> Maturity {
    let pillars = pillars(matrix, scope);
    let w = weights(matrix).clone();
    let index = index_of(&[
        (w.conformance, &pillars.conformance),
        (w.coverage, &pillars.coverage),
        (w.slo, &pillars.slo),
        (w.recovery, &pillars.recovery),
        (w.loop_, &pillars.loop_),
    ])
    .unwrap_or(1.0);
    let gates = gates(matrix, scope, &pillars);
    let gate_level = gate_level(&gates);
    let blocked = gates
        .iter()
        .find(|gate| !gate.passed)
        .and_then(|gate| Some((gate.level, gate.failing.first()?.clone())));
    Maturity {
        level: gate_level.min(floor_level(index)),
        index,
        target: target(matrix),
        gate_level,
        capped: gate_level < floor_level(index),
        blocked,
        pillars,
        weights: w,
        gates,
        unknown_share: matrix.portfolio(scope.function()).unknown,
        label: matrix.ctx.inputs.entity.label(),
    }
}

#[cfg(test)]
mod tests {
    use tumult_core::compliance::EntityProfile;

    use super::super::model::{round2, Inputs, Tier};
    use super::super::worked::{bank_a, firm_b, ns};
    use super::*;

    fn value(pillar: &Pillar) -> f64 {
        pillar
            .value()
            .unwrap_or_else(|| panic!("{pillar:?} has no value"))
    }

    #[test]
    fn worked_example_a_is_level_4_with_index_4_42() {
        let inputs = bank_a();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);

        assert_eq!(value(&m.pillars.conformance).round(), 78.0);
        let shares = m.pillars.shares.unwrap();
        assert_eq!(shares[0].round(), 100.0);
        assert_eq!(shares[2].round(), 85.0);
        assert_eq!(shares[3].round(), 40.0);
        assert_eq!(value(&m.pillars.coverage).round(), 88.0);
        assert_eq!(value(&m.pillars.slo).round(), 93.0);
        assert_eq!(value(&m.pillars.recovery).round(), 87.0);
        assert_eq!(value(&m.pillars.loop_), 72.5);

        assert_eq!(round2(m.index), 4.42);
        assert_eq!(m.gates[0].failing, Vec::<String>::new());
        assert_eq!(m.gates[1].failing, Vec::<String>::new());
        assert_eq!(m.gates[2].failing, Vec::<String>::new());
        assert!(!m.gates[3].passed);
        assert_eq!(m.level, 4);
        assert_eq!(m.target, 3);
        assert!(!m.capped);
        assert_eq!(m.headline(), "level 4 of target 3 (full)");
        assert_eq!(
            m.reason().as_deref(),
            Some("next level blocked by: evidence-grade telemetry (L4) on 40 % of services")
        );
    }

    #[test]
    fn worked_example_a_evidence_status_counts() {
        let inputs = bank_a();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let report = status::report(&matrix, None);

        assert_eq!(report.rows.len(), 34, "applicable rows");
        let gaps: Vec<&str> = report
            .rows
            .iter()
            .filter(|r| r.standing == Standing::Gap)
            .map(|r| r.req.id)
            .collect();
        assert_eq!(gaps, ["dora.art11.6b", "dora.art28.8"]);
        let unknown: Vec<&str> = report
            .rows
            .iter()
            .filter(|r| r.standing == Standing::Unknown)
            .map(|r| r.req.id)
            .collect();
        assert_eq!(unknown, ["dora.art5.2", "dora.art24.4"]);
        assert_eq!(report.count(Standing::Meets), 30);

        let exit_plan = report.row("dora.art28.8").unwrap();
        assert_eq!(
            (exit_plan.status, exit_plan.expected),
            (Status::Designed, 2)
        );
        assert!(matrix.na.iter().any(|req| req.id == "dora.art26.1"));

        // 13 of 15 by weight Operating on the gating rows.
        let mut operating = 0.0;
        let mut total = 0.0;
        for row in report.rows.iter().filter(|r| r.req.weight == GATING_WEIGHT) {
            for cell in &row.cells {
                total += matrix.weight(cell.function);
                if cell.status == Status::Operating {
                    operating += matrix.weight(cell.function);
                }
            }
        }
        assert!((operating / total - 13.0 / 15.0).abs() < 1e-9);

        let portfolio = matrix.portfolio(None);
        assert_eq!(round2(portfolio.unknown), 0.05);
        assert_eq!(round2(portfolio.coverage.unwrap()), 0.96);
    }

    #[test]
    fn worked_example_b_is_level_2_with_index_3_60() {
        let inputs = firm_b();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);

        assert_eq!(value(&m.pillars.conformance).round(), 70.0);
        let shares = m.pillars.shares.unwrap();
        assert_eq!((shares[0].round(), shares[1].round()), (87.0, 52.0));
        assert_eq!(value(&m.pillars.coverage).round(), 69.0);
        assert_eq!(m.pillars.slo, Pillar::Excluded(Excluded::NotDeclared));
        assert_eq!(value(&m.pillars.recovery), 60.0);
        assert_eq!(value(&m.pillars.loop_), 50.0);

        assert_eq!(round2(m.index), 3.6);
        assert!(m.gates[0].passed, "{:?}", m.gates[0]);
        assert_eq!(m.gates[1].failing.len(), 1, "{:?}", m.gates[1]);
        assert_eq!(m.level, 2);
        assert_eq!(m.target, 3);
        assert!(m.capped, "the index is shown greyed");
        assert_eq!(m.headline(), "level 2 of target 3 (simplified)");
        assert_eq!(
            m.reason().as_deref(),
            Some(
                "level 3 blocked by: restore-test evidence for the critical function is 150 \
                 days old; 180 days of unbroken evidence is reached on 2026-12-13"
            )
        );
    }

    #[test]
    fn worked_example_b_rows_not_applicable_and_without_events() {
        let inputs = firm_b();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        for id in ["dora.art26.1", "dora.art28.2", "dora.art12.6"] {
            assert!(matrix.na.iter().any(|req| req.id == id), "{id} is n/a");
        }
        let report = status::report(&matrix, None);
        for id in ["dora.art19.4.a", "dora.art19.4.b", "dora.art19.4.c"] {
            assert_eq!(report.row(id).unwrap().status, Status::NoEvents);
        }
        let restore = report.row("rts1774.art40").unwrap();
        let statuses: Vec<Status> = restore.cells.iter().map(|c| c.status).collect();
        assert_eq!(
            statuses,
            [Status::Existing, Status::Operating, Status::None],
            "critical 150 days, important 276 days, intranet untested"
        );
    }

    #[test]
    fn a_microenterprise_targets_level_2_and_measures_its_own_plan() {
        let mut inputs = firm_b();
        inputs.entity =
            EntityProfile::parse("simplified", &["microenterprise".to_string()]).unwrap();
        for function in &mut inputs.functions {
            function.test_plan = vec![
                super::super::model::PlannedTest {
                    kind: "dr".into(),
                    interval_days: 365,
                },
                super::super::model::PlannedTest {
                    kind: "security".into(),
                    interval_days: 365,
                },
            ];
        }
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);
        assert_eq!(m.target, 2);
        assert_eq!(m.label, "simplified, microenterprise");
        // dr executed for two of three functions, security for none: 2 of 6.
        assert!((value(&m.pillars.coverage) - 100.0 / 3.0).abs() < 1e-9);
        assert!(matrix.na.iter().any(|req| req.id == "dora.art24.6"));
    }

    #[test]
    fn targets_follow_profile_and_designation() {
        let params = HouseParams::default();
        let target_of = |profile: &str, flags: &[&str]| {
            let flags: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
            let inputs = Inputs::empty(
                ns("2026-11-13"),
                EntityProfile::parse(profile, &flags).unwrap(),
            );
            target(&Matrix::build(&inputs, &params))
        };
        assert_eq!(target_of("full", &[]), 3);
        assert_eq!(target_of("full", &["tlpt_designated"]), 4);
        assert_eq!(target_of("full", &["microenterprise"]), 2);
        assert_eq!(target_of("simplified", &[]), 3);
        assert_eq!(target_of("simplified", &["microenterprise"]), 2);
    }

    #[test]
    fn unregistered_functions_hold_the_level_at_1() {
        let mut inputs = bank_a();
        for function in &mut inputs.functions {
            function.registered = false;
        }
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);
        assert_eq!(m.level, 1);
        assert!(m.capped);
        assert_eq!(
            m.reason().as_deref(),
            Some("level 2 blocked by: functions not registered")
        );
    }

    #[test]
    fn nothing_connected_is_level_1_with_unknown_pillars() {
        let inputs = Inputs::empty(ns("2026-11-13"), EntityProfile::full());
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);
        assert_eq!(m.level, 1);
        assert_eq!(m.pillars.conformance.state(), "unknown");
        assert_eq!(m.pillars.slo.state(), "unknown");
        assert_eq!(m.pillars.loop_.state(), "unknown");
        assert_eq!(m.pillars.coverage, Pillar::Excluded(Excluded::OutOfScope));
        assert_eq!(m.index, 1.0);
    }

    #[test]
    fn an_unknown_pillar_counts_as_zero_and_an_excluded_one_leaves_the_sum() {
        let known = Pillar::Value(80.0);
        let unknown = Pillar::Unknown("not connected".into());
        let excluded = Pillar::Excluded(Excluded::NotDeclared);
        assert_eq!(index_of(&[(0.5, &known), (0.5, &unknown)]), Some(2.6));
        assert_eq!(index_of(&[(0.5, &known), (0.5, &excluded)]), Some(4.2));
        assert_eq!(index_of(&[(0.5, &excluded)]), None);
    }

    #[test]
    fn a_function_scope_reads_only_that_function() {
        let inputs = firm_b();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let critical = assess(&matrix, Scope::Function(0));
        assert_eq!(value(&critical.pillars.coverage), 100.0);
        assert_eq!(value(&critical.pillars.recovery), 100.0);
        assert_eq!(critical.pillars.shares.unwrap()[1], 100.0);
        let intranet = assess(&matrix, Scope::Function(2));
        assert_eq!(inputs.functions[2].tier, Tier::Other);
        assert_eq!(value(&intranet.pillars.coverage), 0.0);
        assert_eq!(
            intranet.pillars.recovery,
            Pillar::Excluded(Excluded::OutOfScope)
        );
    }

    #[test]
    fn a_tlpt_designated_entity_needs_the_test_on_record_for_level_5() {
        let mut inputs = bank_a();
        inputs.entity = EntityProfile::parse("full", &["tlpt_designated".to_string()]).unwrap();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let m = assess(&matrix, Scope::Entity);
        assert_eq!(m.target, 4);
        assert!(m.gates[3]
            .failing
            .iter()
            .any(|f| f.contains("threat-led penetration test")));
    }

    #[test]
    fn loop_closure_blends_closure_with_prior_testing_only_when_incidents_exist() {
        let p = HouseParams::default();
        assert_eq!(loop_closure(4, 3, 2, 6, 5, &p), Some(72.5));
        assert_eq!(loop_closure(0, 0, 0, 2, 1, &p), Some(50.0));
        assert_eq!(loop_closure(0, 0, 0, 0, 0, &p), None);
    }

    #[test]
    fn floor_level_is_clamped_to_the_scale() {
        assert_eq!(floor_level(0.2), 1);
        assert_eq!(floor_level(3.999_999_999_9), 4);
        assert_eq!(floor_level(4.42), 4);
        assert_eq!(floor_level(7.0), 5);
    }
}
