//! Ownership and the spine: who owns each entity and system, and the one
//! parent every node hangs from in the layout.

use std::collections::BTreeSet;

use super::build::{flag, org_id, tier_rank, Builder, ROOT, UNASSIGNED};
use super::model::{EdgeSource, Flag, StarKind, StarRel};

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

    /// The unit an owner path resolved by the caller stands for, when
    /// that unit is on the map.
    fn unit_at(&self, path: Option<&String>) -> Option<String> {
        path.map(|path| org_id(path))
            .filter(|id| self.graph.has(id))
    }

    /// The unit `org.yaml` assigns an entity to. The key is the entity id
    /// as reported, which is the node's label.
    fn declared_owner(&self, entity: &str) -> Option<String> {
        let key = &self.graph.nodes.get(entity)?.label;
        self.unit_at(self.inputs.entity_owners.get(key))
    }

    /// The org unit an `ict.entity.owner` value names, by path or name.
    fn unit_named(&self, owner: &str) -> Option<String> {
        let by_path = org_id(owner);
        if by_path != ROOT && self.graph.has(&by_path) {
            return Some(by_path);
        }
        self.facts.org_names.get(owner).cloned()
    }

    /// The owner of the experiments run against `systems`. The names are
    /// walked in byte order and the first one that has an owner decides,
    /// so the answer never depends on run or row order: of `b-exp` (edge)
    /// and `c-exp` (data), `b-exp` wins.
    fn owner_by_experiments(&self, systems: &BTreeSet<String>) -> Option<String> {
        let names: BTreeSet<&String> = systems
            .iter()
            .filter_map(|s| self.facts.sys_experiments.get(s))
            .flatten()
            .collect();
        names
            .into_iter()
            .find_map(|name| self.unit_at(self.inputs.experiment_owners.get(name)))
    }

    /// Owner of an entity: telemetry first, then its `org.yaml`
    /// assignment, then the assignment of its systems' experiments.
    fn entity_owner(&self, entity: &str) -> Option<(String, EdgeSource)> {
        let reported = self.facts.ent_owners.get(entity);
        let telemetry = reported
            .into_iter()
            .flatten()
            .find_map(|owner| self.unit_named(owner));
        if let Some(unit) = telemetry {
            return Some((unit, EdgeSource::Telemetry));
        }
        if let Some(unit) = self.declared_owner(entity) {
            return Some((unit, EdgeSource::Declared));
        }
        let systems = self.facts.ent_systems.get(entity)?;
        let unit = self.owner_by_experiments(systems)?;
        Some((unit, EdgeSource::Declared))
    }

    /// Warnings about who owns an entity: telemetry naming more than one
    /// owner, and telemetry disagreeing with `org.yaml`. An owner value
    /// that names no unit counts as itself, so it disagrees with any
    /// declared unit.
    fn owner_flags(&self, entity: &str) -> Vec<Flag> {
        let empty = BTreeSet::new();
        let reported = self.facts.ent_owners.get(entity).unwrap_or(&empty);
        let named: BTreeSet<String> = reported
            .iter()
            .map(|owner| self.unit_named(owner).unwrap_or_else(|| owner.clone()))
            .collect();
        let mut flags = Vec::new();
        if named.len() > 1 {
            flags.push(Flag::MultipleOwners);
        }
        let declared = self.declared_owner(entity);
        if declared.is_some_and(|unit| named.iter().any(|n| *n != unit)) {
            flags.push(Flag::OwnerConflict);
        }
        flags
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
        let flags = self.owner_flags(entity);
        self.own(entity, owner);
        let Some(node) = self.graph.nodes.get_mut(entity) else {
            return;
        };
        for found in flags {
            flag(node, found);
        }
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
