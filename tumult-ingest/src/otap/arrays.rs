//! Typed, null-aware access to the columns of one OTAP record batch.
//!
//! Dictionary-encoded columns are materialised up front (the producer
//! chooses dictionary encoding per column and per batch), so every accessor
//! sees plain arrays. A dictionary lets a few bytes on the wire stand for
//! many rows, so the expanded size is charged to the batch's byte budget
//! before the expansion, and the expansion goes row by row under the
//! batch's deadline. Id columns honour the `encoding` field metadata:
//! absent or `delta` means cumulative-sum decoding, `plain` means values as
//! sent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{make_array, new_null_array, MutableArrayData};
use arrow::array::{Array, ArrayRef, AsArray, OffsetSizeTrait, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, DurationNanosecondType, FieldRef, Fields, Float64Type, Int32Type,
    Int64Type, TimeUnit, TimestampNanosecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};

use super::limits::Budget;
use super::OtapError;
use crate::log_guard;

/// Name of the column linking a child row to its parent.
const PARENT_ID: &str = "parent_id";

/// Field metadata key naming the column's id encoding.
const ENCODING: &str = "encoding";
/// Metadata value for ids transmitted as-is.
const PLAIN: &str = "plain";

/// One table (a record batch or a struct column of one) with its columns
/// materialised to plain arrays.
pub(super) struct Table {
    name: &'static str,
    prefix: String,
    fields: Fields,
    columns: Vec<ArrayRef>,
    /// Column position by name, resolved once: accessors run per cell.
    index: HashMap<String, usize>,
    len: usize,
    validity: Option<NullBuffer>,
    budget: Arc<Budget>,
}

/// An id column before delta decoding.
pub(super) struct IdColumn {
    pub values: Vec<Option<u32>>,
    /// `true` when the field is delta-encoded (metadata absent or not `plain`).
    pub encoded: bool,
    mask: u32,
}

impl IdColumn {
    /// Apply cumulative-sum decoding over the non-null values when the
    /// column is encoded; nulls stay null and do not advance the sum.
    pub fn decoded(&self) -> Vec<Option<u32>> {
        if !self.encoded {
            return self.values.clone();
        }
        let mut acc: u32 = 0;
        self.values
            .iter()
            .map(|v| {
                v.map(|delta| {
                    acc = acc.wrapping_add(delta) & self.mask;
                    acc
                })
            })
            .collect()
    }

    /// The id `delta` after `previous`, wrapping at the column's width.
    pub fn after(&self, previous: u32, delta: u32) -> u32 {
        previous.wrapping_add(delta) & self.mask
    }
}

/// Bytes a dictionary array occupies once every row holds its own value.
fn expanded_bytes(array: &ArrayRef, value_type: &DataType) -> Result<usize, String> {
    let dictionary = array.as_any_dictionary();
    let values = dictionary.values();
    if values.is_empty() {
        return Ok(0);
    }
    let fixed = |width: usize| Ok(array.len().saturating_mul(width));
    let keys = || dictionary.normalized_keys();
    match value_type {
        DataType::Utf8 => Ok(referenced(
            &keys(),
            values.as_string::<i32>().value_offsets(),
        )),
        DataType::LargeUtf8 => Ok(referenced(
            &keys(),
            values.as_string::<i64>().value_offsets(),
        )),
        DataType::Binary => Ok(referenced(
            &keys(),
            values.as_binary::<i32>().value_offsets(),
        )),
        DataType::LargeBinary => Ok(referenced(
            &keys(),
            values.as_binary::<i64>().value_offsets(),
        )),
        DataType::FixedSizeBinary(width) => fixed(usize::try_from(*width).unwrap_or(0)),
        DataType::Boolean => fixed(1),
        other => match other.primitive_width() {
            Some(width) => fixed(width),
            None => Err(format!(
                "unsupported dictionary value type {}",
                log_guard::kind(other)
            )),
        },
    }
}

/// The dictionary column `column` with every row holding its own value.
///
/// Done row by row rather than with one `cast`, so the batch's deadline
/// ends it part-way: every row is a [`Budget::tick`]. A row whose key is
/// null is null; `normalized_keys` keeps every key inside the values (the
/// IPC reader lets none through that is not).
fn materialised(column: &ArrayRef, budget: &Budget) -> Result<ArrayRef, OtapError> {
    let dictionary = column.as_any_dictionary();
    let values = dictionary.values().to_data();
    if values.is_empty() {
        return Ok(new_null_array(values.data_type(), column.len()));
    }
    let keys = dictionary.keys();
    let mut rows = MutableArrayData::new(vec![&values], true, column.len());
    for (row, key) in dictionary.normalized_keys().into_iter().enumerate() {
        budget.tick()?;
        if keys.is_null(row) {
            rows.extend_nulls(1);
            continue;
        }
        rows.extend(0, key, key + 1);
    }
    Ok(make_array(rows.freeze()))
}

/// Total length of the variable-size values the keys refer to.
fn referenced<O: OffsetSizeTrait>(keys: &[usize], offsets: &[O]) -> usize {
    keys.iter()
        .map(|&key| match (offsets.get(key), offsets.get(key + 1)) {
            (Some(start), Some(end)) => (*end - *start).as_usize(),
            _ => 0,
        })
        .fold(0, usize::saturating_add)
}

/// The error for `count` rows of `table` whose parent row is not in the
/// batch. Children are never dropped silently: an orphan fails the batch.
pub(super) fn orphans(table: &'static str, count: usize) -> OtapError {
    OtapError::Column {
        table,
        column: PARENT_ID.into(),
        reason: format!("{count} row(s) have no parent row"),
    }
}

impl Table {
    pub fn new(
        name: &'static str,
        batch: &RecordBatch,
        budget: &Arc<Budget>,
    ) -> Result<Self, OtapError> {
        Self::build(
            name,
            String::new(),
            batch.schema().fields().clone(),
            batch.columns(),
            batch.num_rows(),
            None,
            budget.clone(),
        )
    }

    fn build(
        name: &'static str,
        prefix: String,
        fields: Fields,
        source: &[ArrayRef],
        len: usize,
        validity: Option<NullBuffer>,
        budget: Arc<Budget>,
    ) -> Result<Self, OtapError> {
        budget.check_deadline()?;
        let mut columns = Vec::with_capacity(source.len());
        let mut index = HashMap::with_capacity(source.len());
        for (position, (field, column)) in fields.iter().zip(source).enumerate() {
            index.entry(field.name().clone()).or_insert(position);
            let DataType::Dictionary(_, value_type) = column.data_type() else {
                columns.push(column.clone());
                continue;
            };
            let failed = |reason: String| OtapError::Column {
                table: name,
                column: log_guard::clean(&format!("{prefix}{}", field.name())),
                reason,
            };
            budget.charge_bytes(expanded_bytes(column, value_type).map_err(failed)?)?;
            columns.push(materialised(column, &budget)?);
        }
        Ok(Self {
            name,
            prefix,
            fields,
            columns,
            index,
            len,
            validity,
            budget,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn budget(&self) -> &Arc<Budget> {
        &self.budget
    }

    /// Row validity of a struct-derived table (always valid for a root table).
    pub fn is_valid(&self, i: usize) -> bool {
        self.validity.as_ref().is_none_or(|v| v.is_valid(i))
    }

    fn err(&self, column: &str, reason: String) -> OtapError {
        OtapError::Column {
            table: self.name,
            column: format!("{}{column}", self.prefix),
            reason,
        }
    }

    fn mismatch(&self, column: &str, expected: &str, found: &DataType) -> OtapError {
        // The kind only: the full type repeats the peer's field names,
        // timezone and metadata.
        let found = log_guard::kind(found);
        self.err(column, format!("expected {expected}, found {found}"))
    }

    fn column(&self, name: &str) -> Option<(&FieldRef, &ArrayRef)> {
        let i = *self.index.get(name)?;
        Some((self.fields.get(i)?, self.columns.get(i)?))
    }

    /// Fail unless every one of `columns` is present.
    pub fn require(&self, columns: &[&str]) -> Result<(), OtapError> {
        match columns.iter().find(|name| !self.index.contains_key(**name)) {
            Some(missing) => Err(self.err(missing, "column is required".into())),
            None => Ok(()),
        }
    }

    /// Called once per row by the decoding loops: fails the batch when it
    /// has used up its time.
    pub fn tick(&self) -> Result<(), OtapError> {
        self.budget.tick()
    }

    /// An id of exactly `width` bytes that must be present and not all
    /// zero (a span's trace and span id).
    pub fn id_at(&self, name: &str, i: usize, width: usize) -> Result<Vec<u8>, OtapError> {
        let Some(id) = self.bytes_at(name, i)? else {
            return Err(self.row_error(name, i, "id is null"));
        };
        if id.len() != width {
            return Err(self.row_error(name, i, &format!("id is not {width} bytes long")));
        }
        if id.iter().all(|byte| *byte == 0) {
            return Err(self.row_error(name, i, "id is all zero"));
        }
        Ok(id.to_vec())
    }

    /// An id that is either absent (null or empty) or `width` bytes long.
    pub fn optional_id_at(&self, name: &str, i: usize, width: usize) -> Result<Vec<u8>, OtapError> {
        let id = self.vec_at(name, i)?;
        if !id.is_empty() && id.len() != width {
            return Err(self.row_error(name, i, &format!("id is not {width} bytes long")));
        }
        Ok(id)
    }

    /// A struct column as a table of its own (`None` when absent).
    pub fn struct_table(&self, name: &str) -> Result<Option<Table>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        let DataType::Struct(fields) = col.data_type() else {
            return Err(self.mismatch(name, "struct", col.data_type()));
        };
        let strukt = col.as_struct();
        Table::build(
            self.name,
            format!("{}{name}.", self.prefix),
            fields.clone(),
            strukt.columns(),
            strukt.len(),
            strukt.nulls().cloned(),
            self.budget.clone(),
        )
        .map(Some)
    }

    fn primitive<T: ArrowPrimitiveType>(
        &self,
        name: &str,
        i: usize,
        expected: &str,
    ) -> Result<Option<T::Native>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        if col.data_type() != &T::DATA_TYPE {
            return Err(self.mismatch(name, expected, col.data_type()));
        }
        if col.is_null(i) {
            return Ok(None);
        }
        Ok(Some(col.as_primitive::<T>().value(i)))
    }

    pub fn u8_at(&self, name: &str, i: usize) -> Result<Option<u8>, OtapError> {
        self.primitive::<UInt8Type>(name, i, "uint8")
    }

    pub fn u32_at(&self, name: &str, i: usize) -> Result<Option<u32>, OtapError> {
        self.primitive::<UInt32Type>(name, i, "uint32")
    }

    pub fn u64_at(&self, name: &str, i: usize) -> Result<Option<u64>, OtapError> {
        self.primitive::<UInt64Type>(name, i, "uint64")
    }

    pub fn i32_at(&self, name: &str, i: usize) -> Result<Option<i32>, OtapError> {
        self.primitive::<Int32Type>(name, i, "int32")
    }

    pub fn i64_at(&self, name: &str, i: usize) -> Result<Option<i64>, OtapError> {
        self.primitive::<Int64Type>(name, i, "int64")
    }

    pub fn f64_at(&self, name: &str, i: usize) -> Result<Option<f64>, OtapError> {
        self.primitive::<Float64Type>(name, i, "float64")
    }

    pub fn bool_at(&self, name: &str, i: usize) -> Result<Option<bool>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        if col.data_type() != &DataType::Boolean {
            return Err(self.mismatch(name, "boolean", col.data_type()));
        }
        Ok((!col.is_null(i)).then(|| col.as_boolean().value(i)))
    }

    pub fn str_at(&self, name: &str, i: usize) -> Result<Option<&str>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        if col.is_null(i) {
            return Ok(None);
        }
        match col.data_type() {
            DataType::Utf8 => Ok(Some(col.as_string::<i32>().value(i))),
            DataType::LargeUtf8 => Ok(Some(col.as_string::<i64>().value(i))),
            other => Err(self.mismatch(name, "string", other)),
        }
    }

    /// A string column rendered as an owned `String` (`""` when null/absent).
    pub fn string_at(&self, name: &str, i: usize) -> Result<String, OtapError> {
        Ok(self.str_at(name, i)?.unwrap_or_default().to_string())
    }

    pub fn bytes_at(&self, name: &str, i: usize) -> Result<Option<&[u8]>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        if col.is_null(i) {
            return Ok(None);
        }
        match col.data_type() {
            DataType::Binary => Ok(Some(col.as_binary::<i32>().value(i))),
            DataType::LargeBinary => Ok(Some(col.as_binary::<i64>().value(i))),
            DataType::FixedSizeBinary(_) => Ok(Some(col.as_fixed_size_binary().value(i))),
            other => Err(self.mismatch(name, "binary", other)),
        }
    }

    /// A binary column as an owned vector (empty when null/absent).
    pub fn vec_at(&self, name: &str, i: usize) -> Result<Vec<u8>, OtapError> {
        Ok(self
            .bytes_at(name, i)?
            .map(<[u8]>::to_vec)
            .unwrap_or_default())
    }

    /// A nanosecond timestamp column as unsigned epoch nanoseconds
    /// (`0` when null/absent, like an unset OTLP timestamp).
    pub fn timestamp_at(&self, name: &str, i: usize) -> Result<u64, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(0);
        };
        if col.is_null(i) {
            return Ok(0);
        }
        let nanos = match col.data_type() {
            DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                col.as_primitive::<TimestampNanosecondType>().value(i)
            }
            DataType::Int64 => col.as_primitive::<Int64Type>().value(i),
            DataType::UInt64 => return Ok(col.as_primitive::<UInt64Type>().value(i)),
            other => return Err(self.mismatch(name, "timestamp[ns]", other)),
        };
        Ok(u64::try_from(nanos).unwrap_or(0))
    }

    /// A nanosecond duration column (`0` when null/absent).
    pub fn duration_at(&self, name: &str, i: usize) -> Result<u64, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(0);
        };
        if col.is_null(i) {
            return Ok(0);
        }
        let nanos = match col.data_type() {
            DataType::Duration(TimeUnit::Nanosecond) => {
                col.as_primitive::<DurationNanosecondType>().value(i)
            }
            DataType::Int64 => col.as_primitive::<Int64Type>().value(i),
            DataType::UInt64 => return Ok(col.as_primitive::<UInt64Type>().value(i)),
            other => return Err(self.mismatch(name, "duration[ns]", other)),
        };
        Ok(u64::try_from(nanos).unwrap_or(0))
    }

    fn list_at(&self, name: &str, i: usize) -> Result<Option<ArrayRef>, OtapError> {
        let Some((_, col)) = self.column(name) else {
            return Ok(None);
        };
        if col.is_null(i) {
            return Ok(None);
        }
        let values = match col.data_type() {
            DataType::List(_) => col.as_list::<i32>().value(i),
            DataType::LargeList(_) => col.as_list::<i64>().value(i),
            other => return Err(self.mismatch(name, "list", other)),
        };
        Ok(Some(values))
    }

    pub fn u64_list_at(&self, name: &str, i: usize) -> Result<Vec<u64>, OtapError> {
        let Some(values) = self.list_at(name, i)? else {
            return Ok(Vec::new());
        };
        let Some(array) = values.as_primitive_opt::<UInt64Type>() else {
            return Err(self.mismatch(name, "list<uint64>", values.data_type()));
        };
        Ok(array.iter().map(Option::unwrap_or_default).collect())
    }

    pub fn f64_list_at(&self, name: &str, i: usize) -> Result<Vec<f64>, OtapError> {
        let Some(values) = self.list_at(name, i)? else {
            return Ok(Vec::new());
        };
        let Some(array) = values.as_primitive_opt::<Float64Type>() else {
            return Err(self.mismatch(name, "list<float64>", values.data_type()));
        };
        Ok(array.iter().map(Option::unwrap_or_default).collect())
    }

    /// Read a `uint16`/`uint32` id column without decoding (`None` when absent).
    pub fn id_column(&self, name: &str) -> Result<Option<IdColumn>, OtapError> {
        let Some((field, col)) = self.column(name) else {
            return Ok(None);
        };
        self.budget.check_deadline()?;
        let encoded = field.metadata().get(ENCODING).map(String::as_str) != Some(PLAIN);
        let (values, mask): (Vec<Option<u32>>, u32) = match col.data_type() {
            DataType::UInt16 => (
                col.as_primitive::<UInt16Type>()
                    .iter()
                    .map(|v| v.map(u32::from))
                    .collect(),
                u32::from(u16::MAX),
            ),
            DataType::UInt32 => (col.as_primitive::<UInt32Type>().iter().collect(), u32::MAX),
            other => return Err(self.mismatch(name, "uint16 or uint32", other)),
        };
        Ok(Some(IdColumn {
            values,
            encoded,
            mask,
        }))
    }

    /// Delta-decoded ids, one per row (all `None` when the column is absent).
    pub fn ids(&self, name: &str) -> Result<Vec<Option<u32>>, OtapError> {
        Ok(self
            .id_column(name)?
            .map_or_else(|| vec![None; self.len], |c| c.decoded()))
    }

    /// [`Self::ids`] for a column other tables join on: an id may name one
    /// row only.
    pub fn unique_ids(&self, name: &str) -> Result<Vec<Option<u32>>, OtapError> {
        let ids = self.ids(name)?;
        let mut seen = HashSet::with_capacity(ids.len());
        match ids.iter().flatten().find(|id| !seen.insert(**id)) {
            Some(id) => Err(self.err(name, format!("duplicate id {id}"))),
            None => Ok(ids),
        }
    }

    /// The `parent_id` column every child table must carry.
    pub fn parents(&self) -> Result<IdColumn, OtapError> {
        self.id_column(PARENT_ID)?
            .ok_or_else(|| self.err(PARENT_ID, "column is required".into()))
    }

    /// The error for `count` rows whose parent row is not in the batch.
    pub fn orphans(&self, count: usize) -> OtapError {
        orphans(self.name, count)
    }

    /// An error about one row's value in `column`.
    pub fn row_error(&self, column: &str, row: usize, reason: &str) -> OtapError {
        self.err(column, format!("row {row}: {reason}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{BooleanArray, DictionaryArray, FixedSizeBinaryArray, UInt8Array};
    use arrow::array::{StringArray, UInt16Array, UInt32Array};
    use arrow::datatypes::{Field, Schema};
    use std::collections::HashMap;

    fn budget() -> Arc<Budget> {
        Arc::new(Budget::new(crate::otap::limits::OtapLimits::default()))
    }

    fn id_field(name: &str, data_type: DataType, encoding: Option<&str>) -> Field {
        let mut field = Field::new(name, data_type, true);
        if let Some(enc) = encoding {
            field.set_metadata(HashMap::from([(ENCODING.to_string(), enc.to_string())]));
        }
        field
    }

    #[test]
    fn delta_ids_sum_over_non_nulls_and_plain_ids_pass_through() {
        let schema = Schema::new(vec![
            id_field("id", DataType::UInt16, None),
            id_field("plain", DataType::UInt16, Some(PLAIN)),
            id_field("wide", DataType::UInt32, Some("delta")),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(UInt16Array::from(vec![Some(3), None, Some(2), Some(0)])),
                Arc::new(UInt16Array::from(vec![Some(3), None, Some(2), Some(0)])),
                Arc::new(UInt32Array::from(vec![Some(1), Some(1), Some(1), Some(1)])),
            ],
        )
        .unwrap();
        let t = Table::new("T", &batch, &budget()).unwrap();
        assert_eq!(t.ids("id").unwrap(), vec![Some(3), None, Some(5), Some(5)]);
        assert_eq!(
            t.ids("plain").unwrap(),
            vec![Some(3), None, Some(2), Some(0)]
        );
        assert_eq!(
            t.ids("wide").unwrap(),
            vec![Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(t.ids("missing").unwrap(), vec![None; 4]);
    }

    #[test]
    fn a_field_name_in_an_error_is_cleaned() {
        let name = "evil\nWARN forged\" level=\"ERROR";
        let items =
            arrow::array::ListArray::from_iter_primitive::<arrow::datatypes::UInt8Type, _, _>([
                Some([Some(1u8)]),
            ]);
        let keys = UInt8Array::from_iter_values([0]);
        let column: ArrayRef = Arc::new(DictionaryArray::new(keys, Arc::new(items)));
        let schema = Schema::new(vec![Field::new(name, column.data_type().clone(), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![column]).unwrap();
        let Err(err) = Table::new("T", &batch, &budget()) else {
            panic!("a dictionary of lists is not supported");
        };
        assert!(
            matches!(&err, OtapError::Column { column, .. }
                if column == "evil\\nWARN forged\\\" level=\\\"ERROR"),
            "{err}"
        );
        assert!(!err.to_string().chars().any(char::is_control), "{err:?}");
    }

    #[test]
    fn dictionary_columns_are_materialised() {
        let values = StringArray::from(vec!["a", "b", "a"]);
        let dict = arrow::compute::cast(
            &(Arc::new(values) as ArrayRef),
            &DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Utf8)),
        )
        .unwrap();
        let schema = Schema::new(vec![Field::new("name", dict.data_type().clone(), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![dict]).unwrap();
        let t = Table::new("T", &batch, &budget()).unwrap();
        assert_eq!(t.str_at("name", 2).unwrap(), Some("a"));
        assert_eq!(t.string_at("absent", 0).unwrap(), "");
    }

    /// Keys `[1, null, 0, 1]` into `values`, as a one-column table.
    fn keyed(values: ArrayRef) -> RecordBatch {
        let keys = UInt8Array::from(vec![Some(1), None, Some(0), Some(1)]);
        let column: ArrayRef = Arc::new(DictionaryArray::new(keys, values));
        let schema = Schema::new(vec![Field::new("c", column.data_type().clone(), true)]);
        RecordBatch::try_new(Arc::new(schema), vec![column]).unwrap()
    }

    #[test]
    fn every_materialised_row_is_a_tick_of_the_deadline() {
        let budget = budget();
        let text: ArrayRef = Arc::new(StringArray::from(vec!["x", "yy"]));
        let t = Table::new("T", &keyed(text), &budget).unwrap();
        assert_eq!(budget.ticks(), 4, "one look per row");
        let read: Vec<_> = (0..4).map(|i| t.str_at("c", i).unwrap()).collect();
        assert_eq!(read, [Some("yy"), None, Some("x"), Some("yy")]);
    }

    #[test]
    fn every_value_type_is_materialised_with_its_nulls() {
        let numbers: ArrayRef = Arc::new(UInt32Array::from(vec![Some(7), None]));
        let t = Table::new("T", &keyed(numbers), &budget()).unwrap();
        let read: Vec<_> = (0..4).map(|i| t.u32_at("c", i).unwrap()).collect();
        assert_eq!(read, [None, None, Some(7), None], "a null value stays null");

        let flags: ArrayRef = Arc::new(BooleanArray::from(vec![false, true]));
        let t = Table::new("T", &keyed(flags), &budget()).unwrap();
        let read: Vec<_> = (0..4).map(|i| t.bool_at("c", i).unwrap()).collect();
        assert_eq!(read, [Some(true), None, Some(false), Some(true)]);

        let ids = [[1u8; 4].as_slice(), [2u8; 4].as_slice()];
        let ids: ArrayRef = Arc::new(FixedSizeBinaryArray::try_from_iter(ids.into_iter()).unwrap());
        let t = Table::new("T", &keyed(ids), &budget()).unwrap();
        let read: Vec<_> = (0..4).map(|i| t.bytes_at("c", i).unwrap()).collect();
        let (one, two) = ([1u8; 4].as_slice(), [2u8; 4].as_slice());
        assert_eq!(read, [Some(two), None, Some(one), Some(two)]);

        // Keys with nothing to point at: no row has a value.
        let keys = UInt8Array::from(vec![None, None]);
        let none: ArrayRef = Arc::new(StringArray::from(Vec::<&str>::new()));
        let column: ArrayRef = Arc::new(DictionaryArray::new(keys, none));
        let schema = Schema::new(vec![Field::new("c", column.data_type().clone(), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![column]).unwrap();
        let t = Table::new("T", &batch, &budget()).unwrap();
        assert_eq!(t.str_at("c", 0).unwrap(), None);
        assert_eq!(t.str_at("c", 1).unwrap(), None);
    }

    #[test]
    fn wrong_column_type_is_an_error() {
        let schema = Schema::new(vec![Field::new("kind", DataType::Utf8, true)]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(vec!["x"]))],
        )
        .unwrap();
        let t = Table::new("SPANS", &batch, &budget()).unwrap();
        let err = t.i32_at("kind", 0).unwrap_err().to_string();
        assert!(
            err.contains("SPANS.kind") && err.contains("expected int32"),
            "{err}"
        );
    }
}
