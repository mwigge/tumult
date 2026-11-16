//! OTAP attribute tables (`*_ATTRS`) and `AnyValue` columns.
//!
//! An attribute table has one row per (parent, key, value): `parent_id`,
//! `key`, a `type` code and one value column per type (`str`, `int`,
//! `double`, `bool`, `bytes`, `ser`). Map and array values travel CBOR-encoded
//! in `ser`. Rows are sorted by (key, value); whenever key and value repeat
//! the previous row's, `parent_id` is a delta from the previous parent.
//!
//! Every row counts against the batch's attribute limit, and every row must
//! end up on a parent: [`Attrs::finish`] fails the batch when rows are left
//! over, so attributes are never dropped silently.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use opentelemetry_proto::tonic::common::v1::{
    any_value, AnyValue, ArrayValue, KeyValue, KeyValueList,
};

use super::arrays::{orphans, Table};
use super::decode::OtapBatch;
use super::limits::{Budget, MAX_CBOR_DEPTH};
use super::proto::ArrowPayloadType;
use super::OtapError;

/// One decoded attribute table: lists keyed by the id of the row they
/// belong to, in table order.
pub(super) struct Attrs {
    table: &'static str,
    lists: HashMap<u32, Vec<KeyValue>>,
    /// Ids handed out by [`Attrs::share`], which keeps the list.
    shared: HashSet<u32>,
    budget: Arc<Budget>,
}

/// Columns the exporter always writes in an attribute table.
const REQUIRED: [&str; 3] = ["parent_id", "key", "type"];

/// Value type codes of the OTAP `type` column.
mod value_type {
    pub const EMPTY: u8 = 0;
    pub const STR: u8 = 1;
    pub const INT: u8 = 2;
    pub const DOUBLE: u8 = 3;
    pub const BOOL: u8 = 4;
    pub const MAP: u8 = 5;
    pub const SLICE: u8 = 6;
    pub const BYTES: u8 = 7;
}

impl Attrs {
    /// Decode the batch's `payload` table (an absent table has no rows).
    pub fn decode(
        batch: &OtapBatch,
        payload: ArrowPayloadType,
        table: &'static str,
    ) -> Result<Self, OtapError> {
        let mut out = Self {
            table,
            lists: HashMap::new(),
            shared: HashSet::new(),
            budget: batch.budget().clone(),
        };
        let Some(rb) = batch.get(payload) else {
            return Ok(out);
        };
        let t = Table::new(table, rb, batch.budget())?;
        t.require(&REQUIRED)?;
        let parents = t.parents()?;
        out.budget.charge_attributes(t.len())?;
        let missing = parents.values.iter().filter(|p| p.is_none()).count();
        if missing > 0 {
            return Err(t.orphans(missing));
        }
        let mut previous: Option<(String, Option<any_value::Value>, u32)> = None;
        for (i, raw) in parents.values.iter().flatten().enumerate() {
            t.tick()?;
            let key = t.string_at("key", i)?;
            let value = any_value_at(&t, i)?;
            let parent = match &previous {
                Some((prev_key, prev_value, prev_parent))
                    if parents.encoded && *prev_key == key && *prev_value == value =>
                {
                    parents.after(*prev_parent, *raw)
                }
                _ => *raw,
            };
            previous = Some((key.clone(), value.clone(), parent));
            out.lists.entry(parent).or_default().push(KeyValue {
                key,
                value: value.map(|v| AnyValue { value: Some(v) }),
                ..Default::default()
            });
        }
        Ok(out)
    }

    /// Move the attributes of row `id` out. For parents whose ids are
    /// unique, so each list has exactly one taker.
    pub fn take(&mut self, id: Option<u32>) -> Vec<KeyValue> {
        id.and_then(|id| self.lists.remove(&id)).unwrap_or_default()
    }

    /// Copy the attributes of `id` for a parent that several groups may
    /// name (a scope under more than one resource). Every copy after the
    /// first counts against the batch's attribute limit.
    pub fn share(&mut self, id: Option<u32>) -> Result<Vec<KeyValue>, OtapError> {
        let Some((id, list)) = id.and_then(|id| Some((id, self.lists.get(&id)?))) else {
            return Ok(Vec::new());
        };
        if !self.shared.insert(id) {
            self.budget.charge_attributes(list.len())?;
        }
        Ok(list.clone())
    }

    /// Fail when rows are left whose parent row never asked for them.
    pub fn finish(self) -> Result<(), OtapError> {
        let left: usize = self
            .lists
            .iter()
            .filter(|(id, _)| !self.shared.contains(id))
            .map(|(_, list)| list.len())
            .sum();
        if left > 0 {
            return Err(orphans(self.table, left));
        }
        Ok(())
    }
}

/// Read the typed value columns of row `i` (attribute tables and log bodies
/// share the layout). `None` is the empty value.
pub(super) fn any_value_at(t: &Table, i: usize) -> Result<Option<any_value::Value>, OtapError> {
    use any_value::Value;
    let Some(code) = t.u8_at("type", i)? else {
        return Ok(None);
    };
    Ok(match code {
        value_type::EMPTY => None,
        value_type::STR => Some(Value::StringValue(t.string_at("str", i)?)),
        value_type::INT => Some(Value::IntValue(t.i64_at("int", i)?.unwrap_or_default())),
        value_type::DOUBLE => Some(Value::DoubleValue(
            t.f64_at("double", i)?.unwrap_or_default(),
        )),
        value_type::BOOL => Some(Value::BoolValue(t.bool_at("bool", i)?.unwrap_or_default())),
        value_type::BYTES => Some(Value::BytesValue(t.vec_at("bytes", i)?)),
        value_type::MAP | value_type::SLICE => match t.bytes_at("ser", i)? {
            Some(bytes) => {
                // The decoded tree is far larger than its encoding, and an
                // element takes at least one byte: count the bytes as
                // attributes before decoding.
                t.budget().charge_attributes(bytes.len())?;
                cbor_value(bytes).map_err(|_| t.row_error("ser", i, "invalid CBOR"))?
            }
            None => None,
        },
        other => return Err(t.row_error("type", i, &format!("unknown value type {other}"))),
    })
}

fn cbor_value(
    bytes: &[u8],
) -> Result<Option<any_value::Value>, ciborium::de::Error<std::io::Error>> {
    let value: ciborium::Value =
        ciborium::de::from_reader_with_recursion_limit(bytes, MAX_CBOR_DEPTH)?;
    Ok(convert(value))
}

fn convert(value: ciborium::Value) -> Option<any_value::Value> {
    use any_value::Value;
    use ciborium::Value as C;
    match value {
        C::Null => None,
        C::Bool(b) => Some(Value::BoolValue(b)),
        C::Integer(i) => Some(Value::IntValue(
            i64::try_from(i128::from(i).clamp(i128::from(i64::MIN), i128::from(i64::MAX)))
                .unwrap_or_default(),
        )),
        C::Float(f) => Some(Value::DoubleValue(f)),
        C::Text(s) => Some(Value::StringValue(s)),
        C::Bytes(b) => Some(Value::BytesValue(b)),
        C::Array(items) => Some(Value::ArrayValue(ArrayValue {
            values: items
                .into_iter()
                .map(|v| AnyValue { value: convert(v) })
                .collect(),
        })),
        C::Map(entries) => Some(Value::KvlistValue(KeyValueList {
            values: entries
                .into_iter()
                .map(|(k, v)| KeyValue {
                    key: match k {
                        C::Text(s) => s,
                        other => format!("{other:?}"),
                    },
                    value: Some(AnyValue { value: convert(v) }),
                    ..Default::default()
                })
                .collect(),
        })),
        C::Tag(_, inner) => convert(*inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `depth` arrays inside each other around the integer 1.
    fn nested(depth: usize) -> Vec<u8> {
        let mut bytes = vec![0x81; depth];
        bytes.push(0x01);
        bytes
    }

    #[test]
    fn shallow_cbor_decodes_and_deep_cbor_is_refused() {
        assert!(matches!(
            cbor_value(&nested(4)),
            Ok(Some(any_value::Value::ArrayValue(_)))
        ));
        // Attribute values are a few levels deep; nothing needs forty.
        assert!(cbor_value(&nested(40)).is_err(), "depth 40 decoded");
    }

    #[tokio::test]
    async fn very_deep_cbor_does_not_overflow_the_blocking_pool_stack() {
        // Where the service decodes: a blocking-pool thread.
        let refused = tokio::task::spawn_blocking(|| {
            [1_000, 100_000, 3_000_000]
                .into_iter()
                .all(|depth| cbor_value(&nested(depth)).is_err())
        })
        .await
        .unwrap();
        assert!(refused);
        // Maps nest the same way.
        let mut maps = Vec::new();
        for _ in 0..100_000 {
            maps.extend_from_slice(&[0xa1, 0x61, b'k']);
        }
        maps.push(0x01);
        let refused = tokio::task::spawn_blocking(move || cbor_value(&maps).is_err())
            .await
            .unwrap();
        assert!(refused);
    }
}
