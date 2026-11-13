//! DORA coverage: `v(function, requirement)` and the article ratios.
//!
//! For a function `f` and a registry row `r`, `v(f, r)` is the best
//! `φ · o · s` over the evidence the row's selector accepts: freshness
//! `φ` (1 inside the row's window, then halving), outcome `o` and mapping
//! strength `s`. A cell is one of three things: scored (a number, possibly
//! 0), `unknown` (the evidence class is not connected, so nothing can be
//! said) or `no events` (the feed is connected and nothing happened).
//! Rows the profile excludes are `na` and never reach this module.
//!
//! ```text
//! cov(A)     = Σ_{r known} w_r · Σ_f c_f · v(f, r)  /  Σ_{r known} w_r · Σ_f c_f
//! unknown(A) = Σ_{r unknown} w_r  /  Σ_{r known or unknown} w_r
//! ```
//!
//! `no events` and `na` rows are left out of both. No coverage number is
//! reported without its unknown share.

use std::collections::BTreeMap;

use tumult_core::compliance::{
    applicable, EvidenceStrength, Feed, Requirement, Selector, ARTICLES, REQUIREMENTS,
};

use super::model::{day_of, Evidence, Function, IncidentFeed, Inputs, SloFeed, Source};
use super::params::HouseParams;
use super::status;

/// Freshness `φ`: 1 inside the window `F`, then `2^(−(age − F) / (F · d))`
/// with `d` the decay fraction.
#[must_use]
pub fn freshness(age_days: i64, window_days: u32, params: &HouseParams) -> f64 {
    let window = f64::from(window_days);
    let age = age_days.max(0) as f64;
    if age <= window {
        return 1.0;
    }
    0.5_f64.powf((age - window) / (window * params.freshness_decay_fraction))
}

/// Strength factor `s` of a row's mapping.
#[must_use]
pub fn strength_factor(strength: EvidenceStrength, params: &HouseParams) -> f64 {
    match strength {
        EvidenceStrength::Direct => params.strength.direct,
        EvidenceStrength::Supporting => params.strength.supporting,
        EvidenceStrength::Indirect => params.strength.indirect,
    }
}

/// One piece of evidence scored against one row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored<'a> {
    pub evidence: &'a Evidence,
    pub age_days: i64,
    pub phi: f64,
    pub o: f64,
    pub s: f64,
    pub v: f64,
}

/// What can be said about one (function, requirement) cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    /// The evidence class is not connected.
    Unknown,
    /// The feed is connected and nothing happened in the window.
    NoEvents,
    /// Observable: `v` is a number, possibly 0.
    Scored,
}

/// `v(f, r)` with its explanation.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell<'a> {
    pub state: CellState,
    pub v: f64,
    /// The evidence that set `v`, when one piece did.
    pub best: Option<Scored<'a>>,
    /// Why the cell is unknown, or what holds it at 0.
    pub reason: Option<String>,
}

impl<'a> Cell<'a> {
    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            state: CellState::Unknown,
            v: 0.0,
            best: None,
            reason: Some(reason.into()),
        }
    }

    fn no_events() -> Self {
        Self {
            state: CellState::NoEvents,
            v: 0.0,
            best: None,
            reason: None,
        }
    }

    fn value(v: f64) -> Self {
        Self {
            state: CellState::Scored,
            v,
            best: None,
            reason: None,
        }
    }

    fn zero(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::value(0.0)
        }
    }

    fn best_of(rows: Vec<Scored<'a>>) -> Self {
        let best = rows
            .into_iter()
            .fold(None, |best: Option<Scored<'a>>, row| match best {
                Some(b) if !better(&row, &b) => Some(b),
                _ => Some(row),
            });
        Self {
            v: best.map_or(0.0, |b| b.v),
            best,
            ..Self::value(0.0)
        }
    }
}

/// Total order on scored evidence: higher `v`, then newer, then id, so
/// the chosen row never depends on input order.
fn better(a: &Scored<'_>, b: &Scored<'_>) -> bool {
    let key = |r: &Scored<'_>| (r.v, r.evidence.at_ns);
    match key(a).partial_cmp(&key(b)) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Less) => false,
        _ => a.evidence.id > b.evidence.id,
    }
}

/// Scoring context: the inputs, the parameters and the evidence indexes.
#[derive(Debug, Clone)]
pub struct Ctx<'a> {
    pub inputs: &'a Inputs,
    pub params: &'a HouseParams,
    by_function: BTreeMap<&'a str, Vec<&'a Evidence>>,
    entity_wide: Vec<&'a Evidence>,
    first_cited: BTreeMap<&'a str, i64>,
}

impl<'a> Ctx<'a> {
    /// Index the evidence of `inputs` by function and by cited row.
    #[must_use]
    pub fn new(inputs: &'a Inputs, params: &'a HouseParams) -> Self {
        let mut by_function: BTreeMap<&str, Vec<&Evidence>> = BTreeMap::new();
        let mut entity_wide = Vec::new();
        let mut first_cited: BTreeMap<&str, i64> = BTreeMap::new();
        for evidence in &inputs.evidence {
            for function in &evidence.functions {
                by_function.entry(function).or_default().push(evidence);
            }
            if evidence.functions.is_empty() && evidence.source == Source::Manual {
                entity_wide.push(evidence);
            }
            for id in &evidence.cited {
                let first = first_cited.entry(id).or_insert(evidence.at_ns);
                *first = (*first).min(evidence.at_ns);
            }
        }
        Self {
            inputs,
            params,
            by_function,
            entity_wide,
            first_cited,
        }
    }

    /// Evidence naming `function`, no later than `as_of_ns`.
    pub fn evidence_for(
        &self,
        function: &str,
        as_of_ns: i64,
    ) -> impl Iterator<Item = &'a Evidence> + '_ {
        self.by_function
            .get(function)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |e| e.at_ns <= as_of_ns)
    }

    fn score(
        &self,
        evidence: &'a Evidence,
        as_of_ns: i64,
        window: u32,
        o: f64,
        s: f64,
    ) -> Scored<'a> {
        let age_days = day_of(as_of_ns) - day_of(evidence.at_ns);
        let phi = freshness(age_days, window, self.params);
        Scored {
            evidence,
            age_days,
            phi,
            o,
            s,
            v: phi * o * s,
        }
    }

    /// `φ · o · s` of a piece of test evidence against a window, outside
    /// any registry row: `s` is direct for a run and the manual-test cap
    /// for a manual record standing in for one.
    #[must_use]
    pub fn test_value(&self, evidence: &Evidence, as_of_ns: i64, window: u32) -> f64 {
        let age_days = day_of(as_of_ns) - day_of(evidence.at_ns);
        let s = match evidence.source {
            Source::Run => self.params.strength.direct,
            Source::Manual => self.params.strength.manual_test,
        };
        freshness(age_days, window, self.params) * evidence.outcome.factor(self.params) * s
    }

    /// Strength of a piece of test evidence for a row: the row's own
    /// strength, capped for a manual record standing in for a run.
    fn test_strength(&self, req: &Requirement, evidence: &Evidence) -> f64 {
        let s = strength_factor(req.strength, self.params);
        if evidence.source == Source::Manual {
            return s.min(self.params.strength.manual_test);
        }
        s
    }

    /// Test evidence of the given disciplines for a function (an empty
    /// list accepts every discipline), scored against a window.
    pub fn test_rows(
        &self,
        req: &Requirement,
        function: &str,
        kinds: &[&str],
        as_of_ns: i64,
        window: u32,
    ) -> Vec<Scored<'a>> {
        self.evidence_for(function, as_of_ns)
            .filter(|e| {
                e.kind
                    .as_deref()
                    .is_some_and(|k| kinds.is_empty() || kinds.contains(&k))
            })
            .map(|e| {
                let o = e.outcome.factor(self.params);
                self.score(e, as_of_ns, window, o, self.test_strength(req, e))
            })
            .collect()
    }

    fn recovery_rows(
        &self,
        req: &Requirement,
        function: &Function,
        as_of_ns: i64,
        point: bool,
    ) -> Vec<Scored<'a>> {
        self.evidence_for(&function.id, as_of_ns)
            .filter_map(|e| {
                let met = if point {
                    recovery_point_met(e, function)?
                } else {
                    recovery_met(e, function)?
                };
                let o = if met {
                    self.params.outcome.passed
                } else {
                    self.params.outcome.failed
                };
                Some(self.score(e, as_of_ns, req.window_days, o, self.test_strength(req, e)))
            })
            .collect()
    }

    fn exercise_rows(
        &self,
        req: &Requirement,
        function: &str,
        types: &[&str],
        cited: bool,
        as_of_ns: i64,
    ) -> Vec<Scored<'a>> {
        self.evidence_for(function, as_of_ns)
            .filter(|e| e.source == Source::Manual)
            .filter(|e| {
                e.exercise_type
                    .as_deref()
                    .is_some_and(|t| types.contains(&t))
            })
            .filter(|e| !cited || e.cited.iter().any(|id| id == req.id))
            .map(|e| self.manual_row(req, e, as_of_ns))
            .collect()
    }

    fn attestation_rows(
        &self,
        req: &Requirement,
        function: &str,
        as_of_ns: i64,
    ) -> Vec<Scored<'a>> {
        let wide = self
            .entity_wide
            .iter()
            .copied()
            .filter(|e| e.at_ns <= as_of_ns);
        self.evidence_for(function, as_of_ns)
            .chain(wide)
            .filter(|e| e.source == Source::Manual && e.cited.iter().any(|id| id == req.id))
            .map(|e| self.manual_row(req, e, as_of_ns))
            .collect()
    }

    fn manual_row(&self, req: &Requirement, evidence: &'a Evidence, as_of_ns: i64) -> Scored<'a> {
        let o = evidence.outcome.factor(self.params);
        let s = strength_factor(req.strength, self.params);
        // No interval set for this entity: a record of any age is fresh.
        let window = req.window_for(&self.inputs.entity).unwrap_or(u32::MAX);
        self.score(evidence, as_of_ns, window, o, s)
    }

    /// Every piece of evidence the row's selector accepts for a function,
    /// each with its `φ`, `o`, `s` and `v`. Rows whose value is derived
    /// (a share, a declaration, a feed) have no evidence rows.
    #[must_use]
    pub fn evidence_rows(
        &self,
        req: &Requirement,
        function: &Function,
        as_of_ns: i64,
    ) -> Vec<Scored<'a>> {
        let id = function.id.as_str();
        let mut rows = match req.selector {
            Selector::Run { kinds } => self.test_rows(req, id, kinds, as_of_ns, req.window_days),
            Selector::Disciplines { .. } | Selector::Remediation | Selector::PlanExecuted => {
                self.test_rows(req, id, &[], as_of_ns, req.window_days)
            }
            Selector::RecoveryMet => self.recovery_rows(req, function, as_of_ns, false),
            Selector::RecoveryPoint => self.recovery_rows(req, function, as_of_ns, true),
            Selector::Exercise { types, cited } => {
                self.exercise_rows(req, id, types, cited, as_of_ns)
            }
            Selector::Attestation | Selector::FunctionAssessed => {
                self.attestation_rows(req, id, as_of_ns)
            }
            Selector::Programme => self.programme_rows(req, function, as_of_ns),
            Selector::Telemetry | Selector::Topology | Selector::Feed(_) => Vec::new(),
        };
        rows.sort_by(|a, b| {
            if better(a, b) {
                return std::cmp::Ordering::Less;
            }
            std::cmp::Ordering::Greater
        });
        rows
    }

    /// `v(function, requirement)` as of `as_of_ns`. Evidence dated after
    /// `as_of_ns` is ignored, so the same call rebuilds any past day.
    #[must_use]
    pub fn evaluate(&self, req: &Requirement, function: &Function, as_of_ns: i64) -> Cell<'a> {
        let id = function.id.as_str();
        match req.selector {
            Selector::Run { kinds } => {
                Cell::best_of(self.test_rows(req, id, kinds, as_of_ns, req.window_days))
            }
            Selector::Disciplines { min } => self.disciplines(req, id, min, as_of_ns),
            Selector::Remediation => self.remediation(req, function, as_of_ns),
            Selector::RecoveryMet => {
                Cell::best_of(self.recovery_rows(req, function, as_of_ns, false))
            }
            Selector::RecoveryPoint => {
                Cell::best_of(self.recovery_rows(req, function, as_of_ns, true))
            }
            Selector::Exercise { types, cited } => {
                Cell::best_of(self.exercise_rows(req, id, types, cited, as_of_ns))
            }
            Selector::Attestation => self.attestation(req, id, as_of_ns),
            Selector::FunctionAssessed => self.assessed(req, function, as_of_ns),
            Selector::Programme => self.programme(req, function, as_of_ns),
            Selector::PlanExecuted => self.plan_executed(req, function, as_of_ns),
            Selector::Telemetry => self.telemetry(req, id, as_of_ns),
            Selector::Topology => self.topology(id, as_of_ns),
            Selector::Feed(Feed::Incident) => self.incident_feed(req, id, as_of_ns),
            Selector::Feed(Feed::Slo) => self.slo_feed(id),
            Selector::Feed(Feed::Register) => self.register_feed(req, as_of_ns),
        }
    }

    /// Mean of the best `v` in each of the `min` best-covered disciplines.
    fn disciplines(
        &self,
        req: &Requirement,
        function: &str,
        min: usize,
        as_of_ns: i64,
    ) -> Cell<'a> {
        let mut per_kind: BTreeMap<&str, Scored<'a>> = BTreeMap::new();
        for row in self.test_rows(req, function, &[], as_of_ns, req.window_days) {
            let Some(kind) = row.evidence.kind.as_deref() else {
                continue;
            };
            match per_kind.get(kind) {
                Some(held) if !better(&row, held) => {}
                _ => {
                    per_kind.insert(kind, row);
                }
            }
        }
        let mut best: Vec<Scored<'a>> = per_kind.into_values().collect();
        best.sort_by(|a, b| {
            if better(a, b) {
                return std::cmp::Ordering::Less;
            }
            std::cmp::Ordering::Greater
        });
        let needed = min.max(1);
        let total: f64 = best.iter().take(needed).map(|row| row.v).sum();
        let reason = (best.len() < needed)
            .then(|| format!("{} of {needed} test disciplines have evidence", best.len()));
        Cell {
            state: CellState::Scored,
            v: total / needed as f64,
            best: best.first().copied(),
            reason,
        }
    }

    /// Findings of a function: failed tests dated within `window` days of
    /// `as_of_ns` (`None` = any age), each with whether a later passing
    /// run of the same experiment closed it.
    #[must_use]
    pub fn findings(
        &self,
        function: &str,
        as_of_ns: i64,
        window: Option<u32>,
    ) -> Vec<(&'a Evidence, bool)> {
        let tests: Vec<&Evidence> = self
            .evidence_for(function, as_of_ns)
            .filter(|e| e.kind.is_some())
            .collect();
        let in_window = |e: &Evidence| {
            window.is_none_or(|w| day_of(as_of_ns) - day_of(e.at_ns) <= i64::from(w))
        };
        tests
            .iter()
            .filter(|e| e.outcome == super::model::Outcome::Failed && in_window(e))
            .map(|finding| {
                let closed = tests.iter().any(|later| {
                    later.outcome == super::model::Outcome::Passed
                        && later.experiment == finding.experiment
                        && later.at_ns > finding.at_ns
                });
                (*finding, closed)
            })
            .collect()
    }

    /// 0 while any finding is open or nothing was ever tested; otherwise
    /// the freshness of the latest test.
    fn remediation(&self, req: &Requirement, function: &Function, as_of_ns: i64) -> Cell<'a> {
        let id = function.id.as_str();
        let open = self
            .findings(id, as_of_ns, None)
            .into_iter()
            .find(|(_, closed)| !closed);
        if let Some((finding, _)) = open {
            return Cell::zero(format!(
                "open finding: {} failed on {} with no passing run since",
                finding.experiment,
                super::model::format_day(day_of(finding.at_ns))
            ));
        }
        let latest = self
            .evidence_for(id, as_of_ns)
            .filter(|e| e.kind.is_some())
            .max_by_key(|e| (e.at_ns, e.id.as_str()));
        let Some(latest) = latest else {
            return Cell::zero("no test on record");
        };
        let row = self.score(latest, as_of_ns, req.window_days, 1.0, 1.0);
        Cell {
            v: row.v,
            best: Some(row),
            ..Cell::value(0.0)
        }
    }

    fn attestation(&self, req: &Requirement, function: &str, as_of_ns: i64) -> Cell<'a> {
        let on_file = self
            .first_cited
            .get(req.id)
            .is_some_and(|first| *first <= as_of_ns);
        if !on_file {
            return Cell::unknown(format!("no record citing {} on file", req.id));
        }
        Cell::best_of(self.attestation_rows(req, function, as_of_ns))
    }

    /// The criticality assessment of a function, dated by the stored
    /// record that cites the row. The date in the org file is a
    /// declaration: it makes the row Designed and covers nothing.
    fn assessed(&self, req: &Requirement, function: &Function, as_of_ns: i64) -> Cell<'a> {
        if !function.registered {
            return Cell::zero("function not registered");
        }
        let rows = self.attestation_rows(req, &function.id, as_of_ns);
        if !rows.is_empty() {
            return Cell::best_of(rows);
        }
        if function.last_assessed_ns.is_some() {
            return Cell::zero(format!(
                "assessment date declared in the org file only: no record citing {} is stored",
                req.id
            ));
        }
        Cell::zero("no criticality assessment declared or on record")
    }

    /// What the lake holds of a declared testing programme: every stored
    /// test of the function and every record citing the row. A stored
    /// test shows the programme exists whatever its age or outcome (the
    /// rows on execution and findings score those), so it counts in full.
    fn programme_rows(
        &self,
        req: &Requirement,
        function: &Function,
        as_of_ns: i64,
    ) -> Vec<Scored<'a>> {
        let mut rows = self.attestation_rows(req, &function.id, as_of_ns);
        if function.test_plan.is_empty() && !function.scheduled {
            return rows;
        }
        let tests = self
            .evidence_for(&function.id, as_of_ns)
            .filter(|e| e.kind.is_some())
            .map(|e| self.score(e, as_of_ns, u32::MAX, 1.0, 1.0));
        rows.extend(tests);
        rows
    }

    /// A plan or a schedule is a declaration: the row is Designed and
    /// covers nothing until a run or a record of it is stored.
    fn programme(&self, req: &Requirement, function: &Function, as_of_ns: i64) -> Cell<'a> {
        let rows = self.programme_rows(req, function, as_of_ns);
        if !rows.is_empty() {
            return Cell::best_of(rows);
        }
        if function.test_plan.is_empty() && !function.scheduled {
            return Cell::zero("no test plan or schedule declared");
        }
        Cell::zero("test plan or schedule declared only: no run or record of it is stored")
    }

    fn plan_executed(&self, _req: &Requirement, function: &Function, as_of_ns: i64) -> Cell<'a> {
        let Some(share) = self.plan_share(function, as_of_ns) else {
            return Cell::zero("no test plan declared");
        };
        Cell::value(share)
    }

    /// Share of a function's planned tests executed within their planned
    /// interval; `None` when it declares no plan.
    #[must_use]
    pub fn plan_share(&self, function: &Function, as_of_ns: i64) -> Option<f64> {
        if function.test_plan.is_empty() {
            return None;
        }
        let executed = function
            .test_plan
            .iter()
            .filter(|planned| {
                self.evidence_for(&function.id, as_of_ns).any(|e| {
                    e.kind.as_deref() == Some(planned.kind.as_str())
                        && day_of(as_of_ns) - day_of(e.at_ns) <= i64::from(planned.interval_days)
                })
            })
            .count();
        Some(executed as f64 / function.test_plan.len() as f64)
    }

    fn telemetry(&self, req: &Requirement, function: &str, as_of_ns: i64) -> Cell<'a> {
        let services: Vec<_> = self.services_of(function).collect();
        if services.is_empty() {
            return Cell::zero("no service maps to the function");
        }
        let total: f64 = services
            .iter()
            .map(|service| {
                let seen = service.first_seen_ns.is_some_and(|first| first <= as_of_ns);
                let Some(last) = service.last_seen_ns.filter(|_| seen) else {
                    return 0.0;
                };
                let age = day_of(as_of_ns) - day_of(last.min(as_of_ns));
                freshness(age, req.window_days, self.params)
            })
            .sum();
        Cell::value(total / services.len() as f64)
    }

    fn topology(&self, function: &str, as_of_ns: i64) -> Cell<'a> {
        if !self.inputs.facts.topology_observed {
            return Cell::unknown("no dependency topology observed");
        }
        let services: Vec<_> = self.services_of(function).collect();
        if services.is_empty() {
            return Cell::zero("no service maps to the function");
        }
        let seen = services
            .iter()
            .filter(|s| s.first_seen_ns.is_some_and(|first| first <= as_of_ns))
            .count();
        Cell::value(seen as f64 / services.len() as f64)
    }

    fn incident_feed(&self, req: &Requirement, function: &str, as_of_ns: i64) -> Cell<'a> {
        let IncidentFeed::Connected(incidents) = &self.inputs.incidents else {
            return Cell::unknown("incident feed not connected");
        };
        let relevant: Vec<_> = incidents
            .iter()
            .filter(|i| i.functions.is_empty() || i.functions.iter().any(|f| f == function))
            .filter(|i| i.at_ns <= as_of_ns)
            .filter(|i| day_of(as_of_ns) - day_of(i.at_ns) <= i64::from(req.window_days))
            .collect();
        if relevant.is_empty() {
            return Cell::no_events();
        }
        let satisfied = relevant
            .iter()
            .filter(|i| i.satisfied.iter().any(|id| id == req.id))
            .count();
        Cell::value(satisfied as f64 / relevant.len() as f64)
    }

    fn slo_feed(&self, function: &str) -> Cell<'a> {
        let SloFeed::Connected { slos, .. } = &self.inputs.slos else {
            return Cell::unknown("SLO registry not connected");
        };
        if slos
            .iter()
            .any(|slo| slo.function.as_deref() == Some(function))
        {
            return Cell::value(1.0);
        }
        Cell::zero("no SLO declared for the function")
    }

    fn register_feed(&self, req: &Requirement, as_of_ns: i64) -> Cell<'a> {
        let facts = &self.inputs.facts;
        if !facts.register_connected {
            return Cell::unknown("register of information not connected");
        }
        let Some(at) = facts.register_reconciled_ns.filter(|at| *at <= as_of_ns) else {
            return Cell::zero("register never reconciled");
        };
        let age = day_of(as_of_ns) - day_of(at);
        Cell::value(freshness(age, req.window_days, self.params))
    }

    /// Services that support a function.
    pub fn services_of<'s>(
        &'s self,
        function: &'s str,
    ) -> impl Iterator<Item = &'a super::model::Service> + 's {
        self.inputs
            .services
            .iter()
            .filter(move |s| s.functions.iter().any(|f| f == function))
    }

    /// Whether the owner has declared a target, plan, schedule or policy
    /// for the row: what separates "Designed" from "None".
    #[must_use]
    pub fn declared(&self, req: &Requirement, function: &Function) -> bool {
        if function.declared.iter().any(|id| id == req.id) {
            return true;
        }
        let planned = !function.test_plan.is_empty() || function.scheduled;
        match req.selector {
            Selector::RecoveryMet => function.rto_s.is_some(),
            Selector::RecoveryPoint => function.rpo_s.is_some(),
            Selector::Run { kinds } => {
                function.scheduled
                    || function
                        .test_plan
                        .iter()
                        .any(|p| kinds.is_empty() || kinds.contains(&p.kind.as_str()))
            }
            Selector::Disciplines { .. }
            | Selector::Remediation
            | Selector::PlanExecuted
            | Selector::Programme => planned,
            Selector::FunctionAssessed => function.registered,
            Selector::Exercise { .. }
            | Selector::Attestation
            | Selector::Telemetry
            | Selector::Topology
            | Selector::Feed(_) => false,
        }
    }
}

fn within(measured: Option<f64>, target: Option<f64>) -> Option<bool> {
    Some(measured? <= target?)
}

/// Whether a measured recovery met its targets: recovery time within the
/// target and, when a recovery point was measured against a target, that
/// too. `None` when no recovery time was measured against a target.
#[must_use]
pub fn recovery_met(evidence: &Evidence, function: &Function) -> Option<bool> {
    let time = within(
        evidence.rto_measured_s,
        evidence.rto_target_s.or(function.rto_s),
    )?;
    Some(time && recovery_point_met(evidence, function).unwrap_or(true))
}

/// Whether a measured recovery point met its target; `None` when none was
/// measured against a target.
#[must_use]
pub fn recovery_point_met(evidence: &Evidence, function: &Function) -> Option<bool> {
    within(
        evidence.rpo_measured_s,
        evidence.rpo_target_s.or(function.rpo_s),
    )
}

/// One (function, requirement) cell of the matrix, with how long its
/// value has held.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixCell<'a> {
    /// Index into `Inputs::functions`.
    pub function: usize,
    pub cell: Cell<'a>,
    pub declared: bool,
    /// Unbroken days, up to today, with `v` at or above the Existing
    /// threshold (capped at the Operating window).
    pub streak_days: u32,
}

/// One applicable registry row with a cell per function it binds.
#[derive(Debug, Clone)]
pub struct MatrixRow<'a> {
    pub req: &'static Requirement,
    pub cells: Vec<MatrixCell<'a>>,
}

/// State of a row over a scope of functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Known,
    Unknown,
    NoEvents,
    /// Binds no function in scope.
    Na,
}

impl RowState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Unknown => "unknown",
            Self::NoEvents => "no_events",
            Self::Na => "na",
        }
    }
}

impl<'a> MatrixRow<'a> {
    /// Cells in scope: one function, or all of them.
    pub fn scoped(&self, scope: Option<usize>) -> impl Iterator<Item = &MatrixCell<'a>> + '_ {
        self.cells
            .iter()
            .filter(move |c| scope.is_none_or(|f| c.function == f))
    }

    /// State of the row over a scope.
    #[must_use]
    pub fn state(&self, scope: Option<usize>) -> RowState {
        state_of(self.scoped(scope))
    }

    /// Cells of a set of functions (an org unit).
    pub fn among<'s>(
        &'s self,
        functions: &'s [usize],
    ) -> impl Iterator<Item = &'s MatrixCell<'a>> + 's {
        self.cells
            .iter()
            .filter(move |c| functions.contains(&c.function))
    }

    /// Why the row is unknown, from its first unknown cell.
    #[must_use]
    pub fn reason(&self, scope: Option<usize>) -> Option<&str> {
        self.scoped(scope)
            .find(|c| c.cell.state == CellState::Unknown)
            .and_then(|c| c.cell.reason.as_deref())
    }
}

/// State of a row over some of its cells: known as soon as one is scored,
/// else unknown when one is, else without events; `na` without a cell.
fn state_of<'c, 'a: 'c>(cells: impl Iterator<Item = &'c MatrixCell<'a>>) -> RowState {
    let mut any = false;
    let mut unknown = false;
    for cell in cells {
        any = true;
        match cell.cell.state {
            CellState::Scored => return RowState::Known,
            CellState::Unknown => unknown = true,
            CellState::NoEvents => {}
        }
    }
    if !any {
        return RowState::Na;
    }
    if unknown {
        return RowState::Unknown;
    }
    RowState::NoEvents
}

/// The state a set of row states reads as: the most informative one.
#[must_use]
pub fn strongest(states: impl Iterator<Item = RowState>) -> RowState {
    let mut best = RowState::Na;
    for state in states {
        best = match (best, state) {
            (_, RowState::Known) | (RowState::Known, _) => return RowState::Known,
            (_, RowState::Unknown) | (RowState::Unknown, _) => RowState::Unknown,
            (_, RowState::NoEvents) | (RowState::NoEvents, _) => RowState::NoEvents,
            (RowState::Na, RowState::Na) => RowState::Na,
        };
    }
    best
}

/// The scored matrix of one legal entity: every applicable row against
/// every function it binds.
#[derive(Debug, Clone)]
pub struct Matrix<'a> {
    pub ctx: Ctx<'a>,
    pub rows: Vec<MatrixRow<'a>>,
    /// Registry rows the profile excludes.
    pub na: Vec<&'static Requirement>,
}

impl<'a> Matrix<'a> {
    /// Score every applicable row for every function it binds, as of the
    /// inputs' `as_of_ns`.
    #[must_use]
    pub fn build(inputs: &'a Inputs, params: &'a HouseParams) -> Self {
        let ctx = Ctx::new(inputs, params);
        let rows = applicable(&inputs.entity)
            .map(|req| MatrixRow {
                req,
                cells: inputs
                    .functions
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| req.binds(f.tier.is_critical_or_important()))
                    .map(|(index, f)| matrix_cell(&ctx, req, index, f))
                    .collect(),
            })
            .collect();
        let na = REQUIREMENTS
            .iter()
            .filter(|req| !req.applies(&inputs.entity))
            .collect();
        Self { ctx, rows, na }
    }

    /// The row with this requirement id.
    #[must_use]
    pub fn row(&self, id: &str) -> Option<&MatrixRow<'a>> {
        self.rows.iter().find(|row| row.req.id == id)
    }

    /// Criticality weight of a function by index.
    #[must_use]
    pub fn weight(&self, function: usize) -> f64 {
        self.ctx.inputs.functions[function]
            .tier
            .weight(self.ctx.params)
    }

    fn ratio_rows(&self, article: Option<&str>, scope: Option<usize>) -> Vec<RatioRow> {
        self.rows
            .iter()
            .filter(|row| article.is_none_or(|a| row.req.article == a))
            .map(|row| RatioRow {
                weight: f64::from(row.req.weight),
                state: row.state(scope),
                cells: row
                    .scoped(scope)
                    .filter(|c| c.cell.state == CellState::Scored)
                    .map(|c| (self.weight(c.function), c.cell.v))
                    .collect(),
            })
            .collect()
    }

    /// `cov(A)` and `unknown(A)` of one article over a scope.
    #[must_use]
    pub fn article(&self, article: &str, scope: Option<usize>) -> Ratio {
        ratio(&self.ratio_rows(Some(article), scope))
    }

    /// `cov(A)`, `unknown(A)` and the state of one article over a set of
    /// functions: an org unit, one function, or all of them.
    #[must_use]
    pub fn article_over(&self, article: &str, functions: &[usize]) -> (Ratio, RowState) {
        let rows: Vec<RatioRow> = self
            .rows
            .iter()
            .filter(|row| row.req.article == article)
            .map(|row| RatioRow {
                weight: f64::from(row.req.weight),
                state: state_of(row.among(functions)),
                cells: row
                    .among(functions)
                    .filter(|c| c.cell.state == CellState::Scored)
                    .map(|c| (self.weight(c.function), c.cell.v))
                    .collect(),
            })
            .collect();
        (ratio(&rows), strongest(rows.iter().map(|r| r.state)))
    }

    /// Portfolio coverage: the mean of the article coverages weighted by
    /// each article's row weights, with the unknown share of all rows.
    #[must_use]
    pub fn portfolio(&self, scope: Option<usize>) -> Ratio {
        let mut weighted = 0.0;
        let mut total = 0.0;
        for (article, _) in ARTICLES {
            let rows = self.ratio_rows(Some(article), scope);
            let Some(coverage) = ratio(&rows).coverage else {
                continue;
            };
            let weight: f64 = rows
                .iter()
                .filter(|r| matches!(r.state, RowState::Known | RowState::Unknown))
                .map(|r| r.weight)
                .sum();
            weighted += weight * coverage;
            total += weight;
        }
        Ratio {
            coverage: (total > 0.0).then(|| weighted / total),
            unknown: ratio(&self.ratio_rows(None, scope)).unknown,
        }
    }
}

fn matrix_cell<'a>(
    ctx: &Ctx<'a>,
    req: &'static Requirement,
    index: usize,
    function: &'a Function,
) -> MatrixCell<'a> {
    let as_of_ns = ctx.inputs.as_of_ns;
    let cell = ctx.evaluate(req, function, as_of_ns);
    let today = day_of(as_of_ns);
    let stored = ctx
        .inputs
        .snapshots
        .get(&(function.id.clone(), req.id.to_string()));
    // Only a row the daily job stored on its own day counts: nothing is
    // reconstructed from the time of the evidence.
    let stored_on = |day: i64| stored.and_then(|days| days.get(&day)).copied().flatten();
    let holds = cell.state == CellState::Scored && ctx.params.holds(cell.v);
    let streak_days = if holds {
        status::streak_days(stored_on, today, ctx.params)
    } else {
        0
    };
    MatrixCell {
        function: index,
        declared: ctx.declared(req, function),
        cell,
        streak_days,
    }
}

/// One row as the ratio sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct RatioRow {
    pub weight: f64,
    pub state: RowState,
    /// `(c_f, v)` of every scored cell.
    pub cells: Vec<(f64, f64)>,
}

/// A coverage value with the unknown share it must be shown with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio {
    /// `None` when no row is known.
    pub coverage: Option<f64>,
    pub unknown: f64,
}

/// `cov` and `unknown` over a set of rows.
#[must_use]
pub fn ratio(rows: &[RatioRow]) -> Ratio {
    let mut num = 0.0;
    let mut den = 0.0;
    let mut known = 0.0;
    let mut unknown = 0.0;
    for row in rows {
        match row.state {
            RowState::Known => {
                known += row.weight;
                num += row.weight * row.cells.iter().map(|(c, v)| c * v).sum::<f64>();
                den += row.weight * row.cells.iter().map(|(c, _)| c).sum::<f64>();
            }
            RowState::Unknown => unknown += row.weight,
            RowState::NoEvents | RowState::Na => {}
        }
    }
    Ratio {
        coverage: (den > 0.0).then(|| num / den),
        unknown: if known + unknown > 0.0 {
            unknown / (known + unknown)
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use tumult_core::compliance::{requirement, EntityProfile};

    use super::super::model::{parse_day, Outcome, PlannedTest, Tier, DAY_NS};
    use super::*;

    fn ns(date: &str) -> i64 {
        parse_day(date).unwrap() * DAY_NS + DAY_NS / 2
    }

    fn planned(function: &mut Function) {
        function.test_plan = vec![PlannedTest {
            kind: "chaos".into(),
            interval_days: 365,
        }];
        function.last_assessed_ns = Some(ns("2026-09-01"));
    }

    /// The Art. 24 worked example: checkout (critical), ledger (important)
    /// and an intranet function the Art. 24 rows do not bind.
    fn art24_inputs() -> Inputs {
        let mut inputs = Inputs::empty(ns("2026-11-13"), EntityProfile::full());
        let mut checkout = Function::new("cif-0042", Tier::Critical);
        let mut ledger = Function::new("cif-0017", Tier::Important);
        planned(&mut checkout);
        planned(&mut ledger);
        inputs.functions = vec![checkout, ledger, Function::new("cif-0090", Tier::Other)];
        let mut failed = Evidence::run("run-ledger-dr", "cif-0017", "dr", ns("2025-06-20"));
        failed.outcome = Outcome::Failed;
        // The stored record of the criticality assessment: the date in
        // the register alone covers nothing.
        let mut assessed = Evidence::manual("man-assessed", &[], "other", ns("2026-09-01"));
        assessed.cited = vec!["dora.art24.3".into()];
        inputs.evidence = vec![
            Evidence::run("run-checkout-chaos", "cif-0042", "chaos", ns("2026-10-02")),
            failed,
            assessed,
        ];
        inputs
    }

    #[test]
    fn freshness_is_one_inside_the_window_then_halves_every_quarter_window() {
        let p = HouseParams::default();
        assert_eq!(freshness(0, 365, &p), 1.0);
        assert_eq!(freshness(365, 365, &p), 1.0);
        assert!((freshness(511, 365, &p) - 0.33).abs() < 0.005);
        let one_step = 365 + 365 / 4;
        assert!((freshness(one_step, 365, &p) - 0.5).abs() < 0.01);
        assert_eq!(freshness(-3, 365, &p), 1.0);
    }

    #[test]
    fn article_24_matches_the_worked_example() {
        let inputs = art24_inputs();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);

        let row = matrix.row("dora.art24.6").unwrap();
        assert_eq!(
            row.cells.len(),
            2,
            "24(6) binds critical and important only"
        );
        let checkout = &row.cells[0];
        assert_eq!(checkout.cell.v, 1.0);
        assert_eq!(checkout.cell.best.unwrap().age_days, 42);
        let ledger = &row.cells[1];
        assert_eq!(ledger.cell.best.unwrap().age_days, 511);
        assert!((ledger.cell.v - 0.165).abs() < 0.002, "{}", ledger.cell.v);
        let r246 = ratio(&[RatioRow {
            weight: 5.0,
            state: row.state(None),
            cells: vec![(3.0, checkout.cell.v), (2.0, ledger.cell.v)],
        }]);
        assert!((r246.coverage.unwrap() - 0.67).abs() < 0.005);

        assert_eq!(
            matrix.row("dora.art24.4").unwrap().state(None),
            RowState::Unknown
        );
        let open = &matrix.row("dora.art24.5").unwrap().cells[1];
        assert_eq!(open.cell.v, 0.0, "ledger's open finding holds 24(5) at 0");
        assert!(open
            .cell
            .reason
            .as_deref()
            .unwrap()
            .contains("open finding"));

        let art24 = matrix.article("24", None);
        assert!((art24.coverage.unwrap() - 0.78).abs() < 0.005, "{art24:?}");
        assert!((art24.unknown - 2.0 / 13.0).abs() < 1e-9, "{art24:?}");
    }

    /// The Art. 24 inputs with nothing in the lake: a plan, a schedule
    /// and an assessment date, all of them declarations.
    fn declared_only() -> Inputs {
        let mut inputs = art24_inputs();
        inputs.evidence.clear();
        for function in &mut inputs.functions[..2] {
            function.scheduled = true;
        }
        inputs
    }

    #[test]
    fn an_org_file_alone_never_covers_the_programme_or_the_assessment() {
        let inputs = declared_only();
        let params = HouseParams::default();
        let mut matrix = Matrix::build(&inputs, &params);
        // Even with every day of the Operating window stored.
        for row in &mut matrix.rows {
            for cell in &mut row.cells {
                cell.streak_days = params.operating_days;
            }
        }
        let report = status::report(&matrix, None);
        for id in ["dora.art24.1", "dora.art24.3"] {
            let row = matrix.row(id).unwrap();
            assert_eq!(row.cells.len(), 2, "{id}");
            for cell in &row.cells {
                assert_eq!(cell.cell.state, CellState::Scored, "{id}");
                assert_eq!(cell.cell.v, 0.0, "{id}: a declaration covers nothing");
                let reason = cell.cell.reason.as_deref().unwrap_or_default();
                assert!(reason.contains("declared"), "{id}: {reason}");
                assert!(cell.declared, "{id}");
                assert_eq!(
                    status::cell_status(cell, &params),
                    status::Status::Designed,
                    "{id}"
                );
            }
            let stands = report.row(id).unwrap();
            assert_eq!(stands.status, status::Status::Designed, "{id}");
            assert_eq!(
                stands.standing,
                status::Standing::Gap,
                "{id}: the no-gap gate reads this"
            );
        }
    }

    #[test]
    fn the_programme_row_scores_once_a_run_or_an_attestation_is_stored() {
        let mut inputs = declared_only();
        let params = HouseParams::default();
        let req = requirement("dora.art24.1").unwrap();
        inputs.evidence.push(Evidence::run(
            "run-checkout-chaos",
            "cif-0042",
            "chaos",
            ns("2026-10-02"),
        ));
        let mut record = Evidence::manual("man-prog", &["cif-0017"], "other", ns("2026-07-01"));
        record.cited = vec!["dora.art24.1".into()];
        inputs.evidence.push(record);
        let ctx = Ctx::new(&inputs, &params);
        for function in &inputs.functions[..2] {
            let cell = ctx.evaluate(req, function, inputs.as_of_ns);
            assert_eq!(cell.v, 1.0, "{}", function.id);
            assert!(
                cell.best.is_some(),
                "{}: the stored record is named",
                function.id
            );
        }
        let before = ctx.evaluate(req, &inputs.functions[0], ns("2026-10-01"));
        assert_eq!(before.v, 0.0, "nothing was stored yet on that day");

        let mut undeclared = inputs.clone();
        undeclared.functions[0].test_plan.clear();
        undeclared.functions[0].scheduled = false;
        let ctx = Ctx::new(&undeclared, &params);
        let cell = ctx.evaluate(req, &undeclared.functions[0], undeclared.as_of_ns);
        assert_eq!(cell.v, 0.0, "a run without a declared programme is not one");
    }

    #[test]
    fn the_assessment_row_takes_its_date_from_a_stored_attestation_only() {
        let mut inputs = declared_only();
        let params = HouseParams::default();
        let req = requirement("dora.art24.3").unwrap();
        // The org file says it was assessed yesterday; the stored record
        // is two windows old.
        inputs.functions[0].last_assessed_ns = Some(ns("2026-11-12"));
        let old = inputs.as_of_ns - 2 * i64::from(req.window_days) * DAY_NS;
        let mut record = Evidence::manual("man-assess", &["cif-0042"], "other", old);
        record.cited = vec!["dora.art24.3".into()];
        inputs.evidence.push(record);
        let ctx = Ctx::new(&inputs, &params);
        let stale = ctx.evaluate(req, &inputs.functions[0], inputs.as_of_ns);
        assert!(stale.v > 0.0 && stale.v < 0.1, "{}", stale.v);
        assert_eq!(stale.best.unwrap().evidence.id, "man-assess");
        let unrecorded = ctx.evaluate(req, &inputs.functions[1], inputs.as_of_ns);
        assert_eq!(unrecorded.v, 0.0);

        let mut fresh = Evidence::manual("man-assess-2", &["cif-0042"], "other", ns("2026-09-01"));
        fresh.cited = vec!["dora.art24.3".into()];
        inputs.evidence.push(fresh);
        let ctx = Ctx::new(&inputs, &params);
        let cell = ctx.evaluate(req, &inputs.functions[0], inputs.as_of_ns);
        assert_eq!(cell.v, 1.0);
    }

    #[test]
    fn a_per_function_cell_uses_the_same_ratio_without_the_sum_over_functions() {
        let inputs = art24_inputs();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let checkout = matrix.article("24", Some(0));
        assert_eq!(checkout.coverage, Some(1.0));
        assert!((checkout.unknown - 2.0 / 13.0).abs() < 1e-9);
        let intranet = matrix.article("24", Some(2));
        assert_eq!(intranet.coverage, None, "Art. 24 rows do not bind it");
        assert_eq!(intranet.unknown, 0.0);
    }

    #[test]
    fn evidence_dated_after_as_of_is_ignored() {
        let inputs = art24_inputs();
        let params = HouseParams::default();
        let ctx = Ctx::new(&inputs, &params);
        let req = requirement("dora.art24.6").unwrap();
        let before = ctx.evaluate(req, &inputs.functions[0], ns("2026-10-01"));
        assert_eq!(before.v, 0.0);
        let after = ctx.evaluate(req, &inputs.functions[0], ns("2026-10-02") + 1);
        assert_eq!(after.v, 1.0);
    }

    #[test]
    fn a_manual_record_standing_in_for_a_test_is_capped_at_supporting() {
        let mut inputs = art24_inputs();
        let mut restore = Evidence::manual("man-1", &["cif-0017"], "drill", ns("2026-02-10"));
        restore.kind = Some("dr".into());
        inputs.evidence.push(restore);
        let params = HouseParams::default();
        let ctx = Ctx::new(&inputs, &params);
        let cell = ctx.evaluate(
            requirement("dora.art12.2").unwrap(),
            &inputs.functions[1],
            inputs.as_of_ns,
        );
        assert_eq!(cell.v, 0.75);
        assert_eq!(cell.best.unwrap().evidence.id, "man-1");
    }

    /// A penetration test record is not a threat-led test: only a record
    /// of type `tlpt` scores the Art. 26(1) row.
    #[test]
    fn a_plain_pentest_record_does_not_score_the_tlpt_row() {
        let mut inputs = art24_inputs();
        inputs.entity = EntityProfile::parse("full", &["tlpt_designated".to_string()]).unwrap();
        inputs.evidence.push(Evidence::manual(
            "man-pentest",
            &["cif-0042"],
            "pentest",
            ns("2026-08-01"),
        ));
        let params = HouseParams::default();
        let req = requirement("dora.art26.1").unwrap();
        let plain = Ctx::new(&inputs, &params)
            .evaluate(req, &inputs.functions[0], inputs.as_of_ns)
            .v;
        assert_eq!(plain, 0.0, "a pentest record scored the TLPT row");

        inputs.evidence.push(Evidence::manual(
            "man-tlpt",
            &["cif-0042"],
            "tlpt",
            ns("2026-08-02"),
        ));
        let ctx = Ctx::new(&inputs, &params);
        let cell = ctx.evaluate(req, &inputs.functions[0], inputs.as_of_ns);
        assert_eq!(cell.v, 1.0);
        assert_eq!(cell.best.unwrap().evidence.id, "man-tlpt");
    }

    /// Art. 6(5) sets no yearly interval for a microenterprise, so an
    /// old review still counts in full there and decays everywhere else.
    #[test]
    fn a_microenterprise_framework_review_does_not_age_after_a_year() {
        let mut inputs = art24_inputs();
        let mut review = Evidence::manual("man-review", &["cif-0042"], "other", ns("2023-01-10"));
        review.cited = vec!["dora.art6.5".to_string()];
        inputs.evidence.push(review);
        let params = HouseParams::default();
        let req = requirement("dora.art6.5").unwrap();
        let yearly = Ctx::new(&inputs, &params)
            .evaluate(req, &inputs.functions[0], inputs.as_of_ns)
            .v;
        assert!(yearly < 0.01, "{yearly}");

        inputs.entity = EntityProfile::parse("full", &["microenterprise".to_string()]).unwrap();
        let periodic = Ctx::new(&inputs, &params)
            .evaluate(req, &inputs.functions[0], inputs.as_of_ns)
            .v;
        assert_eq!(periodic, 1.0);
    }

    #[test]
    fn a_tabletop_counts_only_for_the_functions_it_names() {
        let mut inputs = art24_inputs();
        inputs.evidence.push(Evidence::manual(
            "man-tt",
            &["cif-0042"],
            "tabletop",
            ns("2026-08-01"),
        ));
        let params = HouseParams::default();
        let ctx = Ctx::new(&inputs, &params);
        let req = requirement("dora.art11.6b").unwrap();
        let named = ctx.evaluate(req, &inputs.functions[0], inputs.as_of_ns);
        let unnamed = ctx.evaluate(req, &inputs.functions[1], inputs.as_of_ns);
        assert_eq!(named.v, 1.0);
        assert_eq!(unnamed.v, 0.0);
        assert_eq!(unnamed.state, CellState::Scored);
    }

    #[test]
    fn an_attestation_is_unknown_until_a_record_cites_the_row() {
        let mut inputs = art24_inputs();
        let params = HouseParams::default();
        let req = requirement("dora.art24.4").unwrap();
        {
            let ctx = Ctx::new(&inputs, &params);
            let cell = ctx.evaluate(req, &inputs.functions[0], inputs.as_of_ns);
            assert_eq!(cell.state, CellState::Unknown);
            assert!(cell.reason.unwrap().contains("dora.art24.4"));
        }
        let mut record = Evidence::manual("man-ind", &[], "other", ns("2026-07-01"));
        record.cited = vec!["dora.art24.4".into()];
        inputs.evidence.push(record);
        let ctx = Ctx::new(&inputs, &params);
        for function in &inputs.functions[..2] {
            let cell = ctx.evaluate(req, function, inputs.as_of_ns);
            assert_eq!(cell.state, CellState::Scored);
            assert_eq!(
                cell.v, 1.0,
                "an entity-wide record speaks for every function"
            );
        }
    }

    #[test]
    fn incident_rows_are_unknown_without_a_feed_and_no_events_with_an_empty_one() {
        let mut inputs = art24_inputs();
        let params = HouseParams::default();
        let req = requirement("dora.art19.4.a").unwrap();
        {
            let matrix = Matrix::build(&inputs, &params);
            assert_eq!(matrix.row(req.id).unwrap().state(None), RowState::Unknown);
            let art19 = matrix.article("19", None);
            assert_eq!(art19.coverage, None);
            assert_eq!(art19.unknown, 1.0);
        }
        inputs.incidents = IncidentFeed::Connected(Vec::new());
        let matrix = Matrix::build(&inputs, &params);
        assert_eq!(matrix.row(req.id).unwrap().state(None), RowState::NoEvents);
        let art19 = matrix.article("19", None);
        assert_eq!(art19.coverage, None);
        assert_eq!(art19.unknown, 0.0, "no events is not unknown");
    }

    #[test]
    fn the_simplified_profile_has_no_article_5_to_15_rows() {
        let mut inputs = art24_inputs();
        inputs.entity = EntityProfile::parse("simplified", &[]).unwrap();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let full_only = ["5", "6", "9", "10", "11", "12", "13"];
        assert!(matrix
            .rows
            .iter()
            .all(|row| !full_only.contains(&row.req.article)));
        assert!(matrix.row("rts1774.art40").is_some());
        assert!(matrix.na.iter().any(|req| req.id == "dora.art12.6"));
        assert!(matrix.na.iter().any(|req| req.id == "dora.art26.1"));
    }

    /// Deterministic pseudo-random stream for the property tests.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64) / ((1_u64 << 53) as f64)
        }

        fn pick(&mut self, n: usize) -> usize {
            ((self.next() * n as f64) as usize).min(n - 1)
        }

        fn row(&mut self) -> RatioRow {
            let state = [
                RowState::Known,
                RowState::Unknown,
                RowState::NoEvents,
                RowState::Na,
            ][self.pick(4)];
            let weight = [1.0, 2.0, 3.0, 5.0][self.pick(4)];
            let cells = (0..=self.pick(5))
                .map(|_| ([3.0, 2.0, 1.5][self.pick(3)], self.next()))
                .collect();
            RatioRow {
                weight,
                state,
                cells: if state == RowState::Known {
                    cells
                } else {
                    Vec::new()
                },
            }
        }

        fn rows(&mut self) -> Vec<RatioRow> {
            (0..=self.pick(12)).map(|_| self.row()).collect()
        }
    }

    #[test]
    fn property_coverage_and_unknown_share_stay_within_zero_and_one() {
        let mut rng = Lcg(7);
        for _ in 0..2_000 {
            let r = ratio(&rng.rows());
            assert!((0.0..=1.0).contains(&r.unknown), "{r:?}");
            if let Some(cov) = r.coverage {
                assert!((0.0..=1.0).contains(&cov), "{r:?}");
            }
        }
    }

    #[test]
    fn property_adding_an_na_row_never_changes_coverage_or_unknown() {
        let mut rng = Lcg(11);
        for _ in 0..2_000 {
            let mut rows = rng.rows();
            let before = ratio(&rows);
            let at = rng.pick(rows.len() + 1);
            rows.insert(
                at,
                RatioRow {
                    weight: 5.0,
                    state: RowState::Na,
                    cells: Vec::new(),
                },
            );
            assert_eq!(ratio(&rows), before);
        }
    }

    #[test]
    fn property_adding_an_unknown_row_changes_only_the_unknown_share() {
        let mut rng = Lcg(13);
        for _ in 0..2_000 {
            let mut rows = rng.rows();
            let before = ratio(&rows);
            rows.push(RatioRow {
                weight: [1.0, 2.0, 3.0, 5.0][rng.pick(4)],
                state: RowState::Unknown,
                cells: Vec::new(),
            });
            let after = ratio(&rows);
            assert_eq!(after.coverage, before.coverage);
            assert!(after.unknown > before.unknown || before.unknown == 1.0);
        }
    }

    #[test]
    fn property_a_no_events_row_changes_nothing() {
        let mut rng = Lcg(17);
        for _ in 0..500 {
            let mut rows = rng.rows();
            let before = ratio(&rows);
            rows.push(RatioRow {
                weight: 2.0,
                state: RowState::NoEvents,
                cells: Vec::new(),
            });
            assert_eq!(ratio(&rows), before);
        }
    }

    #[test]
    fn the_matrix_does_not_depend_on_evidence_order() {
        let inputs = art24_inputs();
        let mut reversed = inputs.clone();
        reversed.evidence.reverse();
        let params = HouseParams::default();
        let a = Matrix::build(&inputs, &params);
        let b = Matrix::build(&reversed, &params);
        assert_eq!(a.portfolio(None), b.portfolio(None));
    }

    #[test]
    fn an_article_over_a_set_of_functions_matches_the_single_scopes() {
        let inputs = super::super::worked::bank_a();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let all: Vec<usize> = (0..inputs.functions.len()).collect();
        for (article, _) in ARTICLES {
            let (whole, _) = matrix.article_over(article, &all);
            assert_eq!(whole, matrix.article(article, None), "article {article}");
            let (one, _) = matrix.article_over(article, &[2]);
            assert_eq!(one, matrix.article(article, Some(2)), "article {article}");
        }
    }

    #[test]
    fn an_article_state_is_the_most_informative_of_its_rows() {
        let inputs = super::super::worked::firm_b();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        // The incident feed is connected and empty.
        let (ratio, state) = matrix.article_over("19", &[0]);
        assert_eq!(state, RowState::NoEvents);
        assert_eq!(ratio.coverage, None);
        assert_eq!(ratio.unknown, 0.0);
        // Arts. 5-15 do not apply under the simplified framework.
        assert_eq!(matrix.article_over("9", &[0]).1, RowState::Na);
        // No function at all.
        assert_eq!(matrix.article_over("24", &[]).1, RowState::Na);

        let mut blind = inputs.clone();
        blind.incidents = super::super::model::IncidentFeed::NotConnected;
        let matrix = Matrix::build(&blind, &params);
        let (ratio, state) = matrix.article_over("19", &[0]);
        assert_eq!(state, RowState::Unknown);
        assert_eq!(ratio.unknown, 1.0);
        assert_eq!(
            strongest([RowState::Na, RowState::NoEvents, RowState::Unknown].into_iter()),
            RowState::Unknown
        );
    }
}
