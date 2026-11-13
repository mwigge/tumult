//! Turns input rows into nodes and typed edges, and records the facts
//! the spine and overlay passes need. Every choice between candidates is
//! made on sorted keys, never on row order.

use std::collections::{BTreeMap, BTreeSet};

use super::graph::Graph;
use super::input::{CallRow, RunRow, ServiceRow, StarInputs, SystemRow, WorkloadRow};
use super::model::{EdgeSource, Gap, Posture, StarKind, StarNode, StarRel};

/// Id of the organisation root.
pub(super) const ROOT: &str = "org:";
/// Id of the unit that holds everything nobody owns.
pub(super) const UNASSIGNED: &str = "org:(unassigned)";
/// Path of the unassigned unit in `org.yaml` terms.
const UNASSIGNED_PATH: &str = "(unassigned)";

/// What one service reported about where it belongs.
#[derive(Debug, Default)]
pub(super) struct SvcLinks {
    pub components: BTreeSet<String>,
    pub systems: BTreeSet<String>,
    pub entities: BTreeSet<String>,
}

/// Relations learned while building that are not edges themselves.
#[derive(Debug, Default)]
pub(super) struct Facts {
    /// Entity id to the `ict.entity.owner` values reported for it.
    pub ent_owners: BTreeMap<String, BTreeSet<String>>,
    pub ent_systems: BTreeMap<String, BTreeSet<String>>,
    pub sys_entities: BTreeMap<String, BTreeSet<String>>,
    /// System id to the names of the experiments run against it.
    pub sys_experiments: BTreeMap<String, BTreeSet<String>>,
    pub sys_by_name: BTreeMap<String, BTreeSet<String>>,
    /// Systems the coverage matrix has a row for.
    pub sys_rows: BTreeSet<String>,
    pub svc_links: BTreeMap<String, SvcLinks>,
    /// `(namespace, deployment)` to the services reporting it.
    pub svc_k8s: BTreeMap<(String, String), BTreeSet<String>>,
    /// Experiment name to definition node id, for registered definitions.
    pub def_ids: BTreeMap<String, String>,
    /// Org unit name to node id; the first path in sorted order wins.
    pub org_names: BTreeMap<String, String>,
}

pub(super) struct Builder<'a> {
    pub inputs: &'a StarInputs,
    pub scope: Option<&'a [String]>,
    pub include_runs: bool,
    pub graph: Graph,
    pub facts: Facts,
}

/// Whether `env` is visible to the caller.
pub(super) fn in_scope(scope: Option<&[String]>, env: Option<&str>) -> bool {
    let Some(allowed) = scope else {
        return true;
    };
    env.is_some_and(|e| allowed.iter().any(|a| a == e))
}

/// Rank of a criticality or tier label, in tenths of the layout weight.
pub(super) fn tier_rank(level: Option<&str>) -> u8 {
    match level.map(str::to_ascii_lowercase).as_deref() {
        Some("critical") => 30,
        Some("high" | "important") => 20,
        Some("medium") => 15,
        _ => 10,
    }
}

/// Keep the highest tier seen; ties go to the smaller label.
fn raise_tier(node: &mut StarNode, tier: Option<&str>) {
    let Some(tier) = tier.map(str::to_ascii_lowercase) else {
        return;
    };
    let new = tier_rank(Some(&tier));
    let current = tier_rank(node.tier.as_deref());
    let better = match node.tier.as_deref() {
        None => true,
        Some(held) => new > current || (new == current && tier.as_str() < held),
    };
    if !better {
        return;
    }
    node.criticality_weight = f64::from(new) / 10.0;
    node.tier = Some(tier);
}

fn keep_min(slot: &mut Option<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if slot.as_deref().is_none_or(|held| value < held) {
        *slot = Some(value.to_string());
    }
}

pub(super) fn keep_max(slot: &mut Option<i64>, value: i64) {
    if slot.is_none_or(|held| value > held) {
        *slot = Some(value);
    }
}

pub(super) fn sys_id(env: Option<&str>, system: &str) -> String {
    format!("sys:{}/{system}", env.unwrap_or("-"))
}

fn org_id(path: &str) -> String {
    format!("org:{path}")
}

fn org_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((head, _)) => org_id(head),
        None => ROOT.to_string(),
    }
}

/// Coverage state of one system from its discipline statuses.
fn coverage_gap(statuses: &[(String, String)]) -> Option<Gap> {
    if statuses.iter().any(|(_, s)| s == "covered") {
        return None;
    }
    if statuses.iter().any(|(_, s)| s == "stale") {
        return Some(Gap::Stale);
    }
    Some(Gap::Missing)
}

impl<'a> Builder<'a> {
    pub fn new(inputs: &'a StarInputs, scope: Option<&'a [String]>, include_runs: bool) -> Self {
        Self {
            inputs,
            scope,
            include_runs,
            graph: Graph::default(),
            facts: Facts::default(),
        }
    }

    /// Build every node and typed edge the inputs describe.
    pub fn build(&mut self) {
        self.add_org();
        let mut services: Vec<&ServiceRow> = self.inputs.services.iter().collect();
        services.sort_by(|a, b| (&a.service, &a.entity_id).cmp(&(&b.service, &b.entity_id)));
        for row in services {
            self.add_service(row);
        }
        let mut systems: Vec<&SystemRow> = self.inputs.systems.iter().collect();
        systems.sort_by(|a, b| (&a.system, &a.environment).cmp(&(&b.system, &b.environment)));
        for row in systems {
            self.add_system(row);
        }
        self.add_definitions();
        self.add_runs();
        let mut workloads: Vec<&WorkloadRow> = self.inputs.workloads.iter().collect();
        workloads.sort_by(|a, b| {
            (&a.namespace, &a.kind, &a.name).cmp(&(&b.namespace, &b.kind, &b.name))
        });
        for row in workloads {
            self.add_workload(row);
        }
        self.add_calls();
        self.add_depends();
        self.add_covers();
        self.add_gaps();
    }

    fn add_org(&mut self) {
        let root = self.graph.ensure(ROOT, StarKind::Org, "(company)", false);
        root.org_kind = Some("company".to_string());
        let mut units: Vec<_> = self.inputs.org_units.iter().collect();
        units.sort_by(|a, b| a.path.cmp(&b.path));
        for unit in units {
            if unit.path == UNASSIGNED_PATH {
                continue;
            }
            if unit.path.is_empty() {
                self.name_root(&unit.name, &unit.kind);
                continue;
            }
            self.add_org_unit(&unit.path, &unit.name, &unit.kind);
        }
    }

    fn name_root(&mut self, name: &str, kind: &str) {
        let root = self.graph.ensure(ROOT, StarKind::Org, "(company)", false);
        if !name.is_empty() {
            root.label = name.to_string();
        }
        if !kind.is_empty() {
            root.org_kind = Some(kind.to_string());
        }
    }

    fn add_org_unit(&mut self, path: &str, name: &str, kind: &str) {
        let id = org_id(path);
        let parent = org_parent(path);
        let node = self.graph.ensure(&id, StarKind::Org, name, false);
        node.label = name.to_string();
        node.org_kind = Some(kind.to_string());
        node.parent = Some(parent.clone());
        self.facts
            .org_names
            .entry(name.to_string())
            .or_insert_with(|| id.clone());
        self.ensure_org_chain(&parent);
    }

    /// A unit whose ancestors are missing from the file still needs them.
    fn ensure_org_chain(&mut self, id: &str) {
        let mut current = id.to_string();
        while !self.graph.has(&current) {
            let path = current.trim_start_matches("org:").to_string();
            let label = path.rsplit('/').next().unwrap_or_default().to_string();
            let parent = org_parent(&path);
            let node = self.graph.ensure(&current, StarKind::Org, &label, false);
            node.org_kind = Some("unit".to_string());
            node.parent = Some(parent.clone());
            current = parent;
        }
    }

    fn ensure_entity(
        &mut self,
        raw: &str,
        tier: Option<&str>,
        owner: Option<&str>,
        declared: bool,
    ) -> String {
        let key = raw.trim().to_lowercase();
        let id = format!("ent:{key}");
        let node = self.graph.ensure(&id, StarKind::Entity, &key, declared);
        raise_tier(node, tier);
        if let Some(owner) = owner {
            self.facts
                .ent_owners
                .entry(id.clone())
                .or_default()
                .insert(owner.to_string());
        }
        id
    }

    fn ensure_system(&mut self, env: Option<&str>, name: &str, declared: bool) -> String {
        let id = sys_id(env, name);
        let node = self.graph.ensure(&id, StarKind::System, name, declared);
        node.env = env.map(str::to_string);
        self.facts
            .sys_by_name
            .entry(name.to_string())
            .or_default()
            .insert(id.clone());
        id
    }

    fn ensure_component(&mut self, system: &str, component: &str, declared: bool) -> String {
        let key = system.strip_prefix("sys:").unwrap_or(system);
        let id = format!("cmp:{key}/{component}");
        let env = self.graph.nodes.get(system).and_then(|n| n.env.clone());
        let node = self
            .graph
            .ensure(&id, StarKind::Component, component, declared);
        node.env = env;
        node.parent = Some(system.to_string());
        id
    }

    fn entity_implements(&mut self, entity: &str, system: &str, source: EdgeSource) {
        self.graph.link(entity, StarRel::Implements, system, source);
        self.facts
            .ent_systems
            .entry(entity.to_string())
            .or_default()
            .insert(system.to_string());
        self.facts
            .sys_entities
            .entry(system.to_string())
            .or_default()
            .insert(entity.to_string());
    }

    fn add_service(&mut self, row: &ServiceRow) {
        let env = row.environment.as_deref();
        if !in_scope(self.scope, env) {
            return;
        }
        let svc = format!("svc:{}", row.service);
        let node = self
            .graph
            .ensure(&svc, StarKind::Service, &row.service, false);
        keep_min(&mut node.env, env);
        keep_min(&mut node.conformance, row.conformance.as_deref());
        keep_max(&mut node.last_seen_ns, row.last_seen_ns);
        raise_tier(node, row.entity_tier.as_deref());

        let entity = row.entity_id.as_deref().map(|raw| {
            self.ensure_entity(
                raw,
                row.entity_tier.as_deref(),
                row.entity_owner.as_deref(),
                false,
            )
        });
        let system = row
            .system
            .as_deref()
            .map(|name| self.ensure_system(env, name, false));
        let component = match (&system, row.component.as_deref()) {
            (Some(system), Some(component)) => {
                Some(self.ensure_component(system, component, false))
            }
            _ => None,
        };
        if let Some(system) = &system {
            if let Some(node) = self.graph.nodes.get_mut(system) {
                keep_max(&mut node.last_seen_ns, row.last_seen_ns);
            }
        }
        self.link_service(
            &svc,
            entity.as_deref(),
            system.as_deref(),
            component.as_deref(),
        );
        if let (Some(ns), Some(deployment)) = (&row.k8s_namespace, &row.k8s_deployment) {
            self.facts
                .svc_k8s
                .entry((ns.clone(), deployment.clone()))
                .or_default()
                .insert(svc);
        }
    }

    fn link_service(
        &mut self,
        svc: &str,
        entity: Option<&str>,
        system: Option<&str>,
        component: Option<&str>,
    ) {
        let telemetry = EdgeSource::Telemetry;
        if let Some(entity) = entity {
            self.graph.link(svc, StarRel::Implements, entity, telemetry);
        }
        if let (Some(entity), Some(system)) = (entity, system) {
            self.entity_implements(entity, system, telemetry);
        }
        if let (Some(entity), Some(component)) = (entity, component) {
            self.graph
                .link(component, StarRel::Implements, entity, telemetry);
        }
        let links = self.facts.svc_links.entry(svc.to_string()).or_default();
        links.entities.extend(entity.map(str::to_string));
        links.systems.extend(system.map(str::to_string));
        links.components.extend(component.map(str::to_string));
    }

    fn add_system(&mut self, row: &SystemRow) {
        let env = row.environment.as_deref();
        if !in_scope(self.scope, env) {
            return;
        }
        let id = self.ensure_system(env, &row.system, false);
        let node = self.graph.ensure(&id, StarKind::System, &row.system, false);
        raise_tier(node, row.criticality.as_deref());
        node.posture = row.posture.as_ref().map(|p| Posture {
            score: p.score,
            band: p.band.clone(),
        });
        node.evidence.by_discipline = row.disciplines.iter().cloned().collect();
        node.gap = coverage_gap(&row.disciplines);
        self.facts.sys_rows.insert(id.clone());
        let names = self.facts.sys_experiments.entry(id).or_default();
        names.extend(row.experiments.iter().map(|e| e.name.clone()));
    }

    fn add_definitions(&mut self) {
        let mut rows: Vec<_> = self.inputs.definitions.iter().collect();
        rows.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        for row in rows {
            let Some(registry_id) = &row.id else {
                continue;
            };
            let id = format!("def:{registry_id}");
            self.facts
                .def_ids
                .entry(row.name.clone())
                .or_insert_with(|| id.clone());
            self.graph.ensure(&id, StarKind::Def, &row.name, false);
        }
        // Unregistered definitions exist only when the caller may see
        // everything; scoped callers learn of them through their systems.
        if self.scope.is_some() {
            return;
        }
        for row in self.inputs.definitions.iter().filter(|r| r.id.is_none()) {
            self.def_node(&row.name);
        }
    }

    /// The definition node for an experiment name, created on first use.
    pub fn def_node(&mut self, name: &str) -> String {
        let id = match self.facts.def_ids.get(name) {
            Some(id) => id.clone(),
            None => format!("def:{name}"),
        };
        self.graph.ensure(&id, StarKind::Def, name, false);
        id
    }

    fn visible_runs(&self) -> Vec<&'a RunRow> {
        let mut runs: Vec<&RunRow> = self
            .inputs
            .runs
            .iter()
            .filter(|r| in_scope(self.scope, r.environment.as_deref()))
            .collect();
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        runs
    }

    /// Runs become evidence counts, `tested_by` and `broke` edges, and
    /// with `include=runs` nodes of their own.
    fn add_runs(&mut self) {
        let runs = self.visible_runs();
        let mut latest = self.latest_from_rows();
        let mut in_window: BTreeSet<(String, String)> = BTreeSet::new();
        for run in &runs {
            let system = run
                .system
                .as_deref()
                .map(|name| self.ensure_system(run.environment.as_deref(), name, false));
            self.count_run(run, system.as_deref());
            let (Some(system), false) = (system, run.manual) else {
                continue;
            };
            let key = (system, run.experiment.clone());
            let seen = latest.entry(key.clone()).or_insert((i64::MIN, false));
            *seen = (*seen).max((run.started_ns, run.failed));
            in_window.insert(key);
        }
        for ((system, experiment), (_, failed)) in &latest {
            let def = self.def_node(experiment);
            self.facts
                .sys_experiments
                .entry(system.clone())
                .or_default()
                .insert(experiment.clone());
            if self.include_runs && in_window.contains(&(system.clone(), experiment.clone())) {
                continue;
            }
            self.graph
                .link(system, StarRel::TestedBy, &def, EdgeSource::Evidence);
            if *failed {
                self.graph
                    .link(&def, StarRel::Broke, system, EdgeSource::Evidence);
            }
        }
        if self.include_runs {
            for run in runs {
                self.add_run_node(run);
            }
        }
    }

    /// Latest known outcome per `(system, experiment)` from the coverage
    /// rows, which look further back than the window.
    fn latest_from_rows(&self) -> BTreeMap<(String, String), (i64, bool)> {
        let mut latest = BTreeMap::new();
        for row in &self.inputs.systems {
            let env = row.environment.as_deref();
            if !in_scope(self.scope, env) {
                continue;
            }
            for experiment in &row.experiments {
                let key = (sys_id(env, &row.system), experiment.name.clone());
                let seen = latest.entry(key).or_insert((i64::MIN, false));
                *seen = (*seen).max((experiment.last_run_ns, !experiment.passed));
            }
        }
        latest
    }

    fn count_run(&mut self, run: &RunRow, system: Option<&str>) {
        let mut targets: Vec<String> = system.map(str::to_string).into_iter().collect();
        if !run.manual {
            targets.push(self.def_node(&run.experiment));
        }
        for id in targets {
            let Some(node) = self.graph.nodes.get_mut(&id) else {
                continue;
            };
            if run.manual {
                node.evidence.manual += 1;
            } else {
                node.evidence.runs += 1;
            }
            node.evidence.failed += u64::from(run.failed);
            keep_max(&mut node.evidence.last_run_ns, run.started_ns);
        }
    }

    fn add_run_node(&mut self, run: &RunRow) {
        let prefix = if run.manual { "man" } else { "run" };
        let id = format!("{prefix}:{}", run.id);
        let node = self
            .graph
            .ensure(&id, StarKind::Run, &run.experiment, false);
        node.env.clone_from(&run.environment);
        node.last_seen_ns = Some(run.started_ns);
        node.evidence.runs = u64::from(!run.manual);
        node.evidence.manual = u64::from(run.manual);
        node.evidence.failed = u64::from(run.failed);
        node.evidence.last_run_ns = Some(run.started_ns);
        let evidence = EdgeSource::Evidence;
        if !run.manual {
            let def = self.def_node(&run.experiment);
            self.graph.link(&id, StarRel::TestedBy, &def, evidence);
        }
        let Some(system) = run.system.as_deref() else {
            return;
        };
        let system = sys_id(run.environment.as_deref(), system);
        self.graph.link(&system, StarRel::TestedBy, &id, evidence);
        if run.failed {
            self.graph.link(&id, StarRel::Broke, &system, evidence);
        }
    }

    fn systems_named(&self, name: &str, env: Option<&str>) -> Vec<String> {
        let Some(ids) = self.facts.sys_by_name.get(name) else {
            return Vec::new();
        };
        ids.iter()
            .filter(|id| {
                let node_env = self.graph.nodes.get(*id).and_then(|n| n.env.as_deref());
                env.is_none() || node_env == env
            })
            .cloned()
            .collect()
    }

    /// Systems a workload belongs to: the labelled one (created when no
    /// telemetry or evidence knows it), else a system sharing its name
    /// or `app` label.
    fn workload_systems(
        &mut self,
        row: &WorkloadRow,
        env: Option<&str>,
        app: Option<&str>,
    ) -> Vec<String> {
        if let Some(name) = label(row, "resilience.target.system") {
            let found = self.systems_named(name, env);
            if !found.is_empty() {
                return found;
            }
            return vec![self.ensure_system(env, name, true)];
        }
        let by_name = self.systems_named(&row.name, env);
        if !by_name.is_empty() {
            return by_name;
        }
        app.map(|app| self.systems_named(app, env))
            .unwrap_or_default()
    }

    /// Services running on a workload: by k8s resource attributes, else
    /// by the `app` label naming the service.
    fn workload_services(&self, row: &WorkloadRow, app: Option<&str>) -> (Vec<String>, EdgeSource) {
        let key = (row.namespace.clone(), row.name.clone());
        if let Some(found) = self.facts.svc_k8s.get(&key) {
            return (found.iter().cloned().collect(), EdgeSource::Telemetry);
        }
        let by_app = app
            .map(|app| format!("svc:{app}"))
            .filter(|id| self.graph.has(id));
        (by_app.into_iter().collect(), EdgeSource::Declared)
    }

    fn add_workload(&mut self, row: &WorkloadRow) {
        let env_label = label(row, "resilience.target.environment");
        let app = label(row, "app").or_else(|| label(row, "app.kubernetes.io/name"));
        let systems = self.workload_systems(row, env_label, app);
        let components: Vec<String> = match label(row, "resilience.target.component") {
            Some(component) => systems
                .iter()
                .map(|system| self.ensure_component(system, component, true))
                .collect(),
            None => Vec::new(),
        };
        let (services, service_source) = self.workload_services(row, app);
        if let Some(entity) = label(row, "ict.entity.id") {
            self.declare_entity(row, entity, [&systems, &components, &services]);
        }
        let (hosts, source) = if !services.is_empty() {
            (services, service_source)
        } else if !components.is_empty() {
            (components, EdgeSource::Declared)
        } else {
            (systems, EdgeSource::Declared)
        };
        let host_env = hosts
            .iter()
            .filter_map(|h| self.graph.nodes.get(h).and_then(|n| n.env.clone()))
            .min();
        let id = format!(
            "wl:{}/{}/{}",
            row.namespace,
            row.kind.to_lowercase(),
            row.name
        );
        let node = self.graph.ensure(&id, StarKind::Workload, &row.name, false);
        node.env = env_label.map(str::to_string).or(host_env);
        node.parent = Some(hosts.first().cloned().unwrap_or(UNASSIGNED.to_string()));
        for host in &hosts {
            self.graph.link(host, StarRel::RunsOn, &id, source);
        }
    }

    /// An `ict.entity.id` label declares the entity behind the workload's
    /// systems, components and services.
    fn declare_entity(&mut self, row: &WorkloadRow, raw: &str, linked: [&Vec<String>; 3]) {
        let [systems, components, services] = linked;
        let entity = self.ensure_entity(
            raw,
            label(row, "ict.entity.tier"),
            label(row, "ict.entity.owner"),
            true,
        );
        let declared = EdgeSource::Declared;
        for system in systems {
            self.entity_implements(&entity, system, declared);
        }
        for from in components.iter().chain(services) {
            self.graph
                .link(from, StarRel::Implements, &entity, declared);
        }
        for service in services {
            let links = self.facts.svc_links.entry(service.clone()).or_default();
            links.entities.insert(entity.clone());
        }
    }

    fn add_calls(&mut self) {
        let mut calls: Vec<&CallRow> = self.inputs.calls.iter().collect();
        calls.sort_by(|a, b| {
            (&a.client, &a.server, a.calls, a.errors)
                .cmp(&(&b.client, &b.server, b.calls, b.errors))
        });
        for call in calls {
            let from = format!("svc:{}", call.client);
            let to = format!("svc:{}", call.server);
            if !self.graph.has(&from) || !self.graph.has(&to) {
                continue;
            }
            let edge = self
                .graph
                .link(&from, StarRel::Calls, &to, EdgeSource::Observed);
            super::cluster::add_traffic(
                edge,
                Some(call.calls),
                Some(call.errors),
                Some(call.avg_duration_ns),
            );
        }
    }

    fn add_depends(&mut self) {
        let mut rows: Vec<_> = self.inputs.depends.iter().collect();
        rows.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        for row in rows {
            let from = format!("svc:{}", row.from);
            let to = format!("svc:{}", row.to);
            self.graph.ensure(&from, StarKind::Service, &row.from, true);
            self.graph.ensure(&to, StarKind::Service, &row.to, true);
            self.graph
                .link(&from, StarRel::DependsOn, &to, EdgeSource::Declared);
        }
    }

    fn add_covers(&mut self) {
        let labels: BTreeMap<&str, &str> = self
            .inputs
            .articles
            .iter()
            .map(|a| (a.id.as_str(), a.label.as_str()))
            .collect();
        for cover in &self.inputs.covers {
            let from = match &cover.from {
                super::input::CoverFrom::Experiment(name) => match self.facts.def_ids.get(name) {
                    Some(id) => id.clone(),
                    None => format!("def:{name}"),
                },
                super::input::CoverFrom::Manual(id) => format!("man:{id}"),
            };
            if !self.graph.has(&from) {
                continue;
            }
            let label = labels.get(cover.article.as_str()).copied();
            let label = label.unwrap_or(cover.article.as_str());
            self.graph
                .ensure(&cover.article, StarKind::Article, label, false);
            let source = if cover.evidenced {
                EdgeSource::Evidence
            } else {
                EdgeSource::Declared
            };
            self.graph
                .link(&from, StarRel::Covers, &cover.article, source);
        }
    }

    fn add_gaps(&mut self) {
        for gap in &self.inputs.gaps {
            let node = self.graph.ensure(&gap.id, StarKind::Gap, &gap.label, false);
            node.gap = Some(Gap::Missing);
        }
    }
}

fn label<'r>(row: &'r WorkloadRow, key: &str) -> Option<&'r str> {
    row.labels
        .get(key)
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}
