//! What the three root tables share: the `resource` and `scope` struct
//! columns, their ids and attribute tables, and the row id used by child
//! tables.

use std::collections::HashMap;

use arrow::array::RecordBatch;
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;

use super::arrays::Table;
use super::attrs::Attrs;
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// A root table with its resource/scope context resolved.
pub(super) struct Root {
    pub table: Table,
    /// Delta-decoded row ids (`None` where the row has no children). An id
    /// names one row only.
    pub ids: Vec<Option<u32>>,
    resource: Option<Table>,
    scope: Option<Table>,
    resource_ids: Vec<Option<u32>>,
    scope_ids: Vec<Option<u32>>,
    resource_attrs: Attrs,
    scope_attrs: Attrs,
}

/// The ids of a `resource`/`scope` struct column, `None` where the struct
/// itself is null.
fn struct_ids(table: Option<&Table>, len: usize) -> Result<Vec<Option<u32>>, OtapError> {
    let Some(t) = table else {
        return Ok(vec![None; len]);
    };
    let mut ids = t.ids("id")?;
    for (i, id) in ids.iter_mut().enumerate() {
        if !t.is_valid(i) {
            *id = None;
        }
    }
    Ok(ids)
}

impl Root {
    pub fn new(
        name: &'static str,
        root: &RecordBatch,
        batch: &OtapBatch,
    ) -> Result<Self, OtapError> {
        let table = Table::new(name, root, batch.budget())?;
        let ids = table.unique_ids("id")?;
        let resource = table.struct_table("resource")?;
        let scope = table.struct_table("scope")?;
        let resource_ids = struct_ids(resource.as_ref(), table.len())?;
        let scope_ids = struct_ids(scope.as_ref(), table.len())?;
        Ok(Self {
            table,
            ids,
            resource,
            scope,
            resource_ids,
            scope_ids,
            resource_attrs: Attrs::decode(
                batch,
                ArrowPayloadType::ResourceAttrs,
                "RESOURCE_ATTRS",
            )?,
            scope_attrs: Attrs::decode(batch, ArrowPayloadType::ScopeAttrs, "SCOPE_ATTRS")?,
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn resource_id(&self, i: usize) -> Option<u32> {
        self.resource_ids[i]
    }

    pub fn scope_id(&self, i: usize) -> Option<u32> {
        self.scope_ids[i]
    }

    /// The OTLP resource of row `i` (`None` when the struct is null). Called
    /// once per resource id ([`Groups`]), so the attributes move out.
    pub fn resource(&mut self, i: usize) -> Result<Option<Resource>, OtapError> {
        let Some(r) = self.resource.as_ref().filter(|r| r.is_valid(i)) else {
            return Ok(None);
        };
        Ok(Some(Resource {
            attributes: self.resource_attrs.take(self.resource_ids[i]),
            dropped_attributes_count: r.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
            ..Default::default()
        }))
    }

    /// The resource-level schema URL of row `i`.
    pub fn resource_schema_url(&self, i: usize) -> Result<String, OtapError> {
        match &self.resource {
            Some(r) if r.is_valid(i) => r.string_at("schema_url", i),
            _ => Ok(String::new()),
        }
    }

    /// The OTLP instrumentation scope of row `i` (`None` when null). A scope
    /// id may appear under several resources; every copy of its attributes
    /// after the first counts against the batch's attribute limit.
    pub fn scope(&mut self, i: usize) -> Result<Option<InstrumentationScope>, OtapError> {
        let Some(s) = self.scope.as_ref().filter(|s| s.is_valid(i)) else {
            return Ok(None);
        };
        Ok(Some(InstrumentationScope {
            name: s.string_at("name", i)?,
            version: s.string_at("version", i)?,
            attributes: self.scope_attrs.share(self.scope_ids[i])?,
            dropped_attributes_count: s.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        }))
    }

    /// The scope-level schema URL of row `i` (the root table's `schema_url`).
    pub fn scope_schema_url(&self, i: usize) -> Result<String, OtapError> {
        self.table.string_at("schema_url", i)
    }

    /// Fail when resource or scope attributes are left that no row named.
    pub fn finish(self) -> Result<(), OtapError> {
        self.resource_attrs.finish()?;
        self.scope_attrs.finish()
    }
}

/// Where each row of a root table goes in the OTLP request: one resource
/// group per resource id and, within it, one scope group per scope id, in
/// order of first appearance. Rows need not arrive sorted, and a resource
/// whose rows are interleaved with another's still gets a single group.
#[derive(Default)]
pub(super) struct Groups {
    resources: HashMap<Option<u32>, usize>,
    scopes: HashMap<(usize, Option<u32>), usize>,
}

impl Groups {
    /// The index of the resource group for `id`, and whether the caller must
    /// create it (as the group at index `next`).
    pub fn resource(&mut self, id: Option<u32>, next: usize) -> (usize, bool) {
        slot(self.resources.entry(id), next)
    }

    /// The index of the scope group for `id` under resource group
    /// `resource`, and whether the caller must create it.
    pub fn scope(&mut self, resource: usize, id: Option<u32>, next: usize) -> (usize, bool) {
        slot(self.scopes.entry((resource, id)), next)
    }
}

fn slot<K>(entry: std::collections::hash_map::Entry<'_, K, usize>, next: usize) -> (usize, bool) {
    use std::collections::hash_map::Entry;
    match entry {
        Entry::Occupied(found) => (*found.get(), false),
        Entry::Vacant(vacant) => (*vacant.insert(next), true),
    }
}
