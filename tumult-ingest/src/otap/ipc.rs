//! A strict decoder for the Arrow IPC messages of an OTAP payload.
//!
//! The daemon's release profile aborts on panic, and the general-purpose
//! reader in `arrow-ipc` trusts what a message declares in many places: a
//! peer could end the process with one malformed message. That reader is
//! therefore not used at all. This module decodes the small subset of Arrow
//! the OTAP tables are made of and checks every declared number against the
//! bytes that back it before an array is built.
//!
//! What is accepted:
//!
//! * little-endian schemas of at most [`MAX_FIELDS_PER_SCHEMA`] fields, at
//!   most [`MAX_SCHEMA_DEPTH`] levels deep, sibling names unique and at most
//!   [`MAX_FIELD_NAME_BYTES`] long;
//! * `Bool`, `Int8`..`Int64`, `UInt8`..`UInt64`, `Float32`, `Float64`,
//!   `Utf8`, `Binary`, `FixedSizeBinary` (1 to 64 bytes), `Timestamp` and
//!   `Duration` in nanoseconds, `Struct`, `List` of a primitive or a struct,
//!   and dictionary encoding (`UInt8`/`UInt16`/`UInt32` keys) of the
//!   non-nested types, at top level or inside a struct;
//! * record and dictionary batches whose node and buffer counts are exactly
//!   what the schema implies, uncompressed or zstd-compressed.
//!
//! Everything else is refused as malformed: unions, run-end encoding, views,
//! maps, large and fixed-size lists, large strings, decimals, dates, null
//! columns, dictionaries of nested types, variadic buffer counts.
//!
//! # Panic sites of `arrow-ipc` 58.3.0 and what stands in for each
//!
//! None of `read_record_batch`, `read_dictionary`, `StreamReader` and
//! `fb_to_schema` is called, so none of the sites below is reachable. The
//! right column names the check in this module that covers the same input.
//!
//! | Site in `arrow-ipc` 58.3.0 | Trigger | Check here |
//! |---|---|---|
//! | `reader.rs:66` `read_buffer` → `Buffer::slice_with_length` assert (`arrow-buffer` `immutable.rs:299`) | buffer offset/length outside the body, uncompressed or not | `Reader::range`: every buffer is taken with `slice::get`, "buffer outside the message body" |
//! | `reader.rs:192` `unreachable!()` | run-end encoded column with an odd run type | `Planner::data_type`: `RunEndEncoded` is not on the allowlist |
//! | `reader.rs:210`, `:215` union type-id/offset `slice_with_length` (`arrow-buffer` `boolean.rs:128` behind it) | union column whose buffers are shorter than its node length | `Planner::data_type`: `Union` is not on the allowlist |
//! | `reader.rs:271`, `:318`, `:350`, `:425` `buffers[0]`; `:288`, `:324`, `:357`, `:358`, `:428` `buffers[1]`/`[2]` | fewer buffers than the type needs | `Reader::open`: buffer count must equal the schema's; `Reader::buffer` returns an error when the list ends |
//! | `reader.rs:291` `unreachable!("Data type … not primitive")` | type the primitive builder does not know | allowlist: only fixed-width types with a known width reach `Reader::fixed` |
//! | `reader.rs:333` `unreachable!("Cannot create list or map array")` | list-like type outside `List`/`LargeList`/`Map` | allowlist: only `List` reaches `Reader::list` |
//! | `reader.rs:348` `assert!(matches!(… ListView …))` | list view | allowlist |
//! | `reader.rs:434` `unreachable!("Cannot create dictionary array")` | dictionary field whose type is not a dictionary | `Planner::field` builds the dictionary type itself from the checked key type |
//! | `reader.rs:587`, `:608` `assert!(variadic_counts.is_empty())` | `variadicBufferCounts` without view columns | `Reader::open`: "variadic buffer counts are not supported" |
//! | `reader.rs:627` `self.buffers.next().unwrap()` | fewer buffers than the schema needs | `Reader::open` count check, `Reader::buffer` |
//! | `reader.rs:885` `batch.data().unwrap()` | dictionary batch without data | `dictionary`: "dictionary batch without data" |
//! | `reader.rs:894` `record_batch.column(0)` | dictionary batch that decodes to no column | `dictionary` decodes exactly one node of the dictionary's value type |
//! | `convert.rs:198` `fb.fields().unwrap()` | schema without a field vector | `plan`: "schema without fields" |
//! | `convert.rs:204` `unimplemented!` | big-endian schema with decimals | `plan`: only little-endian schemas |
//! | `convert.rs:299` `indexType().unwrap()`, `:309` bit-width panic | dictionary without or with an odd index type | `Planner::keys` |
//! | `convert.rs:322` `type_as_int().unwrap()`, `:332` int width panic | `Int` type without a table or with a width other than 8/16/32/64 | `Planner::int` |
//! | `convert.rs:345` fixed-size-binary unwrap | type table missing | `Planner::data_type` (`let … else`) |
//! | `convert.rs:349`, `:354` float precision panic | unknown precision | `Planner::data_type`: `SINGLE`/`DOUBLE` only |
//! | `convert.rs:358`-`:411` date/time/timestamp/interval/duration unit panics | unknown units | allowlist: nanosecond `Timestamp`/`Duration` only, others refused |
//! | `convert.rs:415`-`:473` `children().unwrap()`, "expect a list to have one child" | list or struct without children | `Planner::list`, `Planner::members` |
//! | `convert.rs:478`-`:487` decimal, `:491`-`:509` union | malformed decimal/union tables | allowlist |
//! | `convert.rs:514` `unimplemented!("Type … not supported")` | type id unknown to this arrow version | `Planner::data_type` default arm |
//! | `convert.rs:546`, `:549` `build_field` | (writer side) | not called |
//! | `compression.rs:78` `Decompressor::new().expect`, `:338` `try_into().unwrap()` | compressed buffer shorter than its length prefix | `Reader::inflate`: "compressed buffer without length prefix", decompressor errors are returned |
//!
//! Functions of `arrow` that are still called, and why they cannot panic on
//! peer input here:
//!
//! | Call | Its assertion | Guaranteed by |
//! |---|---|---|
//! | `arrow::ipc::root_as_message` | none (runs the flatbuffers verifier) | errors are returned |
//! | flatbuffers `Vector::get` | index in range | never called; vectors are only iterated |
//! | `BooleanBuffer::new` | buffer holds `len` bits | `Reader::validity` loads exactly `ceil(len / 8)` bytes first |
//! | `ArrayDataBuilder::build` | none: `validate_data` returns errors (layout, offsets, UTF-8, dictionary keys) | every buffer is already sized and bounds-checked by this module; errors become [`OtapError::Ipc`] |
//! | `make_array` | buffer count and alignment of valid `ArrayData` | data comes from `build`, buffers are 64-byte aligned `MutableBuffer`s |
//! | `RecordBatch::try_new_with_options` | none: returns errors | column types are the plan's own |
//! | `arrow::compute::concat` (dictionary deltas) | same type on both sides | both sides are decoded with the same plan node |
//! | `MutableArrayData::extend` over a dictionary column (`arrays.rs`) | the copied range lies inside the values | `Reader::keys`: every key, null slots included, is inside the dictionary; `arrays.rs` copies nothing for a null key and takes its keys from `normalized_keys`, which clamps them to the values |
//!
//! # What a dictionary costs per batch
//!
//! The values of a dictionary are validated in full (offsets, UTF-8) once,
//! when the dictionary batch or delta that carries them is decoded:
//! `Reader::array` ends in `ArrayDataBuilder::build`. A record batch that
//! refers to the dictionary does not walk the values again. Its key column
//! is built by the same checked `build`, which for a dictionary type
//! validates the keys against the dictionary's length and, for the child,
//! only the layout (`ArrayData::validate`: buffer counts and sizes, first
//! and last offset), never the bytes: `validate_data` does not recurse,
//! only `validate_full` does. The values handed to it are the retained
//! array itself, immutable behind its `Arc`, or the `concat` of two such
//! arrays, so they are exactly what was validated on arrival. The work per
//! batch is therefore one pass over the keys, which counts against the
//! batch's deadline (`Budget::tick`), whatever the size of the dictionary.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{make_array, new_null_array, Array, ArrayData, ArrayRef, RecordBatch};
use arrow::array::{ArrayDataBuilder, RecordBatchOptions};
use arrow::buffer::{BooleanBuffer, Buffer, MutableBuffer, NullBuffer};
use arrow::datatypes::{DataType, Field, Fields, Schema, SchemaRef, TimeUnit};
use arrow::ipc;

use super::limits::{
    Budget, MAX_DICTIONARIES_PER_SCHEMA, MAX_FIELDS_PER_SCHEMA, MAX_FIELD_NAME_BYTES,
    MAX_SCHEMA_DEPTH,
};
use super::OtapError;

/// Marks the start of every IPC message (the encapsulated message format).
const CONTINUATION: [u8; 4] = [0xff; 4];

/// Size of the uncompressed-length prefix of a compressed IPC buffer.
const LENGTH_PREFIX: usize = 8;

/// Prefix value of a buffer stored uncompressed in a compressed body.
const NOT_COMPRESSED: i64 = -1;

/// Widest `FixedSizeBinary` accepted (trace ids are 16 bytes).
const MAX_FIXED_WIDTH: i32 = 64;

/// Metadata entries looked at per field; the exporter writes at most one.
const MAX_FIELD_METADATA: usize = 16;

/// Field metadata key naming the column's id encoding, and the one value
/// that is kept (everything else means "delta encoded", like its absence).
const ENCODING: &str = "encoding";
const PLAIN: &str = "plain";

/// Bytes a field is assumed to occupy in a retained plan, besides its name.
const FIELD_OVERHEAD: usize = 160;

fn malformed(payload: &'static str, reason: impl Into<String>) -> OtapError {
    OtapError::Malformed {
        payload,
        reason: reason.into(),
    }
}

/// One encapsulated message and what follows it in the payload.
pub(super) struct Framed<'a> {
    pub message: ipc::Message<'a>,
    pub body: &'a [u8],
    pub rest: &'a [u8],
}

/// Split the first message off `bytes`: its metadata and body must lie
/// within the payload.
pub(super) fn frame<'a>(payload: &'static str, bytes: &'a [u8]) -> Result<Framed<'a>, OtapError> {
    let Some((prefix, after_prefix)) = bytes.split_first_chunk::<8>() else {
        return Err(malformed(payload, "truncated message prefix"));
    };
    if prefix[..4] != CONTINUATION {
        return Err(malformed(payload, "missing continuation marker"));
    }
    let declared = i32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
    let meta_len = usize::try_from(declared)
        .ok()
        .filter(|&n| n > 0 && n <= after_prefix.len())
        .ok_or_else(|| {
            malformed(
                payload,
                format!("metadata length {declared} is not in the payload"),
            )
        })?;
    let (meta, after_meta) = after_prefix.split_at(meta_len);
    let message =
        ipc::root_as_message(meta).map_err(|_| malformed(payload, "invalid message metadata"))?;
    let version = message.version();
    if version != ipc::MetadataVersion::V5 && version != ipc::MetadataVersion::V4 {
        return Err(malformed(payload, "unsupported IPC metadata version"));
    }
    let body_len = usize::try_from(message.bodyLength())
        .ok()
        .filter(|&n| n <= after_meta.len())
        .ok_or_else(|| {
            malformed(
                payload,
                format!("body length {} is not in the payload", message.bodyLength()),
            )
        })?;
    let (body, rest) = after_meta.split_at(body_len);
    Ok(Framed {
        message,
        body,
        rest,
    })
}

/// How the buffers of one column are laid out.
#[derive(Debug)]
enum Shape {
    Bool,
    /// Fixed-width values of this many bytes.
    Fixed(usize),
    /// 32-bit offsets into a values buffer (`Utf8`, `Binary`).
    Bytes,
    Struct(Vec<Node>),
    List(Box<Node>),
}

/// One field of a checked schema.
#[derive(Debug)]
pub(super) struct Node {
    data_type: DataType,
    shape: Shape,
    /// The dictionary id when the column holds dictionary keys.
    dictionary: Option<i64>,
}

impl Node {
    /// Field nodes and buffers the column takes in a batch.
    fn counts(&self) -> (usize, usize) {
        match &self.shape {
            Shape::Bool | Shape::Fixed(_) => (1, 2),
            Shape::Bytes => (1, 3),
            Shape::Struct(members) => members.iter().map(Node::counts).fold((1, 1), sum),
            Shape::List(item) => sum((1, 2), item.counts()),
        }
    }
}

fn sum(a: (usize, usize), b: (usize, usize)) -> (usize, usize) {
    (a.0 + b.0, a.1 + b.1)
}

/// A schema message that passed the allowlist, ready to decode batches.
#[derive(Debug)]
pub(super) struct Plan {
    schema: SchemaRef,
    columns: Vec<Node>,
    /// Value layout per dictionary id.
    dictionaries: HashMap<i64, Node>,
    /// Field nodes and buffers of one record batch.
    counts: (usize, usize),
    /// Bytes the plan is assumed to occupy while its stream is open.
    retained: usize,
}

impl Plan {
    pub fn retained(&self) -> usize {
        self.retained
    }
}

struct Planner {
    payload: &'static str,
    fields: usize,
    retained: usize,
    dictionaries: HashMap<i64, Node>,
}

/// Check a schema message against the allowlist.
pub(super) fn plan(payload: &'static str, schema: ipc::Schema<'_>) -> Result<Plan, OtapError> {
    if schema.endianness() != ipc::Endianness::Little {
        return Err(malformed(
            payload,
            "only little-endian schemas are supported",
        ));
    }
    let Some(declared) = schema.fields().filter(|fields| !fields.is_empty()) else {
        return Err(malformed(payload, "schema without fields"));
    };
    let mut planner = Planner {
        payload,
        fields: 0,
        retained: 0,
        dictionaries: HashMap::new(),
    };
    let (fields, columns) = planner.members(declared.iter(), 1)?;
    let counts = columns.iter().map(Node::counts).fold((0, 0), sum);
    Ok(Plan {
        schema: Arc::new(Schema::new(fields)),
        columns,
        dictionaries: planner.dictionaries,
        counts,
        retained: planner.retained,
    })
}

impl Planner {
    fn fail(&self, reason: impl Into<String>) -> OtapError {
        malformed(self.payload, reason)
    }

    fn unsupported(&self, what: &str) -> OtapError {
        self.fail(format!("unsupported column type {what}"))
    }

    /// The sibling fields of one level: a schema's columns or a struct's
    /// members.
    fn members<'a>(
        &mut self,
        declared: impl Iterator<Item = ipc::Field<'a>>,
        depth: usize,
    ) -> Result<(Vec<Field>, Vec<Node>), OtapError> {
        let mut names = HashSet::new();
        let (mut fields, mut nodes) = (Vec::new(), Vec::new());
        for field in declared {
            let (field, node) = self.field(field, depth)?;
            if !names.insert(field.name().clone()) {
                return Err(self.fail("duplicate field name"));
            }
            fields.push(field);
            nodes.push(node);
        }
        Ok((fields, nodes))
    }

    fn field(&mut self, field: ipc::Field<'_>, depth: usize) -> Result<(Field, Node), OtapError> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err(self.fail(format!("fields nested deeper than {MAX_SCHEMA_DEPTH}")));
        }
        self.fields += 1;
        if self.fields > MAX_FIELDS_PER_SCHEMA {
            return Err(self.fail(format!("more than {MAX_FIELDS_PER_SCHEMA} fields")));
        }
        let name = field.name().unwrap_or_default();
        if name.len() > MAX_FIELD_NAME_BYTES {
            // The name itself is never repeated: it is the peer's text.
            return Err(self.fail(format!(
                "field name longer than {MAX_FIELD_NAME_BYTES} bytes"
            )));
        }
        self.retained = self.retained.saturating_add(name.len() + FIELD_OVERHEAD);
        let (data_type, shape) = self.data_type(field, depth)?;
        let mut node = Node {
            data_type,
            shape,
            dictionary: None,
        };
        if let Some(encoding) = field.dictionary() {
            node = self.keys(encoding, node)?;
        }
        let mut out = Field::new(name, node.data_type.clone(), true);
        if self.is_plain(field)? {
            out.set_metadata(HashMap::from([(ENCODING.to_string(), PLAIN.to_string())]));
        }
        Ok((out, node))
    }

    /// Whether the field's `encoding` metadata says `plain` (the last entry
    /// wins, as in a map).
    fn is_plain(&self, field: ipc::Field<'_>) -> Result<bool, OtapError> {
        let Some(entries) = field.custom_metadata() else {
            return Ok(false);
        };
        if entries.len() > MAX_FIELD_METADATA {
            return Err(self.fail(format!(
                "more than {MAX_FIELD_METADATA} metadata entries on a field"
            )));
        }
        Ok(entries
            .iter()
            .rfind(|entry| entry.key() == Some(ENCODING))
            .is_some_and(|entry| entry.value() == Some(PLAIN)))
    }

    /// Turn `values` into the column of keys into dictionary `encoding`.
    fn keys(
        &mut self,
        encoding: ipc::DictionaryEncoding<'_>,
        values: Node,
    ) -> Result<Node, OtapError> {
        if matches!(values.shape, Shape::Struct(_) | Shape::List(_)) {
            return Err(self.unsupported("dictionary of a nested type"));
        }
        if encoding.dictionaryKind() != ipc::DictionaryKind::DenseArray {
            return Err(self.unsupported("dictionary kind"));
        }
        let index = encoding.indexType().map(|i| (i.is_signed(), i.bitWidth()));
        let (key_type, width) = match index {
            Some((false, 8)) => (DataType::UInt8, 1),
            Some((false, 16)) => (DataType::UInt16, 2),
            Some((false, 32)) => (DataType::UInt32, 4),
            _ => return Err(self.unsupported("dictionary index")),
        };
        if self.dictionaries.len() >= MAX_DICTIONARIES_PER_SCHEMA {
            return Err(self.fail(format!(
                "more than {MAX_DICTIONARIES_PER_SCHEMA} dictionary columns"
            )));
        }
        let id = encoding.id();
        let data_type =
            DataType::Dictionary(Box::new(key_type), Box::new(values.data_type.clone()));
        if self.dictionaries.insert(id, values).is_some() {
            return Err(self.fail("duplicate dictionary id"));
        }
        Ok(Node {
            data_type,
            shape: Shape::Fixed(width),
            dictionary: Some(id),
        })
    }

    fn data_type(
        &mut self,
        field: ipc::Field<'_>,
        depth: usize,
    ) -> Result<(DataType, Shape), OtapError> {
        let kind = field.type_type();
        Ok(match kind {
            ipc::Type::Bool => (DataType::Boolean, Shape::Bool),
            ipc::Type::Int => self.int(field)?,
            ipc::Type::FloatingPoint => match field.type_as_floating_point().map(|f| f.precision())
            {
                Some(ipc::Precision::SINGLE) => (DataType::Float32, Shape::Fixed(4)),
                Some(ipc::Precision::DOUBLE) => (DataType::Float64, Shape::Fixed(8)),
                _ => return Err(self.unsupported("float precision")),
            },
            ipc::Type::Utf8 => (DataType::Utf8, Shape::Bytes),
            ipc::Type::Binary => (DataType::Binary, Shape::Bytes),
            ipc::Type::FixedSizeBinary => {
                let width = field.type_as_fixed_size_binary().map(|f| f.byteWidth());
                let Some(width) = width.filter(|w| (1..=MAX_FIXED_WIDTH).contains(w)) else {
                    return Err(self.unsupported("fixed-size binary width"));
                };
                let bytes = usize::try_from(width).unwrap_or(1);
                (DataType::FixedSizeBinary(width), Shape::Fixed(bytes))
            }
            // The timezone is the peer's text and no decoder reads it.
            ipc::Type::Timestamp => match field.type_as_timestamp().map(|t| t.unit()) {
                Some(ipc::TimeUnit::NANOSECOND) => (
                    DataType::Timestamp(TimeUnit::Nanosecond, None),
                    Shape::Fixed(8),
                ),
                _ => return Err(self.unsupported("timestamp unit")),
            },
            ipc::Type::Duration => match field.type_as_duration().map(|d| d.unit()) {
                Some(ipc::TimeUnit::NANOSECOND) => {
                    (DataType::Duration(TimeUnit::Nanosecond), Shape::Fixed(8))
                }
                _ => return Err(self.unsupported("duration unit")),
            },
            ipc::Type::Struct_ => {
                let Some(children) = field.children().filter(|c| !c.is_empty()) else {
                    return Err(self.unsupported("struct without fields"));
                };
                let (fields, nodes) = self.members(children.iter(), depth + 1)?;
                (DataType::Struct(Fields::from(fields)), Shape::Struct(nodes))
            }
            ipc::Type::List => self.list(field, depth)?,
            other => return Err(self.unsupported(other.variant_name().unwrap_or("unknown"))),
        })
    }

    fn int(&self, field: ipc::Field<'_>) -> Result<(DataType, Shape), OtapError> {
        let int = field.type_as_int().map(|i| (i.is_signed(), i.bitWidth()));
        Ok(match int {
            Some((true, 8)) => (DataType::Int8, Shape::Fixed(1)),
            Some((true, 16)) => (DataType::Int16, Shape::Fixed(2)),
            Some((true, 32)) => (DataType::Int32, Shape::Fixed(4)),
            Some((true, 64)) => (DataType::Int64, Shape::Fixed(8)),
            Some((false, 8)) => (DataType::UInt8, Shape::Fixed(1)),
            Some((false, 16)) => (DataType::UInt16, Shape::Fixed(2)),
            Some((false, 32)) => (DataType::UInt32, Shape::Fixed(4)),
            Some((false, 64)) => (DataType::UInt64, Shape::Fixed(8)),
            _ => return Err(self.unsupported("integer width")),
        })
    }

    fn list(
        &mut self,
        field: ipc::Field<'_>,
        depth: usize,
    ) -> Result<(DataType, Shape), OtapError> {
        let mut children = field.children().into_iter().flatten();
        let (Some(child), None) = (children.next(), children.next()) else {
            return Err(self.unsupported("list without exactly one item field"));
        };
        let (item, node) = self.field(child, depth + 1)?;
        if node.dictionary.is_some() {
            return Err(self.unsupported("dictionary in a list"));
        }
        if matches!(node.shape, Shape::List(_)) {
            return Err(self.unsupported("list of lists"));
        }
        Ok((DataType::List(Arc::new(item)), Shape::List(Box::new(node))))
    }
}

/// The most one zstd block decompresses to (RFC 8878, 3.1.1.2.4).
const ZSTD_BLOCK_MAX: usize = 128 * 1024;

/// The least one zstd block takes of the compressed bytes: its header.
const ZSTD_BLOCK_MIN: usize = 3;

/// Whether `data` can be a zstd buffer of `declared` decompressed bytes,
/// asked before anything is allocated for it: the declared size costs the
/// sender eight bytes, whatever it says.
///
/// The frames of `data` are walked by their headers, nothing is
/// decompressed:
///
/// - When every frame header carries its content size, the sizes must
///   add up to `declared`.
/// - When one does not (a streaming encoder), the sizes that are known
///   must not be more than `declared`, and `declared` not more than the
///   bytes can hold: every [`ZSTD_BLOCK_MAX`] bytes of output take a
///   block, and a block at least [`ZSTD_BLOCK_MIN`] bytes of `data`. So
///   a sender pays for the memory it makes the receiver allocate with
///   bytes on the wire (at the format's best ratio) as well as with its
///   decoded-bytes budget.
///
/// This is a coarse filter: it stops a declared size the bytes cannot
/// back, and at zstd's best ratio that still leaves tens of thousands of
/// decoded bytes for one on the wire. The limit on what a batch may
/// allocate is the decoded-bytes charge (`KRONIKA_OTAP_MAX_DECODED_BYTES`)
/// made before this is asked.
fn could_inflate_to(data: &[u8], declared: usize) -> Result<(), &'static str> {
    const INVALID: &str = "invalid zstd buffer";
    const DIFFERS: &str = "decompressed size differs from the declared size";
    let declared_wide = u64::try_from(declared).unwrap_or(u64::MAX);
    let (mut known, mut all_known) = (0u64, true);
    let mut rest = data;
    while !rest.is_empty() {
        match zstd::zstd_safe::get_frame_content_size(rest) {
            Ok(Some(size)) => known = known.saturating_add(size),
            Ok(None) => all_known = false,
            Err(_) => return Err(INVALID),
        }
        let frame = zstd::zstd_safe::find_frame_compressed_size(rest).map_err(|_| INVALID)?;
        rest = match rest.get(frame..) {
            Some(rest) if frame > 0 => rest,
            _ => return Err(INVALID),
        };
    }
    if known > declared_wide || (all_known && known != declared_wide) {
        return Err(DIFFERS);
    }
    if declared > (data.len() / ZSTD_BLOCK_MIN).saturating_mul(ZSTD_BLOCK_MAX) {
        return Err("declared size is more than the compressed bytes can hold");
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// The largest buffer this thread has allocated to decompress into.
    pub(super) static LARGEST_INFLATED: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Reads the nodes and buffers of one record (or dictionary) batch in
/// schema order.
struct Reader<'a> {
    payload: &'static str,
    body: &'a [u8],
    /// `(length, null_count)` as declared.
    nodes: std::vec::IntoIter<(i64, i64)>,
    /// `(offset, length)` as declared.
    buffers: std::vec::IntoIter<(i64, i64)>,
    compressed: bool,
    zstd: Option<zstd::bulk::Decompressor<'static>>,
    budget: &'a Budget,
    dictionaries: &'a HashMap<i64, ArrayRef>,
}

impl<'a> Reader<'a> {
    /// Check the batch header against the limits and against the node and
    /// buffer counts the schema implies; returns the row count too.
    fn open(
        payload: &'static str,
        header: ipc::RecordBatch<'_>,
        body: &'a [u8],
        counts: (usize, usize),
        budget: &'a Budget,
        dictionaries: &'a HashMap<i64, ArrayRef>,
    ) -> Result<(usize, Self), OtapError> {
        let limits = budget.limits();
        let rows = usize::try_from(header.length())
            .map_err(|_| malformed(payload, format!("negative row count {}", header.length())))?;
        if rows > limits.max_rows_per_table {
            return Err(OtapError::Limit {
                what: "rows per table",
                limit: limits.max_rows_per_table,
            });
        }
        if header.variadicBufferCounts().is_some_and(|v| !v.is_empty()) {
            return Err(malformed(
                payload,
                "variadic buffer counts are not supported",
            ));
        }
        let compressed = match header.compression() {
            None => false,
            Some(c) if c.codec() != ipc::CompressionType::ZSTD => {
                return Err(malformed(payload, "only zstd IPC compression is supported"))
            }
            Some(c) if c.method() != ipc::BodyCompressionMethod::BUFFER => {
                return Err(malformed(payload, "unsupported IPC compression method"))
            }
            Some(_) => true,
        };
        let node_count = header.nodes().map_or(0, |n| n.len());
        let buffer_count = header.buffers().map_or(0, |b| b.len());
        if (node_count, buffer_count) != counts {
            return Err(malformed(
                payload,
                format!(
                    "{node_count} field nodes and {buffer_count} buffers where the schema has \
                     {} and {}",
                    counts.0, counts.1
                ),
            ));
        }
        let nodes: Vec<(i64, i64)> = header
            .nodes()
            .into_iter()
            .flatten()
            .map(|n| (n.length(), n.null_count()))
            .collect();
        let buffers: Vec<(i64, i64)> = header
            .buffers()
            .into_iter()
            .flatten()
            .map(|b| (b.offset(), b.length()))
            .collect();
        let reader = Self {
            payload,
            body,
            nodes: nodes.into_iter(),
            buffers: buffers.into_iter(),
            compressed,
            zstd: None,
            budget,
            dictionaries,
        };
        Ok((rows, reader))
    }

    fn fail(&self, reason: impl Into<String>) -> OtapError {
        malformed(self.payload, reason)
    }

    fn ipc(&self, source: arrow::error::ArrowError) -> OtapError {
        OtapError::Ipc {
            payload: self.payload,
            source,
        }
    }

    /// The next field node: `(length, null_count)`.
    fn node(&mut self) -> Result<(usize, usize), OtapError> {
        self.budget.check_deadline()?;
        let Some((length, nulls)) = self.nodes.next() else {
            return Err(self.fail("fewer field nodes than the schema has fields"));
        };
        let len = usize::try_from(length)
            .map_err(|_| self.fail(format!("negative array length {length}")))?;
        // Every element of an array costs at least a bit of decoded body.
        let limit = self.budget.limits().max_decoded_bytes_per_batch;
        if len > limit {
            return Err(OtapError::Limit {
                what: "array length",
                limit,
            });
        }
        let nulls = usize::try_from(nulls)
            .ok()
            .filter(|&n| n <= len)
            .ok_or_else(|| self.fail("null count outside the array"))?;
        Ok((len, nulls))
    }

    /// The bytes of the next buffer as they lie in the body.
    fn range(&mut self) -> Result<&'a [u8], OtapError> {
        let Some((offset, length)) = self.buffers.next() else {
            return Err(self.fail("fewer buffers than the schema needs"));
        };
        usize::try_from(offset)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(offset, len)| self.body.get(offset..offset.checked_add(len)?))
            .ok_or_else(|| self.fail("buffer outside the message body"))
    }

    /// The first `need` decoded bytes of the next buffer, in aligned memory
    /// of their own.
    fn buffer(&mut self, need: usize) -> Result<MutableBuffer, OtapError> {
        let stored = self.range()?;
        if !self.compressed {
            return self.copy(stored, need);
        }
        if stored.is_empty() {
            return self.copy(stored, need);
        }
        let Some((prefix, data)) = stored.split_first_chunk::<LENGTH_PREFIX>() else {
            return Err(self.fail("compressed buffer without length prefix"));
        };
        let declared = i64::from_le_bytes(*prefix);
        if declared == NOT_COMPRESSED {
            return self.copy(data, need);
        }
        let declared = usize::try_from(declared)
            .map_err(|_| self.fail(format!("invalid uncompressed length {declared}")))?;
        self.inflate(data, declared, need)
    }

    fn copy(&self, stored: &[u8], need: usize) -> Result<MutableBuffer, OtapError> {
        let Some(wanted) = stored.get(..need) else {
            return Err(self.fail("buffer shorter than its array"));
        };
        self.budget.charge_bytes(need)?;
        let mut out = MutableBuffer::with_capacity(need);
        out.extend_from_slice(wanted);
        Ok(out)
    }

    fn inflate(
        &mut self,
        data: &[u8],
        declared: usize,
        need: usize,
    ) -> Result<MutableBuffer, OtapError> {
        if declared < need {
            return Err(self.fail("buffer shorter than its array"));
        }
        // Charged before the memory is asked for: the charge is the
        // limit. The check against what the compressed bytes can be is a
        // coarse filter on top of it. zstd never writes past the slice it
        // is given.
        self.budget.charge_bytes(declared)?;
        could_inflate_to(data, declared).map_err(|why| self.fail(why))?;
        #[cfg(test)]
        LARGEST_INFLATED.set(LARGEST_INFLATED.get().max(declared));
        let mut out = MutableBuffer::from_len_zeroed(declared);
        let payload = self.payload;
        let zstd = match &mut self.zstd {
            Some(zstd) => zstd,
            empty => empty.insert(
                zstd::bulk::Decompressor::new()
                    .map_err(|_| malformed(payload, "zstd decoder unavailable"))?,
            ),
        };
        let written = zstd
            .decompress_to_buffer(data, out.as_slice_mut())
            .map_err(|_| malformed(payload, "invalid zstd buffer"))?;
        if written != declared {
            return Err(self.fail("decompressed size differs from the declared size"));
        }
        out.truncate(need);
        Ok(out)
    }

    /// The validity bitmap of an array of `len` elements, `nulls` of them
    /// declared null. With no nulls declared the buffer is only
    /// range-checked.
    fn validity(&mut self, len: usize, nulls: usize) -> Result<Option<NullBuffer>, OtapError> {
        if nulls == 0 {
            self.range()?;
            return Ok(None);
        }
        let bits: Buffer = self.buffer(len.div_ceil(8))?.into();
        let validity = NullBuffer::new(BooleanBuffer::new(bits, 0, len));
        if validity.null_count() != nulls {
            return Err(self.fail("null count differs from the validity bitmap"));
        }
        Ok(Some(validity))
    }

    /// `len + 1` offsets: non-negative and never decreasing. Returns the
    /// last one, the length of what they index.
    fn offsets(&mut self, len: usize) -> Result<(Buffer, usize), OtapError> {
        if len == 0 {
            // Writers may leave the offsets of an empty array out.
            self.range()?;
            let mut zero = MutableBuffer::with_capacity(4);
            zero.extend_from_slice(&0i32.to_le_bytes());
            return Ok((zero.into(), 0));
        }
        let bytes = (len + 1)
            .checked_mul(4)
            .ok_or_else(|| self.fail("array too long"))?;
        let offsets = self.buffer(bytes)?;
        let mut previous = 0i32;
        for chunk in offsets.as_slice().as_chunks::<4>().0 {
            let offset = i32::from_le_bytes(*chunk);
            if offset < previous {
                return Err(self.fail("offsets are negative or decreasing"));
            }
            previous = offset;
        }
        Ok((offsets.into(), usize::try_from(previous).unwrap_or(0)))
    }

    fn build(&self, builder: ArrayDataBuilder) -> Result<ArrayData, OtapError> {
        builder.build().map_err(|source| self.ipc(source))
    }

    /// Decode the next column of the batch.
    fn array(&mut self, node: &Node) -> Result<ArrayData, OtapError> {
        let (len, nulls) = self.node()?;
        let validity = self.validity(len, nulls)?;
        let builder = ArrayData::builder(node.data_type.clone()).len(len);
        if let Some(id) = node.dictionary {
            return self.keys(node, id, len, builder, validity);
        }
        let builder = match &node.shape {
            Shape::Bool => builder.add_buffer(self.buffer(len.div_ceil(8))?.into()),
            Shape::Fixed(width) => builder.add_buffer(self.fixed(len, *width)?.into()),
            Shape::Bytes => {
                let (offsets, end) = self.offsets(len)?;
                let values = self.buffer(end)?;
                builder.add_buffer(offsets).add_buffer(values.into())
            }
            Shape::Struct(members) => {
                let mut builder = builder;
                for member in members {
                    let child = self.array(member)?;
                    if child.len() != len {
                        return Err(self.fail("struct member length differs from the struct's"));
                    }
                    builder = builder.add_child_data(child);
                }
                builder
            }
            Shape::List(item) => {
                let (offsets, end) = self.offsets(len)?;
                let items = self.array(item)?;
                if end > items.len() {
                    return Err(self.fail("list offsets beyond the items"));
                }
                builder.add_buffer(offsets).add_child_data(items)
            }
        };
        self.build(builder.nulls(validity))
    }

    fn fixed(&mut self, len: usize, width: usize) -> Result<MutableBuffer, OtapError> {
        let bytes = len
            .checked_mul(width)
            .ok_or_else(|| self.fail("array too long"))?;
        self.buffer(bytes)
    }

    /// A column of dictionary keys. Every key ends up inside the
    /// dictionary: valid ones are checked, those of null slots are zeroed.
    fn keys(
        &mut self,
        node: &Node,
        id: i64,
        len: usize,
        builder: ArrayDataBuilder,
        validity: Option<NullBuffer>,
    ) -> Result<ArrayData, OtapError> {
        let (Shape::Fixed(width), DataType::Dictionary(_, value_type)) =
            (&node.shape, &node.data_type)
        else {
            return Err(self.fail("dictionary column without a key type"));
        };
        let mut keys = self.fixed(len, *width)?;
        let values = self.dictionaries.get(&id).filter(|d| !d.is_empty());
        let size = values.map_or(0, |d| d.len());
        for (i, key) in keys.as_slice_mut().chunks_exact_mut(*width).enumerate() {
            self.budget.tick()?;
            if validity.as_ref().is_some_and(|v| v.is_null(i)) {
                key.fill(0);
                continue;
            }
            let mut wide = [0u8; 8];
            wide[..key.len()].copy_from_slice(key);
            if usize::try_from(u64::from_le_bytes(wide)).map_or(true, |k| k >= size) {
                return Err(self.fail("dictionary key outside the dictionary"));
            }
        }
        // Null slots hold key 0: give it something to point at.
        let values = match values {
            Some(values) => values.to_data(),
            None => new_null_array(value_type, 1).to_data(),
        };
        self.build(
            builder
                .add_buffer(keys.into())
                .add_child_data(values)
                .nulls(validity),
        )
    }

    /// Fail unless the batch declared exactly what was read.
    fn finish(mut self) -> Result<(), OtapError> {
        if self.nodes.next().is_some() || self.buffers.next().is_some() {
            return Err(self.fail("more field nodes or buffers than the schema needs"));
        }
        Ok(())
    }
}

/// Decode a record batch message against `plan`.
pub(super) fn record_batch(
    payload: &'static str,
    plan: &Plan,
    header: ipc::RecordBatch<'_>,
    body: &[u8],
    budget: &Budget,
    dictionaries: &HashMap<i64, ArrayRef>,
) -> Result<RecordBatch, OtapError> {
    let (rows, mut reader) =
        Reader::open(payload, header, body, plan.counts, budget, dictionaries)?;
    let mut columns = Vec::with_capacity(plan.columns.len());
    for node in &plan.columns {
        let data = reader.array(node)?;
        if data.len() != rows {
            return Err(malformed(
                payload,
                "column length differs from the row count",
            ));
        }
        columns.push(make_array(data));
    }
    reader.finish()?;
    let options = RecordBatchOptions::new().with_row_count(Some(rows));
    RecordBatch::try_new_with_options(plan.schema.clone(), columns, &options)
        .map_err(|source| OtapError::Ipc { payload, source })
}

/// A decoded dictionary batch.
pub(super) struct Dictionary {
    pub id: i64,
    pub is_delta: bool,
    pub values: ArrayRef,
}

/// Decode a dictionary batch message against `plan`.
pub(super) fn dictionary(
    payload: &'static str,
    plan: &Plan,
    header: ipc::DictionaryBatch<'_>,
    body: &[u8],
    budget: &Budget,
) -> Result<Dictionary, OtapError> {
    let id = header.id();
    let Some(node) = plan.dictionaries.get(&id) else {
        return Err(malformed(payload, "dictionary id is not in the schema"));
    };
    let Some(data) = header.data() else {
        return Err(malformed(payload, "dictionary batch without data"));
    };
    let none = HashMap::new();
    let (rows, mut reader) = Reader::open(payload, data, body, node.counts(), budget, &none)?;
    let values = reader.array(node)?;
    if values.len() != rows {
        return Err(malformed(
            payload,
            "dictionary length differs from the row count",
        ));
    }
    reader.finish()?;
    Ok(Dictionary {
        id,
        is_delta: header.isDelta(),
        values: make_array(values),
    })
}
