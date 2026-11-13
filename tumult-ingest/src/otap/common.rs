//! What the three root tables share: the `resource` and `scope` struct
//! columns, their ids and attribute tables, and the row id used by child
//! tables.

use arrow::array::RecordBatch;
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;

use super::arrays::Table;
use super::attrs::{attributes, AttrMap};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// A root table with its resource/scope context resolved.
pub(super) struct Root {
    pub table: Table,
    /// Delta-decoded row ids (`None` where the row has no children).
    pub ids: Vec<Option<u32>>,
    resource: Option<Table>,
    scope: Option<Table>,
    resource_ids: Vec<Option<u32>>,
    scope_ids: Vec<Option<u32>>,
    resource_attrs: AttrMap,
    scope_attrs: AttrMap,
}

/// Which (resource, scope) group a row belongs to.
pub(super) type GroupKey = (Option<u32>, Option<u32>);

impl Root {
    pub fn new(
        name: &'static str,
        root: &RecordBatch,
        batch: &OtapBatch,
    ) -> Result<Self, OtapError> {
        let table = Table::new(name, root)?;
        let ids = table.ids("id")?;
        let resource = table.struct_table("resource")?;
        let scope = table.struct_table("scope")?;
        let resource_ids = match &resource {
            Some(r) => r.ids("id")?,
            None => vec![None; table.len()],
        };
        let scope_ids = match &scope {
            Some(s) => s.ids("id")?,
            None => vec![None; table.len()],
        };
        Ok(Self {
            table,
            ids,
            resource,
            scope,
            resource_ids,
            scope_ids,
            resource_attrs: attributes(
                batch.get(ArrowPayloadType::ResourceAttrs),
                "RESOURCE_ATTRS",
            )?,
            scope_attrs: attributes(batch.get(ArrowPayloadType::ScopeAttrs), "SCOPE_ATTRS")?,
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn group_key(&self, i: usize) -> GroupKey {
        (self.resource_ids[i], self.scope_ids[i])
    }

    /// The OTLP resource of row `i` (`None` when the struct is null).
    pub fn resource(&self, i: usize) -> Result<Option<Resource>, OtapError> {
        let Some(r) = self.resource.as_ref().filter(|r| r.is_valid(i)) else {
            return Ok(None);
        };
        Ok(Some(Resource {
            attributes: self.resource_ids[i]
                .and_then(|id| self.resource_attrs.get(&id).cloned())
                .unwrap_or_default(),
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

    /// The OTLP instrumentation scope of row `i` (`None` when null).
    pub fn scope(&self, i: usize) -> Result<Option<InstrumentationScope>, OtapError> {
        let Some(s) = self.scope.as_ref().filter(|s| s.is_valid(i)) else {
            return Ok(None);
        };
        Ok(Some(InstrumentationScope {
            name: s.string_at("name", i)?,
            version: s.string_at("version", i)?,
            attributes: self.scope_ids[i]
                .and_then(|id| self.scope_attrs.get(&id).cloned())
                .unwrap_or_default(),
            dropped_attributes_count: s.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        }))
    }

    /// The scope-level schema URL of row `i` (the root table's `schema_url`).
    pub fn scope_schema_url(&self, i: usize) -> Result<String, OtapError> {
        self.table.string_at("schema_url", i)
    }
}

/// Walk a root table in row order, yielding `(row, new_resource, new_scope)`:
/// consecutive rows with the same (resource id, scope id) share one
/// resource and one scope group, as the producer sorts them.
pub(super) fn groups(root: &Root) -> impl Iterator<Item = (usize, bool, bool)> + '_ {
    let mut current: Option<GroupKey> = None;
    (0..root.len()).map(move |i| {
        let key = root.group_key(i);
        let new_resource = current.is_none_or(|(r, _)| r != key.0);
        let new_scope = new_resource || current.is_none_or(|(_, s)| s != key.1);
        current = Some(key);
        (i, new_resource, new_scope)
    })
}
