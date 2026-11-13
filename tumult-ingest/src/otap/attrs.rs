//! OTAP attribute tables (`*_ATTRS`) and `AnyValue` columns.
//!
//! An attribute table has one row per (parent, key, value): `parent_id`,
//! `key`, a `type` code and one value column per type (`str`, `int`,
//! `double`, `bool`, `bytes`, `ser`). Map and array values travel CBOR-encoded
//! in `ser`. Rows are sorted by (key, value); whenever key and value repeat
//! the previous row's, `parent_id` is a delta from the previous parent.

use std::collections::HashMap;

use arrow::array::RecordBatch;
use opentelemetry_proto::tonic::common::v1::{
    any_value, AnyValue, ArrayValue, KeyValue, KeyValueList,
};

use super::arrays::Table;
use super::OtapError;

/// Attributes keyed by the id of the row they belong to, in table order.
pub(super) type AttrMap = HashMap<u32, Vec<KeyValue>>;

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

/// Decode an attribute table into per-parent attribute lists. A missing
/// table (`None`) is an empty map.
pub(super) fn attributes(
    batch: Option<&RecordBatch>,
    table: &'static str,
) -> Result<AttrMap, OtapError> {
    let Some(batch) = batch else {
        return Ok(AttrMap::new());
    };
    let t = Table::new(table, batch)?;
    let parents = t.id_column("parent_id")?.ok_or_else(|| OtapError::Column {
        table,
        column: "parent_id".into(),
        reason: "column is required".into(),
    })?;
    let mut out = AttrMap::new();
    let mut previous: Option<(String, Option<any_value::Value>, u32)> = None;
    for i in 0..t.len() {
        let Some(raw) = parents.values[i] else {
            continue;
        };
        let key = t.string_at("key", i)?;
        let value = any_value_at(&t, i)?;
        let parent = match &previous {
            Some((prev_key, prev_value, prev_parent))
                if parents.encoded && *prev_key == key && *prev_value == value =>
            {
                prev_parent.wrapping_add(raw)
            }
            _ => raw,
        };
        previous = Some((key.clone(), value.clone(), parent));
        out.entry(parent).or_default().push(KeyValue {
            key,
            value: value.map(|v| AnyValue { value: Some(v) }),
            ..Default::default()
        });
    }
    Ok(out)
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
            Some(bytes) => cbor_value(bytes).map_err(|e| OtapError::Column {
                table: "attributes",
                column: "ser".into(),
                reason: format!("row {i}: invalid CBOR: {e}"),
            })?,
            None => None,
        },
        other => {
            return Err(OtapError::Column {
                table: "attributes",
                column: "type".into(),
                reason: format!("row {i}: unknown value type {other}"),
            })
        }
    })
}

fn cbor_value(
    bytes: &[u8],
) -> Result<Option<any_value::Value>, ciborium::de::Error<std::io::Error>> {
    let value: ciborium::Value = ciborium::from_reader(bytes)?;
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
