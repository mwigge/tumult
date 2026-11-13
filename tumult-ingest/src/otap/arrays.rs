//! Typed, null-aware access to the columns of one OTAP record batch.
//!
//! Dictionary-encoded columns are materialised up front (the producer
//! chooses dictionary encoding per column and per batch), so every accessor
//! sees plain arrays. Id columns honour the `encoding` field metadata: absent
//! or `delta` means cumulative-sum decoding, `plain` means values as sent.

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, DurationNanosecondType, FieldRef, Fields, Float64Type, Int32Type,
    Int64Type, TimeUnit, TimestampNanosecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};

use super::OtapError;

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
    len: usize,
    validity: Option<NullBuffer>,
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
}

fn materialise(array: &ArrayRef) -> Result<ArrayRef, arrow::error::ArrowError> {
    match array.data_type() {
        DataType::Dictionary(_, value_type) => arrow::compute::cast(array, value_type),
        _ => Ok(array.clone()),
    }
}

impl Table {
    pub fn new(name: &'static str, batch: &RecordBatch) -> Result<Self, OtapError> {
        Self::build(
            name,
            String::new(),
            batch.schema().fields().clone(),
            batch.columns(),
            batch.num_rows(),
            None,
        )
    }

    fn build(
        name: &'static str,
        prefix: String,
        fields: Fields,
        source: &[ArrayRef],
        len: usize,
        validity: Option<NullBuffer>,
    ) -> Result<Self, OtapError> {
        let mut columns = Vec::with_capacity(source.len());
        for (field, column) in fields.iter().zip(source) {
            columns.push(materialise(column).map_err(|e| OtapError::Column {
                table: name,
                column: format!("{prefix}{}", field.name()),
                reason: format!("cannot materialise dictionary: {e}"),
            })?);
        }
        Ok(Self {
            name,
            prefix,
            fields,
            columns,
            len,
            validity,
        })
    }

    pub fn len(&self) -> usize {
        self.len
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
        self.err(column, format!("expected {expected}, found {found}"))
    }

    fn column(&self, name: &str) -> Option<(&FieldRef, &ArrayRef)> {
        self.fields
            .iter()
            .position(|f| f.name() == name)
            .map(|i| (&self.fields[i], &self.columns[i]))
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
        materialise(&values)
            .map(Some)
            .map_err(|e| self.err(name, e.to_string()))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{StringArray, UInt16Array, UInt32Array};
    use arrow::datatypes::{Field, Schema};
    use std::collections::HashMap;
    use std::sync::Arc;

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
        let t = Table::new("T", &batch).unwrap();
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
    fn dictionary_columns_are_materialised() {
        let values = StringArray::from(vec!["a", "b", "a"]);
        let dict = arrow::compute::cast(
            &(Arc::new(values) as ArrayRef),
            &DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Utf8)),
        )
        .unwrap();
        let schema = Schema::new(vec![Field::new("name", dict.data_type().clone(), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![dict]).unwrap();
        let t = Table::new("T", &batch).unwrap();
        assert_eq!(t.str_at("name", 2).unwrap(), Some("a"));
        assert_eq!(t.string_at("absent", 0).unwrap(), "");
    }

    #[test]
    fn wrong_column_type_is_an_error() {
        let schema = Schema::new(vec![Field::new("kind", DataType::Utf8, true)]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(vec!["x"]))],
        )
        .unwrap();
        let t = Table::new("SPANS", &batch).unwrap();
        let err = t.i32_at("kind", 0).unwrap_err().to_string();
        assert!(
            err.contains("SPANS.kind") && err.contains("expected int32"),
            "{err}"
        );
    }
}
