//! Ownership and the spine: who owns each entity and system, and the one
//! parent every node hangs from in the layout.

use std::collections::BTreeSet;

use super::build::{tier_rank, Builder, ROOT, UNASSIGNED};
use super::input::GlobRow;
use super::model::{EdgeSource, StarKind, StarRel};

/// Minimal glob: `*` matches any run of characters, everything else is
/// literal. The same rule `org.yaml` assignments use for experiment
/// names, repeated here because this crate sits below the one that
/// parses the file.
pub(super) fn glob_match(pattern: &str, name: &str) -> bool {
    let segments: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
    let Some(last) = segments.last() else {
        return pattern.contains('*') || name.is_empty();
    };
    let mut rest = name;
    for (i, segment) in segments.iter().enumerate() {
        if i == 0 && !pattern.starts_with('*') {
            let Some(stripped) = rest.strip_prefix(segment) else {
                return false;
            };
            rest = stripped;
            continue;
        }
        let Some(at) = rest.find(segment) else {
            return false;
        };
        rest = &rest[at + segment.len()..];
    }
    pattern.ends_with('*') || name.ends_with(last)
}

impl Builder<'_> {
    /// Resolve owners, then give every spine node its parent.
    pub fn assign_spine(&mut self) {
        let ids: Vec<(String, StarKind)> = self
            .graph
            .nodes
            .values()
            .map(|n| (n.id.clone(), n.kind))
            .collect();
        for (id, _) in ids.iter().filter(|(_, k)| *k == StarKind::Entity) {
            self.place_entity(id);
        }
        for (id, _) in ids.iter().filter(|(_, k)| *k == StarKind::System) {
            self.place_system(id);
        }
        for (id, _) in ids.iter().filter(|(_, k)| *k == StarKind::Service) {
            self.place_service(id);
        }
        self.add_unassigned();
    }

    /// The first glob, in file order, that matches `name` and names an
    /// existing unit.
    fn first_glob(&self, globs: &[GlobRow], name: &str) -> Option<String> {
        globs
            .iter()
            .filter(|g| glob_match(&g.pattern, name))
            .map(|g| format!("org:{}", g.path))
            .find(|id| self.graph.has(id))
    }

    /// The org unit an `ict.entity.owner` value names, by path or name.
    fn unit_named(&self, owner: &str) -> Option<String> {
        let by_path = format!("org:{owner}");
        if by_path != ROOT && self.graph.has(&by_path) {
            return Some(by_path);
        }
        self.facts.org_names.get(owner).cloned()
    }

    fn owner_by_experiments(&self, systems: &BTreeSet<String>) -> Option<String> {
        let names: BTreeSet<&String> = systems
            .iter()
            .filter_map(|s| self.facts.sys_experiments.get(s))
            .flatten()
            .collect();
        names
            .into_iter()
            .find_map(|name| self.first_glob(&self.inputs.experiment_globs, name))
    }

    /// Owner of an entity: telemetry first, then the entity globs, then
    /// the experiment-name assignment of its systems.
    fn entity_owner(&self, entity: &str) -> Option<(String, EdgeSource)> {
        let reported = self.facts.ent_owners.get(entity);
        let telemetry = reported
            .into_iter()
            .flatten()
            .find_map(|owner| self.unit_named(owner));
        if let Some(unit) = telemetry {
            return Some((unit, EdgeSource::Telemetry));
        }
        let key = entity.strip_prefix("ent:").unwrap_or(entity);
        if let Some(unit) = self.first_glob(&self.inputs.entity_globs, key) {
            return Some((unit, EdgeSource::Declared));
        }
        let systems = self.facts.ent_systems.get(entity)?;
        let unit = self.owner_by_experiments(systems)?;
        Some((unit, EdgeSource::Declared))
    }

    fn set_parent(&mut self, id: &str, parent: &str) {
        if let Some(node) = self.graph.nodes.get_mut(id) {
            node.parent = Some(parent.to_string());
        }
    }

    fn own(&mut self, id: &str, owner: Option<(String, EdgeSource)>) {
        let Some((unit, source)) = owner else {
            self.set_parent(id, UNASSIGNED);
            return;
        };
        self.graph.link(&unit, StarRel::Owns, id, source);
        self.set_parent(id, &unit);
    }

    fn place_entity(&mut self, entity: &str) {
        let owner = self.entity_owner(entity);
        self.own(entity, owner);
    }

    /// The entity a node hangs from: the most critical, then the
    /// smallest id.
    fn best_entity(&self, entities: Option<&BTreeSet<String>>) -> Option<String> {
        entities?
            .iter()
            .max_by_key(|id| {
                let tier = self.graph.nodes.get(*id).and_then(|n| n.tier.as_deref());
                (tier_rank(tier), std::cmp::Reverse(*id))
            })
            .cloned()
    }

    fn place_system(&mut self, system: &str) {
        if let Some(entity) = self.best_entity(self.facts.sys_entities.get(system)) {
            self.set_parent(system, &entity);
            return;
        }
        let own: BTreeSet<String> = std::iter::once(system.to_string()).collect();
        let owner = self
            .owner_by_experiments(&own)
            .map(|unit| (unit, EdgeSource::Declared));
        self.own(system, owner);
    }

    fn place_service(&mut self, service: &str) {
        let links = self.facts.svc_links.get(service);
        let parent = links
            .and_then(|l| l.components.first().or(l.systems.first()).cloned())
            .or_else(|| self.best_entity(links.map(|l| &l.entities)))
            .unwrap_or(UNASSIGNED.to_string());
        let conflict = links.is_some_and(|l| l.entities.len() > 1);
        self.set_parent(service, &parent);
        if let Some(node) = self.graph.nodes.get_mut(service) {
            node.conflict = conflict;
        }
    }

    /// The unassigned unit exists only when something hangs from it.
    fn add_unassigned(&mut self) {
        let used = self
            .graph
            .nodes
            .values()
            .any(|n| n.parent.as_deref() == Some(UNASSIGNED));
        if !used {
            return;
        }
        let node = self
            .graph
            .ensure(UNASSIGNED, StarKind::Org, "(unassigned)", false);
        node.org_kind = Some("unassigned".to_string());
        node.parent = Some(ROOT.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn glob_matches_like_org_yaml_assignments() {
        assert!(glob_match("cif-*", "cif-0042"));
        assert!(glob_match("*-0042", "cif-0042"));
        assert!(glob_match("cif-*42", "cif-0042"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("cif-0042", "cif-0042"));
        assert!(!glob_match("cif-0042", "cif-00420"));
        assert!(!glob_match("cif-*", "xcif-0042"));
        assert!(!glob_match("*-0043", "cif-0042"));
        assert!(!glob_match("", "cif-0042"));
        assert!(glob_match("", ""));
    }
}
