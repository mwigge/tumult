//! Maturity per node, rolled up the organisation.
//!
//! The spine is workload → service → component/system → ICT entity
//! (function) → org unit → organisation. Every node has an index and a
//! level:
//!
//! ```text
//! mean(n)  = Σ W(j) · index(j) / Σ W(j)        over children; an unknown child enters at the floor
//! index(n) = ω · own(n) + (1 − ω) · mean(n)    ω by kind; 1 with no children, 0 with no own data
//! level(n) = min(floor(index(n)), gate(n), cap(n))
//! cap(n)   = lowest level among the critical children
//! ```
//!
//! The mean reads the children's uncapped index and the cap reads their
//! level, so a weak child counts once in each. A node that is known to
//! exist and has no data and no known descendant is `unknown`: it enters
//! its parent at the floor with full weight and is never skipped. Caps
//! chain upward, which is why every capped node names the direct children
//! that cap it (`node_ids`) and the deepest node on the chain (`origin`),
//! the one to fix.
//!
//! None of this is prescribed by a regulation: it is a house model, and
//! its numbers live in [`super::params::RollupParams`], all but the
//! floor: an unknown node is index 1 and level 1 by construction.

use std::collections::{BTreeMap, BTreeSet};

use super::coverage::{recovery_met, Ctx, Matrix};
use super::maturity::{
    self, discipline_coverage, floor_level, index_of, slo_attainment, Excluded, Maturity, Pillar,
    Scope,
};
use super::model::{day_of, Evidence, Function, Service, Slo, SloFeed, Tier, Workload};
use super::params::{HouseParams, PillarWeights};

/// Index of an unknown node, and what it enters its parent's mean with:
/// the bottom of the scale. It is not a parameter: nothing an org file
/// says may lift a node no evidence was ingested for.
pub const UNKNOWN_INDEX: f64 = 1.0;
/// Level of an unknown node, whatever gate its input carries.
pub const UNKNOWN_LEVEL: u8 = 1;

/// Id of the organisation node, the root of the spine.
pub const ROOT_ID: &str = "org:";
/// Id of the org node that collects what maps to no function.
pub const UNASSIGNED_ID: &str = "org:(unassigned)";
/// Environment segment of a system id when the environment is not known.
pub const NO_ENV: &str = "_";

/// Kind of a spine node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Workload,
    Service,
    Component,
    System,
    Entity,
    Org,
    Organisation,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workload => "workload",
            Self::Service => "service",
            Self::Component => "component",
            Self::System => "system",
            Self::Entity => "entity",
            Self::Org => "org",
            Self::Organisation => "organisation",
        }
    }

    /// Org units and the organisation only roll up: their weight and
    /// criticality come from what they contain.
    #[must_use]
    pub fn is_org(self) -> bool {
        matches!(self, Self::Org | Self::Organisation)
    }
}

/// One node as the roll-up reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInput {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    /// Spine parent; a node naming none, or one that does not exist,
    /// hangs under the root.
    pub parent: Option<String>,
    /// `W(n)` for an entity and below. Ignored for org nodes, whose
    /// weight is the sum of what they contain.
    pub weight: f64,
    /// Is, supports or contains a critical function. Ignored for org
    /// nodes.
    pub critical: bool,
    /// The node's own score 1–5; `None` when none of its own pillars has
    /// data.
    pub own: Option<f64>,
    /// The own pillars behind `own`, by letter.
    pub pillars: Vec<(&'static str, Pillar)>,
    /// Highest gate passed; 5 where no gate is evaluated.
    pub gate: u8,
    pub gate_reason: Option<String>,
    /// Why the node has no data, for an unknown node.
    pub note: Option<String>,
}

impl NodeInput {
    /// A node with no own score and no gate.
    #[must_use]
    pub fn new(id: &str, kind: Kind, parent: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            kind,
            label: id.to_string(),
            parent: parent.map(str::to_string),
            weight: 0.0,
            critical: false,
            own: None,
            pillars: Vec::new(),
            gate: 5,
            gate_reason: None,
            note: None,
        }
    }
}

/// What holds a node's level below its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapRule {
    None,
    /// A critical child sits at a lower level.
    WeakestCriticalChild,
    /// A level gate fails.
    Gate,
}

impl CapRule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WeakestCriticalChild => "weakest_critical_child",
            Self::Gate => "gate",
        }
    }
}

/// The cap on a node's level and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cap {
    pub rule: CapRule,
    pub level: Option<u8>,
    /// The direct children that set the cap.
    pub node_ids: Vec<String>,
    /// The deepest nodes on the capping chain: the ones to fix.
    pub origin: Vec<String>,
    /// `unknown`, `known`, or `mixed` over the origin nodes.
    pub origin_state: Option<&'static str>,
    /// The failing gate condition, for a gate cap.
    pub reason: Option<String>,
}

impl Cap {
    fn none() -> Self {
        Self {
            rule: CapRule::None,
            level: None,
            node_ids: Vec::new(),
            origin: Vec::new(),
            origin_state: None,
            reason: None,
        }
    }
}

/// One node's maturity.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeScore {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub parent: Option<String>,
    pub index: f64,
    pub level: u8,
    pub target: u8,
    pub own: Option<f64>,
    pub pillars: Vec<(&'static str, Pillar)>,
    /// Weighted mean of the children's indexes; `None` without children.
    pub mean: Option<f64>,
    pub weight: f64,
    pub children: usize,
    /// Share of the weight below this node that is unknown.
    pub unknown_share: f64,
    pub unknown: bool,
    pub critical: bool,
    pub cap: Cap,
    /// Children that do not cap but pull the index down: non-critical
    /// ones below this node's index, or unknown.
    pub drag: Vec<String>,
    pub note: Option<String>,
}

/// Parent of every node, with dangling and cyclic parents re-pointed at
/// the root so nothing is dropped.
fn parents<'a>(
    nodes: &BTreeMap<&'a str, &'a NodeInput>,
    root: &'a str,
) -> BTreeMap<&'a str, &'a str> {
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, node) in nodes {
        if *id == root {
            continue;
        }
        let named = node
            .parent
            .as_deref()
            .and_then(|p| nodes.get_key_value(p))
            .map_or(root, |(key, _)| *key);
        parent.insert(id, named);
    }
    let ids: Vec<&str> = parent.keys().copied().collect();
    for id in ids {
        let mut at = id;
        let mut steps = 0;
        while at != root && steps <= nodes.len() {
            at = parent.get(at).copied().unwrap_or(root);
            steps += 1;
        }
        if at != root {
            parent.insert(id, root);
        }
    }
    parent
}

/// Node ids, children before parents.
fn post_order<'a>(root: &'a str, children: &BTreeMap<&'a str, Vec<&'a str>>) -> Vec<&'a str> {
    let mut order = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        order.push(id);
        stack.extend(children.get(id).into_iter().flatten().copied());
    }
    order.reverse();
    order
}

fn weighted(kids: &[&NodeScore], value: impl Fn(&NodeScore) -> f64) -> Option<f64> {
    let total: f64 = kids.iter().map(|kid| kid.weight).sum();
    if total <= 0.0 {
        return None;
    }
    Some(kids.iter().map(|kid| kid.weight * value(kid)).sum::<f64>() / total)
}

fn own_share(node: &NodeInput, has_children: bool, params: &HouseParams) -> f64 {
    if node.own.is_none() {
        return 0.0;
    }
    if !has_children {
        return 1.0;
    }
    params
        .rollup
        .own_share
        .get(node.kind.as_str())
        .copied()
        .unwrap_or(0.0)
}

fn origin_state(origin: &[String], scores: &BTreeMap<&str, NodeScore>) -> Option<&'static str> {
    let unknown = origin
        .iter()
        .filter(|id| scores.get(id.as_str()).is_some_and(|node| node.unknown))
        .count();
    if origin.is_empty() {
        return None;
    }
    if unknown == origin.len() {
        return Some("unknown");
    }
    if unknown == 0 {
        return Some("known");
    }
    Some("mixed")
}

fn child_cap(level: u8, kids: &[&NodeScore], scores: &BTreeMap<&str, NodeScore>) -> Cap {
    let capping: Vec<&&NodeScore> = kids
        .iter()
        .filter(|kid| kid.critical && kid.level == level)
        .collect();
    let mut origin = BTreeSet::new();
    for kid in &capping {
        if kid.cap.rule == CapRule::WeakestCriticalChild {
            origin.extend(kid.cap.origin.iter().cloned());
            continue;
        }
        origin.insert(kid.id.clone());
    }
    let origin: Vec<String> = origin.into_iter().collect();
    Cap {
        rule: CapRule::WeakestCriticalChild,
        level: Some(level),
        node_ids: capping.iter().map(|kid| kid.id.clone()).collect(),
        origin_state: origin_state(&origin, scores),
        origin,
        reason: None,
    }
}

fn score(
    node: &NodeInput,
    parent: Option<&str>,
    kids: &[&NodeScore],
    scores: &BTreeMap<&str, NodeScore>,
    target: u8,
    params: &HouseParams,
) -> NodeScore {
    let floor = UNKNOWN_INDEX;
    let own = node.own.filter(|_| !node.kind.is_org());
    let unknown = own.is_none() && kids.iter().all(|kid| kid.unknown);
    let mean = weighted(kids, |kid| kid.index);
    let omega = own_share(node, mean.is_some(), params);
    let index = match (own, mean) {
        _ if unknown => floor,
        (Some(own), Some(mean)) => omega * own + (1.0 - omega) * mean,
        (Some(own), None) => own,
        (None, Some(mean)) => mean,
        (None, None) => floor,
    };
    let weight = if node.kind.is_org() {
        let sum: f64 = kids.iter().map(|kid| kid.weight).sum();
        if kids.is_empty() {
            params.rollup.unassigned_weight
        } else {
            sum
        }
    } else {
        node.weight
    };
    let critical = if node.kind.is_org() {
        kids.iter().any(|kid| kid.critical)
    } else {
        node.critical
    };
    let by_index = floor_level(index);
    let gate = if unknown { UNKNOWN_LEVEL } else { node.gate };
    let base = by_index.min(gate);
    let weakest = kids
        .iter()
        .filter(|kid| kid.critical)
        .map(|kid| kid.level)
        .min();
    let cap = match weakest {
        Some(level) if level < base => child_cap(level, kids, scores),
        _ if gate < by_index => Cap {
            rule: CapRule::Gate,
            level: Some(gate),
            reason: node.gate_reason.clone(),
            ..Cap::none()
        },
        _ => Cap::none(),
    };
    let drag = kids
        .iter()
        .filter(|kid| !kid.critical && (kid.unknown || kid.index < index - 1e-9))
        .map(|kid| kid.id.clone())
        .collect();
    NodeScore {
        id: node.id.clone(),
        kind: node.kind,
        label: node.label.clone(),
        parent: parent.map(str::to_string),
        index,
        level: base.min(weakest.unwrap_or(5)),
        target,
        own,
        pillars: node.pillars.clone(),
        mean,
        weight,
        children: kids.len(),
        unknown_share: if unknown {
            1.0
        } else {
            weighted(kids, |kid| kid.unknown_share).unwrap_or(0.0)
        },
        unknown,
        critical,
        cap,
        drag,
        note: node.note.clone(),
    }
}

/// Roll the nodes up to `root`. The result is sorted by id and does not
/// depend on the order of `inputs`. Empty when `root` is not among them.
#[must_use]
pub fn roll_up(
    inputs: &[NodeInput],
    root: &str,
    target: u8,
    params: &HouseParams,
) -> Vec<NodeScore> {
    let nodes: BTreeMap<&str, &NodeInput> = inputs.iter().map(|n| (n.id.as_str(), n)).collect();
    let Some((root, _)) = nodes.get_key_value(root) else {
        return Vec::new();
    };
    let parent = parents(&nodes, root);
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, parent) in &parent {
        children.entry(parent).or_default().push(id);
    }
    let mut scores: BTreeMap<&str, NodeScore> = BTreeMap::new();
    for id in post_order(root, &children) {
        let kids: Vec<&NodeScore> = children
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|kid| scores.get(kid))
            .collect();
        let scored = score(
            nodes[id],
            parent.get(id).copied(),
            &kids,
            &scores,
            target,
            params,
        );
        scores.insert(id, scored);
    }
    scores.into_values().collect()
}

/// Id of an entity node.
#[must_use]
pub fn entity_id(function: &str) -> String {
    format!("ent:{function}")
}

/// Id of a service node.
#[must_use]
pub fn service_id(name: &str) -> String {
    format!("svc:{name}")
}

/// Id of a system node.
#[must_use]
pub fn system_id(env: Option<&str>, system: &str) -> String {
    format!("sys:{}/{system}", env.unwrap_or(NO_ENV))
}

/// Id of a component node.
#[must_use]
pub fn component_id(env: Option<&str>, system: &str, component: &str) -> String {
    format!("cmp:{}/{system}/{component}", env.unwrap_or(NO_ENV))
}

/// Id of an org-unit node.
#[must_use]
pub fn org_id(path: &str) -> String {
    format!("org:{path}")
}

fn profile_weight(weights: &PillarWeights, letter: &str) -> f64 {
    match letter {
        "C" => weights.conformance,
        "T" => weights.coverage,
        "S" => weights.slo,
        "R" => weights.recovery,
        "L" => weights.loop_,
        _ => 0.0,
    }
}

const LETTERS: [&str; 5] = ["C", "T", "S", "R", "L"];

/// Builds the spine of one legal entity from the score inputs.
struct Builder<'m, 'a> {
    matrix: &'m Matrix<'a>,
    nodes: BTreeMap<String, NodeInput>,
}

/// What a node below the entity inherits from the functions it supports.
struct Support<'a> {
    /// The function the node hangs under: most critical first, then id.
    best: Option<&'a Function>,
    weight: f64,
    critical: bool,
}

impl<'a> Builder<'_, 'a> {
    fn ctx(&self) -> &Ctx<'a> {
        &self.matrix.ctx
    }

    fn params(&self) -> &'a HouseParams {
        self.matrix.ctx.params
    }

    fn support<'i>(&self, functions: impl Iterator<Item = &'i str>) -> Support<'a> {
        let inputs = self.matrix.ctx.inputs;
        let params = self.params();
        let mut known: Vec<&Function> = functions.filter_map(|id| inputs.function(id)).collect();
        known.sort_by(|a, b| (a.tier, &a.id).cmp(&(b.tier, &b.id)));
        let weight = known
            .iter()
            .map(|f| f.tier.weight(params))
            .fold(None, |best: Option<f64>, w| {
                Some(best.map_or(w, |b| b.max(w)))
            });
        Support {
            best: known.first().copied(),
            weight: weight.unwrap_or(params.rollup.unassigned_weight),
            critical: params.rollup.critical_inheritance
                && known.iter().any(|f| f.tier == Tier::Critical),
        }
    }

    /// The own score over the pillars the kind observes, with the profile
    /// weights renormalised over them.
    fn own(&self, pillars: &[(&'static str, Pillar)]) -> Option<f64> {
        let weights = maturity::weights(self.matrix);
        let parts: Vec<(f64, &Pillar)> = pillars
            .iter()
            .map(|(letter, pillar)| (profile_weight(weights, letter), pillar))
            .collect();
        index_of(&parts)
    }

    /// Keep the pillars the kind is configured to observe, in C T S R L
    /// order.
    fn observed(
        &self,
        kind: Kind,
        mut pillars: BTreeMap<&'static str, Pillar>,
    ) -> Vec<(&'static str, Pillar)> {
        let listed = self.params().rollup.own_pillars.get(kind.as_str());
        LETTERS
            .iter()
            .filter(|letter| listed.is_some_and(|list| list.iter().any(|l| l == *letter)))
            .filter_map(|letter| Some((*letter, pillars.remove(letter)?)))
            .collect()
    }

    fn add(&mut self, mut node: NodeInput, pillars: Option<BTreeMap<&'static str, Pillar>>) {
        if let Some(pillars) = pillars {
            node.pillars = self.observed(node.kind, pillars);
            node.own = self.own(&node.pillars);
        }
        self.nodes.insert(node.id.clone(), node);
    }

    fn conformance(&self, service: Option<&Service>) -> Pillar {
        let lmax = maturity::ceiling(self.matrix);
        let Some(service) = service.filter(|s| s.observed) else {
            return Pillar::Value(0.0);
        };
        Pillar::Value(100.0 * f64::from(service.level.min(lmax)) / f64::from(lmax.max(1)))
    }

    /// 100 when the latest recovery measured in the window met its
    /// target, else 0.
    fn recovery(&self, runs: &[&Evidence], function: Option<&Function>) -> Pillar {
        let ctx = self.ctx();
        let as_of_ns = ctx.inputs.as_of_ns;
        let window = i64::from(ctx.params.recovery_window_days);
        let nobody = Function::new("", Tier::Other);
        let function = function.unwrap_or(&nobody);
        let met = runs
            .iter()
            .filter(|e| day_of(as_of_ns) - day_of(e.at_ns) <= window)
            .filter_map(|e| Some((e, recovery_met(e, function)?)))
            .max_by_key(|(e, _)| (e.at_ns, e.id.as_str()))
            .is_some_and(|(_, met)| met);
        Pillar::Value(if met { 100.0 } else { 0.0 })
    }

    fn runs(&self, targets: impl Fn(&Evidence) -> bool) -> Vec<&'a Evidence> {
        let inputs = self.matrix.ctx.inputs;
        inputs
            .evidence
            .iter()
            .filter(|e| e.kind.is_some() && e.at_ns <= inputs.as_of_ns)
            .filter(|e| targets(e))
            .collect()
    }

    fn org_units(&mut self) {
        let inputs = self.matrix.ctx.inputs;
        for path in inputs
            .functions
            .iter()
            .filter_map(|f| f.org_unit.as_deref())
        {
            let mut prefix = String::new();
            let mut parent = ROOT_ID.to_string();
            for segment in path.split('/').filter(|s| !s.is_empty()) {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(segment);
                let id = org_id(&prefix);
                let mut node = NodeInput::new(&id, Kind::Org, Some(&parent));
                node.label = segment.to_string();
                self.nodes.entry(id.clone()).or_insert(node);
                parent = id;
            }
        }
    }

    fn entity_parent(function: &Function) -> String {
        let path: Vec<&str> = function
            .org_unit
            .as_deref()
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if path.is_empty() {
            return ROOT_ID.to_string();
        }
        org_id(&path.join("/"))
    }

    fn entities(&mut self) {
        let inputs = self.matrix.ctx.inputs;
        for (index, function) in inputs.functions.iter().enumerate() {
            let m = maturity::assess(self.matrix, Scope::Function(index));
            let mut node = NodeInput::new(
                &entity_id(&function.id),
                Kind::Entity,
                Some(&Self::entity_parent(function)),
            );
            node.label.clone_from(&function.name);
            node.weight = function.tier.weight(self.params());
            node.critical = function.tier == Tier::Critical;
            node.gate = m.gate_level;
            node.gate_reason = m.blocked.as_ref().map(|(_, reason)| reason.clone());
            let tested = self
                .ctx()
                .evidence_for(&function.id, inputs.as_of_ns)
                .next()
                .is_some();
            if !tested && m.pillars.slo.value().is_none() {
                node.note = Some("no test and no SLO names the function".into());
                self.add(node, None);
                continue;
            }
            let pillars = BTreeMap::from([
                ("C", m.pillars.conformance),
                ("T", m.pillars.coverage),
                ("S", m.pillars.slo),
                ("R", m.pillars.recovery),
                ("L", m.pillars.loop_),
            ]);
            self.add(node, Some(pillars));
        }
    }

    /// (env, system) → components, from services and from evidence.
    fn system_keys(&self) -> BTreeMap<(Option<&'a str>, &'a str), BTreeSet<&'a str>> {
        let inputs = self.matrix.ctx.inputs;
        let mut keys: BTreeMap<(Option<&str>, &str), BTreeSet<&str>> = BTreeMap::new();
        let services = inputs.services.iter().map(|s| {
            (
                s.env.as_deref(),
                s.system.as_deref(),
                s.component.as_deref(),
            )
        });
        let evidence = inputs
            .evidence
            .iter()
            .filter(|e| e.kind.is_some())
            .map(|e| {
                (
                    e.env.as_deref(),
                    e.system.as_deref(),
                    e.component.as_deref(),
                )
            });
        for (env, system, component) in services.chain(evidence) {
            let Some(system) = system else {
                continue;
            };
            let components = keys.entry((env, system)).or_default();
            components.extend(component);
        }
        keys
    }

    fn system_node(
        &mut self,
        id: &str,
        kind: Kind,
        parent: Option<&str>,
        label: &str,
        of: impl Fn(Option<&str>, Option<&str>, Option<&str>) -> bool,
    ) {
        let inputs = self.matrix.ctx.inputs;
        let runs = self.runs(|e| {
            of(
                e.env.as_deref(),
                e.system.as_deref(),
                e.component.as_deref(),
            )
        });
        let services = inputs.services.iter().filter(|s| {
            of(
                s.env.as_deref(),
                s.system.as_deref(),
                s.component.as_deref(),
            )
        });
        let functions = services
            .flat_map(|s| s.functions.iter())
            .chain(runs.iter().flat_map(|e| e.functions.iter()))
            .map(String::as_str);
        let support = self.support(functions);
        let fallback = support
            .best
            .map_or_else(|| UNASSIGNED_ID.to_string(), |f| entity_id(&f.id));
        let mut node = NodeInput::new(id, kind, Some(parent.unwrap_or(&fallback)));
        node.label = label.to_string();
        node.weight = support.weight;
        node.critical = support.critical;
        if runs.is_empty() {
            node.note = Some("no test targeted it".into());
            self.add(node, None);
            return;
        }
        let coverage = 100.0 * discipline_coverage(self.ctx(), runs.iter().copied());
        let pillars = BTreeMap::from([
            ("T", Pillar::Value(coverage)),
            ("R", self.recovery(&runs, support.best)),
        ]);
        self.add(node, Some(pillars));
    }

    fn systems(&mut self) {
        for ((env, system), components) in self.system_keys() {
            let sys = system_id(env, system);
            self.system_node(&sys, Kind::System, None, system, |e, s, _| {
                e == env && s == Some(system)
            });
            for component in components {
                let id = component_id(env, system, component);
                self.system_node(&id, Kind::Component, Some(&sys), component, |e, s, c| {
                    e == env && s == Some(system) && c == Some(component)
                });
            }
        }
    }

    fn service_slo(&self, service: &Service) -> Pillar {
        let SloFeed::Connected { slos, .. } = &self.matrix.ctx.inputs.slos else {
            return Pillar::Excluded(Excluded::NotDeclared);
        };
        let bound: Vec<(&Slo, f64)> = slos
            .iter()
            .filter(|slo| slo.service.as_deref() == Some(service.name.as_str()))
            .map(|slo| (slo, 1.0))
            .collect();
        match slo_attainment(&bound, self.params()) {
            Some(value) => Pillar::Value(value),
            None => Pillar::Excluded(Excluded::NotDeclared),
        }
    }

    fn service_parent(service: &Service, support: &Support<'_>) -> String {
        let env = service.env.as_deref();
        match (service.system.as_deref(), service.component.as_deref()) {
            (Some(system), Some(component)) => component_id(env, system, component),
            (Some(system), None) => system_id(env, system),
            (None, _) => support
                .best
                .map_or_else(|| UNASSIGNED_ID.to_string(), |f| entity_id(&f.id)),
        }
    }

    fn services(&mut self) {
        let inputs = self.matrix.ctx.inputs;
        for service in &inputs.services {
            let support = self.support(service.functions.iter().map(String::as_str));
            let parent = Self::service_parent(service, &support);
            let mut node = NodeInput::new(&service_id(&service.name), Kind::Service, Some(&parent));
            node.label.clone_from(&service.name);
            node.weight = support.weight;
            node.critical = support.critical;
            let runs = self.runs(|e| e.services.contains(&service.name));
            if !service.observed && runs.is_empty() {
                node.note = Some("no telemetry and no test".into());
                self.add(node, None);
                continue;
            }
            let coverage = 100.0 * discipline_coverage(self.ctx(), runs.iter().copied());
            let pillars = BTreeMap::from([
                ("C", self.conformance(Some(service))),
                ("T", Pillar::Value(coverage)),
                ("S", self.service_slo(service)),
            ]);
            self.add(node, Some(pillars));
        }
    }

    fn workload(&mut self, workload: &Workload) {
        let inputs = self.matrix.ctx.inputs;
        let service = workload
            .service
            .as_deref()
            .and_then(|name| inputs.services.iter().find(|s| s.name == name));
        let functions = service.into_iter().flat_map(|s| s.functions.iter());
        let support = self.support(functions.map(String::as_str));
        let parent = service.map_or_else(|| UNASSIGNED_ID.to_string(), |s| service_id(&s.name));
        let mut node = NodeInput::new(&workload.id, Kind::Workload, Some(&parent));
        node.weight = support.weight;
        node.critical = support.critical;
        let runs = self.runs(|e| e.workloads.contains(&workload.id));
        let joined = service.is_some_and(|s| s.observed);
        if !joined && runs.is_empty() {
            node.note = Some("no telemetry and no test".into());
            self.add(node, None);
            return;
        }
        let ctx = self.ctx();
        let best = runs
            .iter()
            .map(|e| ctx.test_value(e, inputs.as_of_ns, ctx.params.test_window_days))
            .fold(0.0, f64::max);
        let pillars = BTreeMap::from([
            ("C", self.conformance(service)),
            ("T", Pillar::Value(100.0 * best)),
            ("R", self.recovery(&runs, support.best)),
        ]);
        self.add(node, Some(pillars));
    }

    fn unassigned(&mut self) {
        let used = self
            .nodes
            .values()
            .any(|node| node.parent.as_deref() == Some(UNASSIGNED_ID));
        if !used {
            return;
        }
        let mut node = NodeInput::new(UNASSIGNED_ID, Kind::Org, Some(ROOT_ID));
        node.label = "(unassigned)".into();
        self.nodes.insert(node.id.clone(), node);
    }
}

/// The spine nodes of the legal entity behind `matrix`. `entity` is its
/// maturity at entity scope, whose gates the organisation node carries.
#[must_use]
pub fn build(matrix: &Matrix<'_>, entity: &Maturity) -> Vec<NodeInput> {
    let mut root = NodeInput::new(ROOT_ID, Kind::Organisation, None);
    root.label = "organisation".into();
    root.gate = entity.gate_level;
    root.gate_reason = entity.blocked.as_ref().map(|(_, reason)| reason.clone());
    let mut builder = Builder {
        matrix,
        nodes: BTreeMap::from([(root.id.clone(), root)]),
    };
    builder.org_units();
    builder.entities();
    builder.systems();
    builder.services();
    for workload in &matrix.ctx.inputs.workloads {
        builder.workload(workload);
    }
    builder.unassigned();
    builder.nodes.into_values().collect()
}

/// Maturity of every spine node of the legal entity behind `matrix`.
#[must_use]
pub fn assess(matrix: &Matrix<'_>, entity: &Maturity) -> Vec<NodeScore> {
    roll_up(
        &build(matrix, entity),
        ROOT_ID,
        entity.target,
        matrix.ctx.params,
    )
}

#[cfg(test)]
mod tests {
    use super::super::model::{round2, Inputs};
    use super::super::worked::{bank_a, firm_b};
    use super::*;

    fn node(
        id: &str,
        kind: Kind,
        parent: Option<&str>,
        weight: f64,
        own: Option<f64>,
    ) -> NodeInput {
        NodeInput {
            weight,
            own,
            ..NodeInput::new(id, kind, parent)
        }
    }

    fn critical(mut node: NodeInput) -> NodeInput {
        node.critical = true;
        node
    }

    /// The organisation "Bank" of the worked example; `ledger` is the own
    /// score of `svc:ledger`, `None` while it is uninstrumented.
    fn bank(ledger: Option<f64>) -> Vec<NodeInput> {
        let checkout = Some("ent:cif-0042");
        vec![
            node("org:bank", Kind::Organisation, None, 0.0, None),
            node("org:bank/payments", Kind::Org, Some("org:bank"), 0.0, None),
            node("org:bank/platform", Kind::Org, Some("org:bank"), 0.0, None),
            critical(node(
                "ent:cif-0042",
                Kind::Entity,
                Some("org:bank/payments"),
                3.0,
                Some(4.2),
            )),
            node(
                "ent:cif-0050",
                Kind::Entity,
                Some("org:bank/payments"),
                2.0,
                Some(3.2),
            ),
            node(
                "ent:cif-0100",
                Kind::Entity,
                Some("org:bank/platform"),
                1.5,
                Some(2.4),
            ),
            critical(node(
                "svc:checkout",
                Kind::Service,
                checkout,
                3.0,
                Some(4.1),
            )),
            critical(node(
                "svc:payments-gw",
                Kind::Service,
                checkout,
                3.0,
                Some(3.4),
            )),
            critical(node("svc:ledger", Kind::Service, checkout, 3.0, ledger)),
            node(
                "svc:reports",
                Kind::Service,
                Some("ent:cif-0050"),
                2.0,
                Some(3.0),
            ),
            critical(node(
                "wl:shop/deployment/checkout",
                Kind::Workload,
                Some("svc:checkout"),
                3.0,
                Some(3.8),
            )),
        ]
    }

    fn rolled(nodes: &[NodeInput]) -> BTreeMap<String, NodeScore> {
        roll_up(nodes, "org:bank", 3, &HouseParams::default())
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn worked_example_with_an_uninstrumented_critical_service() {
        let by_id = rolled(&bank(None));
        let at = |id: &str| &by_id[id];

        assert_eq!(round2(at("wl:shop/deployment/checkout").index), 3.8);
        assert_eq!(at("wl:shop/deployment/checkout").level, 3);
        assert_eq!(round2(at("svc:checkout").index), 3.95);
        assert_eq!(at("svc:checkout").level, 3);
        assert_eq!(at("svc:checkout").cap.rule, CapRule::None);
        assert_eq!(round2(at("svc:payments-gw").index), 3.4);
        assert!(at("svc:ledger").unknown);
        assert_eq!((at("svc:ledger").index, at("svc:ledger").level), (1.0, 1));

        let checkout = at("ent:cif-0042");
        assert_eq!(round2(checkout.mean.unwrap()), 2.78);
        assert_eq!(round2(checkout.index), 3.49);
        assert_eq!(checkout.level, 1);
        assert_eq!(checkout.cap.rule, CapRule::WeakestCriticalChild);
        assert_eq!(checkout.cap.node_ids, ids(&["svc:ledger"]));
        assert_eq!(checkout.cap.origin, ids(&["svc:ledger"]));
        assert_eq!(checkout.cap.origin_state, Some("unknown"));
        assert_eq!(round2(checkout.unknown_share), 0.33);

        assert_eq!(round2(at("ent:cif-0050").index), 3.1);
        assert_eq!(at("ent:cif-0050").level, 3);

        let payments = at("org:bank/payments");
        // The document rounds the entity index to 3.49 before averaging
        // and prints 3.33; unrounded it is 3.335.
        assert!((payments.index - 3.33).abs() < 0.006, "{}", payments.index);
        assert_eq!(payments.weight, 5.0);
        assert_eq!(payments.children, 2);
        assert_eq!(payments.level, 1);
        assert_eq!(payments.cap.node_ids, ids(&["ent:cif-0042"]));
        assert_eq!(payments.cap.origin, ids(&["svc:ledger"]));
        assert_eq!(round2(payments.unknown_share), 0.2);
        // Reporting is not critical and sits below the unit: it drags.
        assert_eq!(payments.drag, ids(&["ent:cif-0050"]));

        assert_eq!(round2(at("ent:cif-0100").index), 2.4);
        assert_eq!(at("org:bank/platform").level, 2);
        assert_eq!(at("org:bank/platform").weight, 1.5);

        let org = at("org:bank");
        assert_eq!(round2(org.index), 3.12);
        assert_eq!((org.level, org.target), (1, 3));
        assert_eq!(org.weight, 6.5);
        assert_eq!(org.cap.rule, CapRule::WeakestCriticalChild);
        assert_eq!(org.cap.level, Some(1));
        assert_eq!(org.cap.node_ids, ids(&["org:bank/payments"]));
        assert_eq!(org.cap.origin, ids(&["svc:ledger"]));
        assert_eq!(org.cap.origin_state, Some("unknown"));
        assert_eq!(round2(org.unknown_share), 0.15);
    }

    #[test]
    fn worked_example_after_the_service_is_instrumented() {
        let by_id = rolled(&bank(Some(3.0)));
        let at = |id: &str| &by_id[id];

        let checkout = at("ent:cif-0042");
        assert_eq!(round2(checkout.mean.unwrap()), 3.45);
        assert_eq!(round2(checkout.index), 3.83);
        assert_eq!(checkout.level, 3);
        assert_eq!(checkout.cap.rule, CapRule::None);
        assert_eq!(checkout.unknown_share, 0.0);

        assert_eq!(round2(at("org:bank/payments").index), 3.54);
        assert_eq!(at("org:bank/payments").level, 3);

        let org = at("org:bank");
        assert_eq!(round2(org.index), 3.27);
        assert_eq!((org.level, org.target), (3, 3));
        assert_eq!(org.cap.rule, CapRule::None);
        assert_eq!(org.drag, ids(&["org:bank/platform"]));
        assert_eq!(org.unknown_share, 0.0);
    }

    #[test]
    fn a_failing_gate_caps_and_names_its_reason() {
        let mut nodes = bank(Some(3.0));
        nodes[0].gate = 2;
        nodes[0].gate_reason = Some("recovery (R) is 40, needs 60".into());
        let by_id = rolled(&nodes);
        let org = &by_id["org:bank"];
        assert_eq!(org.level, 2);
        assert_eq!(org.cap.rule, CapRule::Gate);
        assert_eq!(org.cap.level, Some(2));
        assert_eq!(
            org.cap.reason.as_deref(),
            Some("recovery (R) is 40, needs 60")
        );
    }

    #[test]
    fn an_org_unit_with_nothing_in_it_is_unknown_and_still_counted() {
        let mut nodes = bank(Some(3.0));
        nodes.push(node(
            "org:bank/empty",
            Kind::Org,
            Some("org:bank"),
            0.0,
            None,
        ));
        let by_id = rolled(&nodes);
        let empty = &by_id["org:bank/empty"];
        assert!(empty.unknown);
        assert_eq!((empty.index, empty.level, empty.weight), (1.0, 1, 1.0));
        let org = &by_id["org:bank"];
        assert!(org.index < 3.27);
        assert_eq!(org.drag, ids(&["org:bank/empty", "org:bank/platform"]));
    }

    #[test]
    fn a_dangling_or_cyclic_parent_hangs_under_the_root() {
        let mut nodes = bank(Some(3.0));
        nodes.push(node(
            "svc:lost",
            Kind::Service,
            Some("ent:gone"),
            1.0,
            Some(2.0),
        ));
        nodes.push(node("svc:a", Kind::Service, Some("svc:b"), 1.0, Some(2.0)));
        nodes.push(node("svc:b", Kind::Service, Some("svc:a"), 1.0, Some(2.0)));
        let by_id = rolled(&nodes);
        assert_eq!(by_id.len(), nodes.len());
        assert_eq!(by_id["svc:lost"].parent.as_deref(), Some("org:bank"));
        assert_eq!(by_id["svc:a"].parent.as_deref(), Some("org:bank"));
        assert_eq!(by_id["svc:b"].parent.as_deref(), Some("svc:a"));
    }

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

        /// A random spine: org units, entities, services and workloads,
        /// with some nodes left without data.
        fn tree(&mut self) -> Vec<NodeInput> {
            let mut nodes = vec![node("org:bank", Kind::Organisation, None, 0.0, None)];
            let units = 1 + self.pick(3);
            for u in 0..units {
                nodes.push(node(
                    &format!("org:bank/u{u}"),
                    Kind::Org,
                    Some("org:bank"),
                    0.0,
                    None,
                ));
            }
            let weights = [3.0, 2.0, 1.5];
            for e in 0..(2 + self.pick(5)) {
                let tier = self.pick(3);
                let ent = format!("ent:f-{e}");
                let unit = format!("org:bank/u{}", self.pick(units));
                let mut entity = node(&ent, Kind::Entity, Some(&unit), weights[tier], self.own());
                entity.critical = tier == 0;
                entity.gate = 1 + self.pick(5) as u8;
                nodes.push(entity);
                for s in 0..self.pick(4) {
                    let svc = format!("svc:f-{e}-{s}");
                    let mut service =
                        node(&svc, Kind::Service, Some(&ent), weights[tier], self.own());
                    service.critical = tier == 0;
                    nodes.push(service);
                    for w in 0..self.pick(3) {
                        let mut workload = node(
                            &format!("wl:ns/deployment/f-{e}-{s}-{w}"),
                            Kind::Workload,
                            Some(&svc),
                            weights[tier],
                            self.own(),
                        );
                        workload.critical = tier == 0;
                        nodes.push(workload);
                    }
                }
            }
            nodes
        }

        fn own(&mut self) -> Option<f64> {
            if self.next() < 0.25 {
                return None;
            }
            Some(1.0 + 4.0 * self.next())
        }
    }

    #[test]
    fn property_an_added_unknown_child_never_raises_an_index() {
        let mut rng = Lcg(7);
        for _ in 0..300 {
            let mut nodes = rng.tree();
            let before = rolled(&nodes);
            let parent = nodes[rng.pick(nodes.len())].clone();
            if parent.kind == Kind::Workload {
                continue;
            }
            let child_kind = match parent.kind {
                Kind::Organisation | Kind::Org => Kind::Entity,
                Kind::Entity => Kind::Service,
                _ => Kind::Workload,
            };
            let mut child = node("zz:added", child_kind, Some(&parent.id), 1.5, None);
            if parent.kind.is_org() {
                child.weight = 3.0;
            } else {
                child.weight = parent.weight;
                child.critical = parent.critical;
            }
            nodes.push(child);
            let after = rolled(&nodes);
            assert!(after["zz:added"].unknown);
            for (id, was) in &before {
                let is = &after[id];
                assert!(
                    is.index <= was.index + 1e-9,
                    "{id}: {} > {}",
                    is.index,
                    was.index
                );
                assert!(is.level <= was.level, "{id}");
            }
        }
    }

    #[test]
    fn property_a_parent_is_never_above_a_critical_child() {
        let mut rng = Lcg(11);
        for _ in 0..300 {
            let by_id = rolled(&rng.tree());
            for child in by_id.values().filter(|n| n.critical) {
                let Some(parent) = child.parent.as_deref() else {
                    continue;
                };
                assert!(
                    by_id[parent].level <= child.level,
                    "{parent} above {}",
                    child.id
                );
            }
        }
    }

    #[test]
    fn property_the_output_does_not_depend_on_node_order() {
        let mut rng = Lcg(13);
        for _ in 0..100 {
            let nodes = rng.tree();
            let expected = format!(
                "{:?}",
                roll_up(&nodes, "org:bank", 3, &HouseParams::default())
            );
            let mut shuffled = nodes.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, rng.pick(i + 1));
            }
            let got = format!(
                "{:?}",
                roll_up(&shuffled, "org:bank", 3, &HouseParams::default())
            );
            assert_eq!(got, expected);
            shuffled.reverse();
            let got = format!(
                "{:?}",
                roll_up(&shuffled, "org:bank", 3, &HouseParams::default())
            );
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn property_a_cap_origin_has_no_capping_child_of_its_own() {
        let mut rng = Lcg(17);
        let mut capped = 0;
        for _ in 0..300 {
            let by_id = rolled(&rng.tree());
            for n in by_id.values() {
                if n.cap.rule != CapRule::WeakestCriticalChild {
                    assert!(n.cap.origin.is_empty() && n.cap.node_ids.is_empty());
                    continue;
                }
                capped += 1;
                assert!(!n.cap.node_ids.is_empty());
                for origin in &n.cap.origin {
                    assert_ne!(by_id[origin].cap.rule, CapRule::WeakestCriticalChild);
                }
                for direct in &n.cap.node_ids {
                    assert_eq!(by_id[direct].parent.as_deref(), Some(n.id.as_str()));
                    assert!(by_id[direct].critical);
                }
            }
        }
        assert!(capped > 100, "the generator must produce capped nodes");
    }

    fn spine(inputs: &Inputs) -> BTreeMap<String, NodeScore> {
        spine_with(inputs, &HouseParams::default())
    }

    fn spine_with(inputs: &Inputs, params: &HouseParams) -> BTreeMap<String, NodeScore> {
        let matrix = Matrix::build(inputs, params);
        let entity = maturity::assess(&matrix, Scope::Entity);
        assess(&matrix, &entity)
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect()
    }

    #[test]
    fn the_spine_of_the_small_firm_hangs_services_under_their_functions() {
        let by_id = spine(&firm_b());
        let kinds: Vec<(&str, &str)> = by_id
            .values()
            .map(|n| (n.id.as_str(), n.kind.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("ent:cif-0001", "entity"),
                ("ent:cif-0002", "entity"),
                ("ent:ops-0003", "entity"),
                ("org:", "organisation"),
                ("svc:intranet", "service"),
                ("svc:payments-api", "service"),
                ("svc:payments-ledger", "service"),
                ("svc:reporting-api", "service"),
                ("svc:reporting-batch", "service"),
            ]
        );
        let api = &by_id["svc:payments-api"];
        assert_eq!(api.parent.as_deref(), Some("ent:cif-0001"));
        assert_eq!((api.weight, api.critical), (3.0, true));
        // C = 100 at the simplified ceiling, T = 0 (no run targeted the
        // service), S not declared: 1 + 4 · 0.20 · 100 / (100 · 0.50).
        let letters: Vec<&str> = api.pillars.iter().map(|(letter, _)| *letter).collect();
        assert_eq!(letters, ["C", "T", "S"]);
        assert_eq!(round2(api.own.unwrap()), 2.6);
        assert_eq!(by_id["ent:cif-0001"].children, 2);
        assert_eq!(by_id["org:"].target, 3);
        assert_eq!(by_id["org:"].children, 3);
    }

    #[test]
    fn an_uninstrumented_critical_service_caps_the_organisation_until_it_reports() {
        let mut inputs = firm_b();
        inputs
            .services
            .push(Service::silent("payments-db", &["cif-0001"]));
        let by_id = spine(&inputs);
        assert!(by_id["svc:payments-db"].unknown);
        assert_eq!(
            by_id["svc:payments-db"].note.as_deref(),
            Some("no telemetry and no test")
        );
        let org = &by_id["org:"];
        assert_eq!(org.level, 1);
        assert_eq!(org.cap.rule, CapRule::WeakestCriticalChild);
        assert_eq!(org.cap.node_ids, ids(&["ent:cif-0001"]));
        assert_eq!(org.cap.origin, ids(&["svc:payments-db"]));
        assert_eq!(org.cap.origin_state, Some("unknown"));
        assert!(org.unknown_share > 0.0);

        let last = inputs.services.len() - 1;
        inputs.services[last] = Service::observed("payments-db", 2, &["cif-0001"], inputs.as_of_ns);
        let by_id = spine(&inputs);
        let org = &by_id["org:"];
        assert!(!by_id["svc:payments-db"].unknown);
        assert_eq!(org.level, 2);
        assert!(!org.cap.origin.contains(&"svc:payments-db".to_string()));
        assert_eq!(org.unknown_share, 0.0);
    }

    /// The floor is the model's, not the org file's: a parameter that
    /// got past validation still gives a silent service nothing.
    #[test]
    fn an_unknown_node_is_index_1_and_level_1_whatever_the_floor_parameter_says() {
        let mut inputs = firm_b();
        let known = spine(&inputs)["org:"].index;
        for name in ["archive", "batch", "mirror"] {
            inputs.services.push(Service::silent(name, &["cif-0002"]));
        }
        let default = spine(&inputs);
        assert!(default["org:"].index < known);
        let mut params = HouseParams::default();
        params.rollup.unknown_floor = 5.0;
        let raised = spine_with(&inputs, &params);
        for (id, node) in &raised {
            assert_eq!(node.index, default[id].index, "{id}");
            assert_eq!(node.level, default[id].level, "{id}");
            if node.unknown {
                assert_eq!((node.index, node.level), (1.0, 1), "{id}");
            }
        }
        assert_eq!(raised["svc:archive"].index, 1.0);
    }

    #[test]
    fn an_unknown_node_is_level_1_even_where_no_gate_is_evaluated() {
        let mut params = HouseParams::default();
        params.rollup.unknown_floor = 5.0;
        let nodes = [
            node("org:bank", Kind::Organisation, None, 0.0, None),
            node("org:bank/empty", Kind::Org, Some("org:bank"), 0.0, None),
        ];
        for scored in roll_up(&nodes, "org:bank", 3, &params) {
            assert!(scored.unknown);
            assert_eq!((scored.index, scored.level), (1.0, 1), "{}", scored.id);
        }
    }

    #[test]
    fn a_service_without_a_function_lowers_the_organisation_from_the_unassigned_node() {
        let mut inputs = firm_b();
        let before = spine(&inputs)["org:"].index;
        inputs.services.push(Service::silent("stray", &[]));
        let by_id = spine(&inputs);
        let unassigned = &by_id[UNASSIGNED_ID];
        assert_eq!(by_id["svc:stray"].parent.as_deref(), Some(UNASSIGNED_ID));
        assert_eq!((unassigned.weight, unassigned.unknown), (1.0, true));
        assert!(by_id["org:"].index < before);
        assert!(by_id["org:"].drag.contains(&UNASSIGNED_ID.to_string()));
    }

    #[test]
    fn systems_components_and_workloads_join_the_spine() {
        let mut inputs = firm_b();
        inputs.functions[0].org_unit = Some("firm/payments".into());
        inputs.services[0].env = Some("prod".into());
        inputs.services[0].system = Some("core".into());
        inputs.services[0].component = Some("api".into());
        inputs.evidence[0].env = Some("prod".into());
        inputs.evidence[0].system = Some("core".into());
        inputs.workloads = vec![
            Workload {
                id: "wl:pay/deployment/api".into(),
                service: Some("payments-api".into()),
            },
            Workload {
                id: "wl:pay/deployment/orphan".into(),
                service: None,
            },
        ];
        let by_id = spine(&inputs);
        let parent = |id: &str| by_id[id].parent.clone().unwrap();
        assert_eq!(parent("wl:pay/deployment/api"), "svc:payments-api");
        assert_eq!(parent("svc:payments-api"), "cmp:prod/core/api");
        assert_eq!(parent("cmp:prod/core/api"), "sys:prod/core");
        assert_eq!(parent("sys:prod/core"), "ent:cif-0001");
        assert_eq!(parent("ent:cif-0001"), "org:firm/payments");
        assert_eq!(parent("org:firm/payments"), "org:firm");
        assert_eq!(parent("org:firm"), "org:");
        assert_eq!(parent("wl:pay/deployment/orphan"), UNASSIGNED_ID);
        assert!(by_id["wl:pay/deployment/orphan"].unknown);
        // The restore run names the system: T from the dr discipline
        // inside robustness (3 of 10), R met.
        let system = &by_id["sys:prod/core"];
        let own: Vec<(&str, Option<f64>)> = system
            .pillars
            .iter()
            .map(|(letter, pillar)| (*letter, pillar.value()))
            .collect();
        assert_eq!(own.len(), 2);
        assert_eq!(own[0].0, "T");
        assert!((own[0].1.unwrap() - 30.0).abs() < 1e-9);
        assert_eq!(own[1], ("R", Some(100.0)));
        // No run named the component: it has no own score and rolls up.
        assert_eq!(by_id["cmp:prod/core/api"].own, None);
        assert!(!by_id["cmp:prod/core/api"].unknown);
    }

    #[test]
    fn the_bank_spine_is_fully_known_and_carries_the_entity_gates() {
        let inputs = bank_a();
        let by_id = spine(&inputs);
        let org = &by_id["org:"];
        assert_eq!(org.unknown_share, 0.0);
        assert_eq!(org.children, 8);
        assert_eq!(org.weight, 18.0);
        assert!(org.level <= 4);
        for entity in by_id.values().filter(|n| n.kind == Kind::Entity) {
            assert!(entity.own.is_some(), "{}", entity.id);
            let letters: Vec<&str> = entity.pillars.iter().map(|(l, _)| *l).collect();
            assert_eq!(letters, ["T", "S", "R", "L"], "{}", entity.id);
        }
    }
}
