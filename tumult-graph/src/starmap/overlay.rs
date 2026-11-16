//! Overlay blocks: risks, gaps and the roll-up of evidence from systems
//! to the entities they implement.

use std::collections::BTreeSet;

use super::build::{def_id, in_scope, keep_max, svc_id, sys_id, Builder};
use super::ids;
use super::input::{RiskRow, RiskSubject};
use super::model::{Evidence, Gap, RiskSummary, StarKind, StarRel};

fn severity_rank(severity: &str) -> u8 {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn discipline_rank(status: &str) -> u8 {
    match status {
        "covered" => 2,
        "stale" => 1,
        _ => 0,
    }
}

fn note_worst(summary: &mut RiskSummary, severity: &str) {
    let candidate = (severity_rank(severity), severity);
    let held = summary.worst.as_deref().map(|w| (severity_rank(w), w));
    if held.is_none_or(|held| candidate > held) {
        summary.worst = Some(severity.to_string());
    }
}

/// Keep the suppression that expires first; one with no expiry sorts last.
fn note_suppression(summary: &mut RiskSummary, expires: Option<i64>, actor: &str, reason: &str) {
    let candidate = (expires.unwrap_or(i64::MAX), actor, reason);
    let held = summary.suppressed_by.as_deref().map(|by| {
        let reason = summary.suppression_reason.as_deref().unwrap_or_default();
        (
            summary.suppression_expires_ns.unwrap_or(i64::MAX),
            by,
            reason,
        )
    });
    if held.is_some_and(|held| candidate >= held) {
        return;
    }
    summary.suppression_expires_ns = expires;
    summary.suppressed_by = Some(actor.to_string());
    summary.suppression_reason = Some(reason.to_string());
}

fn note_recommended(summary: &mut RiskSummary, kind: &str) {
    if let Err(at) = summary
        .recommended
        .binary_search_by(|k| k.as_str().cmp(kind))
    {
        summary.recommended.insert(at, kind.to_string());
    }
}

fn add_risk(summary: &mut RiskSummary, risk: &RiskRow) {
    let Some(suppression) = &risk.suppression else {
        summary.open += 1;
        note_worst(summary, &risk.severity);
        if let Some(kind) = &risk.recommended_kind {
            note_recommended(summary, kind);
        }
        return;
    };
    summary.suppressed += 1;
    note_suppression(
        summary,
        suppression.expires_at_ns,
        &suppression.actor,
        &suppression.reason,
    );
}

/// Fold one node's risks into an aggregate (an entity or a cluster).
pub(super) fn merge_risks(into: &mut RiskSummary, from: &RiskSummary) {
    into.open += from.open;
    into.suppressed += from.suppressed;
    if let Some(worst) = &from.worst {
        note_worst(into, worst);
    }
    if let Some(by) = &from.suppressed_by {
        let reason = from.suppression_reason.as_deref().unwrap_or_default();
        note_suppression(into, from.suppression_expires_ns, by, reason);
    }
    for kind in &from.recommended {
        note_recommended(into, kind);
    }
}

fn merge_evidence(into: &mut Evidence, from: &Evidence) {
    into.runs += from.runs;
    into.failed += from.failed;
    into.manual += from.manual;
    if let Some(last) = from.last_run_ns {
        keep_max(&mut into.last_run_ns, last);
    }
    for (discipline, status) in &from.by_discipline {
        let held = into.by_discipline.get(discipline);
        if held.is_none_or(|h| discipline_rank(status) > discipline_rank(h)) {
            into.by_discipline
                .insert(discipline.clone(), status.clone());
        }
    }
}

impl Builder<'_> {
    /// Attach risks and gaps, then roll systems up to their entities.
    pub fn attach_overlays(&mut self) {
        self.attach_risks();
        self.mark_untested_systems();
        self.roll_up_entities();
        self.mark_untested_callees();
    }

    /// Attach every visible risk to its node. One that has no node is
    /// counted, never dropped silently, when it is the caller's to count.
    fn attach_risks(&mut self) {
        for risk in &self.inputs.risks {
            let env = risk.environment.as_deref();
            if env.is_some() && !in_scope(self.scope, env) {
                continue;
            }
            let node = self
                .risk_node(risk)
                .and_then(|id| self.graph.nodes.get_mut(&id));
            let Some(node) = node else {
                self.risks_unplaced += u64::from(in_scope(self.scope, env));
                continue;
            };
            add_risk(&mut node.risks, risk);
        }
    }

    /// The node a risk belongs to: its subject when that is a workload
    /// or a definition, else its target system, else the service.
    fn risk_node(&self, risk: &RiskRow) -> Option<String> {
        let by_subject = risk
            .subject
            .as_ref()
            .and_then(|subject| self.subject_node(subject));
        if by_subject.is_some() {
            return by_subject;
        }
        let target = risk.target.as_deref()?;
        if let Some(env) = risk.environment.as_deref() {
            let system = sys_id(Some(env), target);
            return self.graph.has(&system).then_some(system);
        }
        let service = svc_id(target);
        if self.graph.has(&service) {
            return Some(service);
        }
        let named = self.facts.sys_by_name.get(target)?;
        named.first().filter(|_| named.len() == 1).cloned()
    }

    fn workload_named(&self, namespace: &str, name: &str) -> Option<String> {
        let prefix = format!("wl:{}/", ids::seg(namespace));
        let name = ids::seg(name);
        self.graph
            .nodes
            .range(prefix.clone()..)
            .take_while(|(id, _)| id.starts_with(&prefix))
            .find(|(id, _)| {
                let rest = &id[prefix.len()..];
                rest.split_once('/').is_some_and(|(_, n)| n == name)
            })
            .map(|(id, _)| id.clone())
    }

    fn subject_node(&self, subject: &RiskSubject) -> Option<String> {
        let definition = match subject {
            RiskSubject::Workload { namespace, name } => {
                return self.workload_named(namespace, name);
            }
            RiskSubject::Definition(definition) => definition,
        };
        let by_id = def_id(definition);
        if self.graph.has(&by_id) {
            return Some(by_id);
        }
        self.facts.def_ids.get(definition).cloned()
    }

    /// A system the coverage matrix has no row for is a gap unless a run
    /// inside the window says otherwise.
    fn mark_untested_systems(&mut self) {
        let rows = &self.facts.sys_rows;
        for node in self.graph.nodes.values_mut() {
            if node.kind != StarKind::System || rows.contains(&node.id) {
                continue;
            }
            let tested = node.evidence.runs + node.evidence.manual > 0;
            node.gap = (!tested).then_some(Gap::Missing);
        }
    }

    fn roll_up_entities(&mut self) {
        let entities: Vec<String> = self
            .graph
            .nodes
            .values()
            .filter(|n| n.kind == StarKind::Entity)
            .map(|n| n.id.clone())
            .collect();
        for entity in entities {
            self.roll_up_entity(&entity);
        }
    }

    /// An environment name only when the caller may see it.
    fn visible_env<'e>(&self, env: Option<&'e str>) -> Option<&'e str> {
        env.filter(|e| in_scope(self.scope, Some(e)))
    }

    /// An entity shows the best of the systems that implement it: one
    /// covered system clears its gap. How many systems that hides is in
    /// the node's `rollup` (see [`super::rollup`]).
    fn roll_up_entity(&mut self, entity: &str) {
        let empty = BTreeSet::new();
        let systems = self.facts.ent_systems.get(entity).unwrap_or(&empty);
        let mut evidence = Evidence::default();
        let mut risks = RiskSummary::default();
        let mut gap = Some(Gap::Missing);
        let mut env: Option<String> = None;
        let mut last_seen = None;
        for system in systems.iter().filter_map(|s| self.graph.nodes.get(s)) {
            merge_evidence(&mut evidence, &system.evidence);
            merge_risks(&mut risks, &system.risks);
            gap = gap.min(system.gap);
            env = min_env(env, self.visible_env(system.env.as_deref()));
            last_seen = last_seen.max(system.last_seen_ns);
        }
        for (service, links) in &self.facts.svc_links {
            if !links.entities.contains(entity) {
                continue;
            }
            let Some(service) = self.graph.nodes.get(service) else {
                continue;
            };
            env = min_env(env, self.visible_env(service.env.as_deref()));
            last_seen = last_seen.max(service.last_seen_ns);
        }
        let Some(node) = self.graph.nodes.get_mut(entity) else {
            return;
        };
        node.evidence = evidence;
        node.risks = risks;
        node.gap = gap;
        node.env = env;
        node.last_seen_ns = last_seen;
    }

    /// A service others call, with no tested system behind it, is the
    /// untested dependency.
    fn mark_untested_callees(&mut self) {
        let callees: BTreeSet<String> = self
            .graph
            .edges
            .keys()
            .filter(|(_, rel, _)| *rel == StarRel::Calls)
            .map(|(_, _, to)| to.clone())
            .collect();
        for callee in callees {
            if self.has_tested_system(&callee) {
                continue;
            }
            if let Some(node) = self.graph.nodes.get_mut(&callee) {
                node.gap = Some(Gap::Missing);
            }
        }
    }

    fn has_tested_system(&self, service: &str) -> bool {
        let Some(links) = self.facts.svc_links.get(service) else {
            return false;
        };
        links
            .systems
            .iter()
            .filter_map(|s| self.graph.nodes.get(s))
            .any(|system| system.gap != Some(Gap::Missing))
    }
}

fn min_env(held: Option<String>, candidate: Option<&str>) -> Option<String> {
    match (held, candidate) {
        (Some(held), Some(candidate)) if candidate < held.as_str() => Some(candidate.to_string()),
        (None, Some(candidate)) => Some(candidate.to_string()),
        (held, _) => held,
    }
}
