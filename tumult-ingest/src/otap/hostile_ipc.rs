//! Hand-crafted Arrow IPC that no well-behaved writer produces, and streams
//! built to make the receiver keep or compute too much.
//!
//! The daemon is built with `panic = "abort"`: a panic anywhere below
//! `StreamConsumer::consume` would end the process, so every case here must
//! come back as an error. Like `hostile`, these tests hold no database and
//! run under an address-space cap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arrow::array::{
    Array, ArrayRef, DictionaryArray, DurationNanosecondArray, FixedSizeBinaryArray, RecordBatch,
    StringArray, TimestampNanosecondArray, UInt16Array,
};
use arrow::datatypes::{DataType, Field, Fields, IntervalUnit, Schema, TimeUnit, UnionFields};
use arrow::ipc::writer::StreamWriter;

use super::craft::{batch_message, dictionary_message, schema_message, BatchHeader};
use super::limits::{OtapLimits, RetainedPool};
use super::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords};
use super::{to_logs_request, to_trace_request, OtapBatch, OtapError, StreamConsumer};

type Ty = ArrowPayloadType;

fn payload(ty: Ty, record: Vec<u8>) -> ArrowPayload {
    ArrowPayload {
        schema_id: format!("{}-x", ty.as_str_name()),
        r#type: ty as i32,
        record,
    }
}

fn records(arrow_payloads: Vec<ArrowPayload>) -> BatchArrowRecords {
    BatchArrowRecords {
        batch_id: 1,
        arrow_payloads,
        headers: Vec::new(),
    }
}

/// Decode one SPANS payload on a fresh stream.
fn consume(record: Vec<u8>) -> Result<OtapBatch, OtapError> {
    consume_with(OtapLimits::default(), record)
}

fn consume_with(limits: OtapLimits, record: Vec<u8>) -> Result<OtapBatch, OtapError> {
    StreamConsumer::new(limits).consume(records(vec![payload(Ty::Spans, record)]))
}

/// A schema message followed by one record batch with the given header.
fn crafted(schema: &Schema, header: &BatchHeader, body: &[u8]) -> Vec<u8> {
    let mut out = schema_message(schema);
    out.extend(batch_message(header, body));
    out
}

/// The bytes of a fresh IPC stream holding `batch`.
fn ipc(batch: &RecordBatch) -> Vec<u8> {
    let mut writer = StreamWriter::try_new(Vec::new(), batch.schema_ref()).unwrap();
    writer.write(batch).unwrap();
    std::mem::take(writer.get_mut())
}

fn malformed(result: Result<OtapBatch, OtapError>, needle: &str) -> Result<(), String> {
    match result {
        Err(OtapError::Malformed { reason, .. }) if reason.contains(needle) => Ok(()),
        Err(other) => Err(format!("expected malformed ({needle}), got error: {other}")),
        Ok(batch) => Err(format!(
            "expected malformed ({needle}), decoded {} tables",
            batch.payloads()
        )),
    }
}

fn plain(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, true).with_metadata(HashMap::from([(
        "encoding".to_string(),
        "plain".to_string(),
    )]))
}

/// What a SPANS table needs before a span is made of a row.
pub(super) struct SpanColumns {
    pub trace_ids: ArrayRef,
    pub span_ids: ArrayRef,
    pub extra: Vec<(Field, ArrayRef)>,
}

/// `n` spans with distinct, well-formed ids.
pub(super) fn span_columns(n: usize) -> SpanColumns {
    let trace_ids: Vec<[u8; 16]> = (0..n)
        .map(|i| {
            let mut id = [0x11; 16];
            id[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
            id
        })
        .collect();
    let span_ids: Vec<[u8; 8]> = (0..n).map(|i| (i as u64 + 1).to_le_bytes()).collect();
    SpanColumns {
        trace_ids: Arc::new(
            FixedSizeBinaryArray::try_from_iter(trace_ids.iter().map(|id| id.as_slice())).unwrap(),
        ),
        span_ids: Arc::new(
            FixedSizeBinaryArray::try_from_iter(span_ids.iter().map(|id| id.as_slice())).unwrap(),
        ),
        extra: Vec::new(),
    }
}

/// A SPANS table with every required column.
pub(super) fn spans_from(n: usize, columns: SpanColumns) -> RecordBatch {
    let ids: Vec<u16> = (0..n).map(|i| u16::try_from(i % 65_536).unwrap()).collect();
    let mut all: Vec<(Field, ArrayRef)> = vec![
        (
            plain("id", DataType::UInt16),
            Arc::new(UInt16Array::from(ids)),
        ),
        (
            Field::new(
                "start_time_unix_nano",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Arc::new(TimestampNanosecondArray::from(vec![1_000; n])),
        ),
        (
            Field::new(
                "duration_time_unix_nano",
                DataType::Duration(TimeUnit::Nanosecond),
                false,
            ),
            Arc::new(DurationNanosecondArray::from(vec![5; n])),
        ),
        (
            Field::new("trace_id", columns.trace_ids.data_type().clone(), true),
            columns.trace_ids,
        ),
        (
            Field::new("span_id", columns.span_ids.data_type().clone(), true),
            columns.span_ids,
        ),
        (
            Field::new("name", DataType::Utf8, false),
            Arc::new(StringArray::from(vec!["op"; n])),
        ),
    ];
    all.extend(columns.extra);
    let (fields, arrays): (Vec<Field>, Vec<ArrayRef>) = all.into_iter().unzip();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

pub(super) fn real_spans(n: usize) -> RecordBatch {
    spans_from(n, span_columns(n))
}

fn span_count(batch: &OtapBatch) -> Result<usize, OtapError> {
    Ok(to_trace_request(batch)?
        .resource_spans
        .iter()
        .flat_map(|r| &r.scope_spans)
        .map(|s| s.spans.len())
        .sum())
}

fn spans_decoded(rb: &RecordBatch) -> Result<usize, OtapError> {
    span_count(&consume(ipc(rb))?)
}

// ---------------------------------------------------------------------------
// B1. IPC that used to reach a panic in the Arrow reader
// ---------------------------------------------------------------------------

fn ids_schema() -> Schema {
    Schema::new(vec![Field::new("id", DataType::UInt16, true)])
}

fn one_id(buffer: (i64, i64)) -> BatchHeader {
    BatchHeader {
        rows: 1,
        nodes: vec![(1, 0)],
        buffers: vec![(0, 0), buffer],
        ..BatchHeader::default()
    }
}

#[test]
fn uncompressed_buffer_outside_the_body_is_refused() {
    let body = [7u8, 0, 0, 0, 0, 0, 0, 0];
    let ok = consume(crafted(&ids_schema(), &one_id((0, 2)), &body)).unwrap();
    assert_eq!(ok.get(Ty::Spans).unwrap().num_rows(), 1, "the honest batch");

    for buffer in [
        (1 << 40, 8),
        (0, -1),
        (-8, 8),
        (8, 8),
        (7, 2),
        (i64::MAX, 1),
        (4, i64::MAX),
        (i64::MIN, i64::MIN),
    ] {
        let result = consume(crafted(&ids_schema(), &one_id(buffer), &body));
        malformed(result, "buffer outside the message body")
            .unwrap_or_else(|e| panic!("buffer {buffer:?}: {e}"));
    }
}

#[test]
fn variadic_buffer_counts_are_refused() {
    let header = BatchHeader {
        variadic: Some(vec![1]),
        ..one_id((0, 2))
    };
    let result = consume(crafted(&ids_schema(), &header, &[7u8; 8]));
    malformed(result, "variadic").unwrap();
}

#[test]
fn union_column_is_refused_by_the_schema_allowlist() {
    let union = DataType::Union(
        UnionFields::try_new(vec![0], vec![Field::new("a", DataType::Int32, true)]).unwrap(),
        arrow::datatypes::UnionMode::Sparse,
    );
    let schema = Schema::new(vec![Field::new("u", union, true)]);
    // 100 rows whose type ids buffer is empty.
    let header = BatchHeader {
        rows: 100,
        nodes: vec![(100, 0), (100, 0)],
        buffers: vec![(0, 0), (0, 0), (0, 0)],
        ..BatchHeader::default()
    };
    malformed(
        consume(crafted(&schema, &header, &[])),
        "unsupported column type",
    )
    .unwrap();
}

fn item(data_type: DataType) -> Arc<Field> {
    Arc::new(Field::new("item", data_type, true))
}

/// Types the OTAP tables never use. The Arrow reader has panics behind
/// several of them, so none is let through.
fn types_outside_the_allowlist() -> Vec<DataType> {
    let entries = Field::new(
        "entries",
        DataType::Struct(Fields::from(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Int32, true),
        ])),
        false,
    );
    vec![
        DataType::Null,
        DataType::Utf8View,
        DataType::BinaryView,
        DataType::LargeUtf8,
        DataType::LargeBinary,
        DataType::LargeList(item(DataType::Int32)),
        DataType::ListView(item(DataType::Int32)),
        DataType::FixedSizeList(item(DataType::Int32), 2),
        DataType::Map(Arc::new(entries), false),
        DataType::RunEndEncoded(
            Arc::new(Field::new("run_ends", DataType::Int32, false)),
            Arc::new(Field::new("values", DataType::Utf8, true)),
        ),
        DataType::Decimal128(10, 2),
        DataType::Date32,
        DataType::Time64(TimeUnit::Nanosecond),
        DataType::Interval(IntervalUnit::DayTime),
        DataType::Timestamp(TimeUnit::Millisecond, None),
        DataType::Duration(TimeUnit::Second),
        DataType::Float16,
        DataType::List(item(DataType::List(item(DataType::Int32)))),
        DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8)),
        DataType::Dictionary(Box::new(DataType::UInt64), Box::new(DataType::Utf8)),
        DataType::Dictionary(
            Box::new(DataType::UInt8),
            Box::new(DataType::List(item(DataType::Int32))),
        ),
        DataType::FixedSizeBinary(0),
        DataType::FixedSizeBinary(4096),
        DataType::Struct(Fields::empty()),
    ]
}

#[test]
fn types_the_tables_never_use_are_refused() {
    for data_type in types_outside_the_allowlist() {
        let schema = Schema::new(vec![
            Field::new("id", DataType::UInt16, true),
            Field::new("odd", data_type.clone(), true),
        ]);
        let header = BatchHeader::default();
        let result = consume(crafted(&schema, &header, &[]));
        malformed(result, "unsupported column type").unwrap_or_else(|e| panic!("{data_type}: {e}"));
    }
}

/// `{id: u16, name: utf8}`, two rows: the honest header and body.
fn two_columns() -> (Schema, BatchHeader, Vec<u8>) {
    let schema = Schema::new(vec![
        Field::new("id", DataType::UInt16, true),
        Field::new("name", DataType::Utf8, true),
    ]);
    let mut body = Vec::new();
    body.extend_from_slice(&[1, 0, 2, 0, 0, 0, 0, 0]); // id values at 0
    for offset in [0i32, 2, 4] {
        body.extend_from_slice(&offset.to_le_bytes()); // name offsets at 8
    }
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(b"abcd\0\0\0\0"); // name values at 24
    body.extend_from_slice(&[0b01, 0, 0, 0, 0, 0, 0, 0]); // a validity bitmap at 32
    let header = BatchHeader {
        rows: 2,
        nodes: vec![(2, 0), (2, 0)],
        buffers: vec![(0, 0), (0, 4), (0, 0), (8, 12), (24, 4)],
        ..BatchHeader::default()
    };
    (schema, header, body)
}

fn set_offsets(body: &mut [u8], offsets: [i32; 3]) {
    for (i, offset) in offsets.into_iter().enumerate() {
        body[8 + 4 * i..12 + 4 * i].copy_from_slice(&offset.to_le_bytes());
    }
}

#[test]
fn structure_that_disagrees_with_the_schema_is_refused() {
    let (schema, honest, body) = two_columns();
    let batch = consume(crafted(&schema, &honest, &body)).unwrap();
    assert_eq!(batch.get(Ty::Spans).unwrap().num_rows(), 2);
    let with_nulls = BatchHeader {
        nodes: vec![(2, 1), (2, 0)],
        buffers: vec![(32, 1), (0, 4), (0, 0), (8, 12), (24, 4)],
        ..honest.clone()
    };
    let batch = consume(crafted(&schema, &with_nulls, &body)).unwrap();
    assert_eq!(batch.get(Ty::Spans).unwrap().column(0).null_count(), 1);

    type Damage = fn(&mut BatchHeader, &mut Vec<u8>);
    let cases: Vec<(&str, Damage)> = vec![
        ("a node missing", |h, _| {
            h.nodes.pop();
        }),
        ("a node too many", |h, _| h.nodes.push((2, 0))),
        ("no nodes", |h, _| h.nodes.clear()),
        ("a buffer missing", |h, _| {
            h.buffers.pop();
        }),
        ("a buffer too many", |h, _| h.buffers.push((0, 0))),
        ("no buffers", |h, _| h.buffers.clear()),
        ("values shorter than the rows", |h, _| h.buffers[1] = (0, 2)),
        ("no values", |h, _| h.buffers[1] = (0, 0)),
        ("nulls without a bitmap", |h, _| h.nodes[0] = (2, 1)),
        ("more nulls than rows", |h, _| {
            h.nodes[0] = (2, 3);
            h.buffers[0] = (32, 1);
        }),
        ("negative null count", |h, _| h.nodes[0] = (2, -1)),
        ("null count that is not the bitmap's", |h, _| {
            h.nodes[0] = (2, 2);
            h.buffers[0] = (32, 1);
        }),
        ("column longer than the batch", |h, _| h.nodes[0] = (3, 0)),
        ("column shorter than the batch", |h, _| h.nodes[1] = (1, 0)),
        ("negative column length", |h, _| h.nodes[1] = (-2, 0)),
        ("offsets shorter than the rows", |h, _| {
            h.buffers[3] = (8, 8);
        }),
        ("no offsets", |h, _| h.buffers[3] = (8, 0)),
        ("offsets going backwards", |_, b| set_offsets(b, [0, 4, 2])),
        ("negative first offset", |_, b| set_offsets(b, [-1, 2, 4])),
        ("last offset beyond the values", |_, b| {
            set_offsets(b, [0, 2, 9]);
        }),
        ("huge offset", |_, b| set_offsets(b, [0, 2, i32::MAX])),
        ("text that is not utf-8", |_, b| b[24] = 0xff),
        ("rows beyond the columns", |h, _| h.rows = 3),
        ("negative rows", |h, _| h.rows = -1),
    ];
    for (what, damage) in cases {
        let (mut header, mut body) = (honest.clone(), body.clone());
        damage(&mut header, &mut body);
        let result = consume(crafted(&schema, &header, &body));
        assert!(
            matches!(
                result,
                Err(OtapError::Malformed { .. } | OtapError::Ipc { .. })
            ),
            "{what}: {:?}",
            result.map(|b| b.payloads())
        );
    }
}

fn names_schema() -> Schema {
    let names = DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Utf8));
    Schema::new(vec![Field::new("name", names, true)])
}

/// A dictionary of the two strings `ab` and `cd`.
fn two_names() -> (BatchHeader, Vec<u8>) {
    let mut body = Vec::new();
    for offset in [0i32, 2, 4] {
        body.extend_from_slice(&offset.to_le_bytes());
    }
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(b"abcd\0\0\0\0");
    let header = BatchHeader {
        rows: 2,
        nodes: vec![(2, 0)],
        buffers: vec![(0, 0), (0, 12), (16, 4)],
        ..BatchHeader::default()
    };
    (header, body)
}

/// Two rows of `name` with the given dictionary keys.
fn keyed(keys: [u8; 2]) -> Vec<u8> {
    let header = BatchHeader {
        rows: 2,
        nodes: vec![(2, 0)],
        buffers: vec![(0, 0), (0, 2)],
        ..BatchHeader::default()
    };
    batch_message(&header, &[keys[0], keys[1], 0, 0, 0, 0, 0, 0])
}

fn dictionary_stream(id: i64, is_delta: bool, keys: [u8; 2]) -> Vec<u8> {
    let (header, body) = two_names();
    let mut out = schema_message(&names_schema());
    out.extend(dictionary_message(id, is_delta, &header, &body));
    out.extend(keyed(keys));
    out
}

#[test]
fn dictionary_keys_and_ids_are_checked() {
    let batch = consume(dictionary_stream(0, false, [0, 1])).unwrap();
    assert_eq!(batch.get(Ty::Spans).unwrap().num_rows(), 2, "honest keys");

    malformed(consume(dictionary_stream(0, false, [0, 2])), "dictionary")
        .unwrap_or_else(|e| panic!("key outside the dictionary: {e}"));
    malformed(consume(dictionary_stream(0, false, [255, 0])), "dictionary")
        .unwrap_or_else(|e| panic!("key far outside the dictionary: {e}"));
    malformed(consume(dictionary_stream(7, false, [0, 1])), "dictionary")
        .unwrap_or_else(|e| panic!("dictionary id the schema does not have: {e}"));
    malformed(consume(dictionary_stream(0, true, [0, 1])), "dictionary")
        .unwrap_or_else(|e| panic!("delta for a dictionary never sent: {e}"));

    // Keys with no dictionary at all.
    let mut record = schema_message(&names_schema());
    record.extend(keyed([0, 0]));
    malformed(consume(record), "dictionary")
        .unwrap_or_else(|e| panic!("keys without a dictionary: {e}"));
}

// ---------------------------------------------------------------------------
// B2. What a stream makes the receiver keep
// ---------------------------------------------------------------------------

/// `columns` dictionary-encoded columns of `values` distinct 64-byte strings.
fn dictionary_table(columns: usize, values: usize, salt: &str) -> RecordBatch {
    let text: Vec<String> = (0..values).map(|i| format!("{salt}{i:0>60}")).collect();
    let keys = UInt16Array::from_iter_values((0..values).map(|i| u16::try_from(i).unwrap()));
    let array: ArrayRef = Arc::new(DictionaryArray::new(
        keys,
        Arc::new(StringArray::from_iter_values(&text)),
    ));
    let fields: Vec<Field> = (0..columns)
        .map(|c| Field::new(format!("c{c}"), array.data_type().clone(), true))
        .collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), vec![array; columns]).unwrap()
}

fn stream_state(result: Result<OtapBatch, OtapError>, expected: &str, in_pool: bool) {
    match result {
        Err(OtapError::StreamState { what, shared, .. }) => {
            assert_eq!((what, shared), (expected, in_pool));
        }
        Err(other) => panic!("expected {expected}, got error: {other}"),
        Ok(batch) => panic!("expected {expected}, decoded {} tables", batch.payloads()),
    }
}

#[test]
fn schema_id_longer_than_the_cap_is_refused() {
    let mut payload = payload(Ty::Spans, ipc(&real_spans(1)));
    payload.schema_id = "s".repeat(256);
    StreamConsumer::default()
        .consume(records(vec![payload.clone()]))
        .unwrap();
    payload.schema_id = "s".repeat(257);
    let result = StreamConsumer::default().consume(records(vec![payload]));
    malformed(result, "schema id longer than 256 bytes").unwrap();
}

#[test]
fn dictionaries_a_stream_keeps_are_budgeted_in_bytes() {
    let limits = OtapLimits {
        max_stream_retained_bytes: 64 * 1024,
        ..OtapLimits::default()
    };
    // About 7 KiB of dictionary: fits.
    consume_with(limits, ipc(&dictionary_table(1, 100, "a"))).unwrap();
    // One big dictionary.
    let result = consume_with(limits, ipc(&dictionary_table(1, 2000, "a")));
    stream_state(result, "retained bytes per stream", false);
    // Many dictionary ids, each small.
    let result = consume_with(limits, ipc(&dictionary_table(20, 100, "a")));
    stream_state(result, "retained bytes per stream", false);
}

#[test]
fn dictionaries_of_every_payload_type_count_against_the_stream() {
    let limits = OtapLimits {
        max_stream_retained_bytes: 64 * 1024,
        ..OtapLimits::default()
    };
    let mut consumer = StreamConsumer::new(limits);
    let mut result = Ok(());
    // Each table alone fits; the stream keeps all of them.
    for (round, ty) in [Ty::Spans, Ty::SpanAttrs, Ty::SpanEvents, Ty::SpanLinks]
        .into_iter()
        .enumerate()
    {
        let record = ipc(&dictionary_table(4, 100, "a"));
        result = consumer
            .consume(records(vec![payload(ty, record)]))
            .map(drop);
        if result.is_err() {
            assert!(round >= 1, "the first table fits");
            break;
        }
    }
    let err = result.unwrap_err();
    assert!(
        matches!(
            err,
            OtapError::StreamState {
                what: "retained bytes per stream",
                shared: false,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn replacing_a_dictionary_gives_its_bytes_back() {
    let limits = OtapLimits {
        max_stream_retained_bytes: 64 * 1024,
        ..OtapLimits::default()
    };
    let pool = RetainedPool::new(1 << 20);
    let mut consumer = StreamConsumer::with_pool(limits, pool.clone());
    let mut high = 0;
    // A new schema id replaces the table's state: fifty replacements keep
    // no more than one.
    for round in 0..50 {
        let mut payload = payload(Ty::Spans, ipc(&dictionary_table(1, 300, "a")));
        payload.schema_id = format!("schema-{round}");
        consumer.consume(records(vec![payload])).unwrap();
        high = high.max(pool.used());
    }
    assert!(high > 15_000, "the dictionary is accounted for: {high}");
    assert!(high < 64 * 1024, "replaced state is released: {high}");
    drop(consumer);
    assert_eq!(pool.used(), 0);
}

#[test]
fn streams_share_one_pool_of_retained_bytes() {
    let limits = OtapLimits::default();
    let pool = RetainedPool::new(48 * 1024);
    let table = dictionary_table(1, 400, "a");
    let mut first = StreamConsumer::with_pool(limits, pool.clone());
    first
        .consume(records(vec![payload(Ty::Spans, ipc(&table))]))
        .unwrap();
    let held = pool.used();
    assert!(
        held > 20_000,
        "the first stream holds its dictionary: {held}"
    );

    let mut second = StreamConsumer::with_pool(limits, pool.clone());
    let result = second.consume(records(vec![payload(Ty::Spans, ipc(&table))]));
    stream_state(result, "retained bytes across streams", true);
    assert_eq!(pool.used(), held, "a refused stream holds nothing");

    drop(first);
    assert_eq!(pool.used(), 0, "a closed stream gives everything back");
    let mut third = StreamConsumer::with_pool(limits, pool.clone());
    third
        .consume(records(vec![payload(Ty::Spans, ipc(&table))]))
        .unwrap();
    drop((second, third));
    assert_eq!(pool.used(), 0);
}

#[test]
fn delta_dictionaries_are_capped_in_number() {
    let limits = OtapLimits {
        max_dictionary_deltas: 2,
        ..OtapLimits::default()
    };
    let mut consumer = StreamConsumer::new(limits);
    consumer
        .consume(records(vec![payload(
            Ty::Spans,
            dictionary_stream(0, false, [0, 1]),
        )]))
        .unwrap();
    let (header, body) = two_names();
    let mut result = Ok(0);
    for round in 0..3 {
        let mut record = dictionary_message(0, true, &header, &body);
        record.extend(keyed([0, 3]));
        result = consumer
            .consume(records(vec![payload(Ty::Spans, record)]))
            .map(|batch| batch.get(Ty::Spans).map_or(0, RecordBatch::num_rows));
        if result.is_err() {
            assert_eq!(round, 2, "two deltas are allowed");
            break;
        }
        assert_eq!(result.as_ref().ok(), Some(&2), "round {round}");
    }
    let err = result.unwrap_err();
    assert!(
        matches!(
            err,
            OtapError::StreamState {
                what: "dictionary deltas per stream",
                limit: 2,
                shared: false,
            }
        ),
        "{err}"
    );
}

#[test]
fn schema_width_is_capped() {
    let wide = |n: usize, data_type: DataType| {
        let fields: Vec<Field> = (0..n)
            .map(|i| Field::new(format!("f{i}"), data_type.clone(), true))
            .collect();
        consume(crafted(&Schema::new(fields), &BatchHeader::default(), &[]))
    };
    malformed(wide(257, DataType::UInt8), "fields").unwrap();
    let names = DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Utf8));
    malformed(wide(65, names), "dictionar").unwrap();
    malformed(wide(0, DataType::UInt8), "fields").unwrap();

    // Depth: structs within structs.
    let mut nested = DataType::Int32;
    for _ in 0..6 {
        nested = DataType::Struct(Fields::from(vec![Field::new("in", nested, true)]));
    }
    let schema = Schema::new(vec![Field::new("deep", nested, true)]);
    malformed(
        consume(crafted(&schema, &BatchHeader::default(), &[])),
        "nest",
    )
    .unwrap();

    // Two columns of one name: which one a decoder reads is anyone's guess.
    let schema = Schema::new(vec![
        Field::new("id", DataType::UInt16, true),
        Field::new("id", DataType::UInt16, true),
    ]);
    malformed(
        consume(crafted(&schema, &BatchHeader::default(), &[])),
        "duplicate",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// B3. Work per byte received
// ---------------------------------------------------------------------------

#[test]
fn columns_without_buffers_cannot_carry_rows() {
    // 2000 null-typed columns of 250 000 rows fit in a few kilobytes.
    let fields: Vec<Field> = (0..2000)
        .map(|i| Field::new(format!("n{i}"), DataType::Null, true))
        .collect();
    let header = BatchHeader {
        rows: 250_000,
        nodes: vec![(250_000, 250_000); 2000],
        ..BatchHeader::default()
    };
    let result = consume(crafted(&Schema::new(fields), &header, &[]));
    assert!(
        matches!(result, Err(OtapError::Malformed { .. })),
        "{:?}",
        result.map(|b| b.payloads())
    );
}

#[test]
fn spans_without_the_required_columns_are_refused() {
    assert_eq!(spans_decoded(&real_spans(3)).unwrap(), 3);

    // A few hundred bytes must not become 250 000 default spans.
    let ids = UInt16Array::from_iter_values((0..50_000).map(|i| u16::try_from(i).unwrap()));
    let bare = RecordBatch::try_new(
        Arc::new(Schema::new(vec![plain("id", DataType::UInt16)])),
        vec![Arc::new(ids)],
    )
    .unwrap();
    let err = spans_decoded(&bare).unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "SPANS", reason, .. }
            if reason.contains("required")),
        "{err}"
    );

    for missing in [
        "start_time_unix_nano",
        "duration_time_unix_nano",
        "trace_id",
        "span_id",
        "name",
    ] {
        let full = real_spans(2);
        let keep: Vec<usize> = (0..full.num_columns())
            .filter(|&i| full.schema().field(i).name() != missing)
            .collect();
        let err = spans_decoded(&full.project(&keep).unwrap()).unwrap_err();
        assert!(
            matches!(&err, OtapError::Column { table: "SPANS", column, reason }
                if column == missing && reason.contains("required")),
            "{missing}: {err}"
        );
    }
}

fn fixed(width: usize, rows: &[Option<Vec<u8>>]) -> ArrayRef {
    Arc::new(
        FixedSizeBinaryArray::try_from_sparse_iter_with_size(
            rows.iter().map(|r| r.as_deref()),
            i32::try_from(width).unwrap(),
        )
        .unwrap(),
    )
}

#[test]
fn span_and_trace_ids_must_be_real() {
    let with = |trace_ids: ArrayRef, span_ids: ArrayRef| {
        let columns = SpanColumns {
            trace_ids,
            span_ids,
            extra: Vec::new(),
        };
        spans_decoded(&spans_from(1, columns))
    };
    let good = span_columns(1);
    assert_eq!(
        with(good.trace_ids.clone(), good.span_ids.clone()).unwrap(),
        1
    );
    let cases: Vec<(&str, ArrayRef, ArrayRef)> = vec![
        (
            "trace_id",
            fixed(8, &[Some(vec![1; 8])]),
            good.span_ids.clone(),
        ),
        (
            "trace_id",
            fixed(16, &[Some(vec![0; 16])]),
            good.span_ids.clone(),
        ),
        ("trace_id", fixed(16, &[None]), good.span_ids.clone()),
        (
            "trace_id",
            Arc::new(arrow::array::BinaryArray::from_iter_values([b"short"])),
            good.span_ids.clone(),
        ),
        (
            "span_id",
            good.trace_ids.clone(),
            fixed(16, &[Some(vec![1; 16])]),
        ),
        (
            "span_id",
            good.trace_ids.clone(),
            fixed(8, &[Some(vec![0; 8])]),
        ),
        ("span_id", good.trace_ids.clone(), fixed(8, &[None])),
    ];
    for (i, (expected, trace_ids, span_ids)) in cases.into_iter().enumerate() {
        let err = with(trace_ids, span_ids).unwrap_err();
        assert!(
            matches!(&err, OtapError::Column { table: "SPANS", column, .. } if column == expected),
            "case {i}: {err}"
        );
    }
}

#[test]
fn batch_deadline_stops_decoding_and_conversion() {
    let expired = OtapLimits {
        batch_deadline: Duration::ZERO,
        ..OtapLimits::default()
    };
    // In the IPC decoder.
    let result = consume_with(expired, ipc(&real_spans(10)));
    assert!(
        matches!(result, Err(OtapError::Deadline { limit_ms: 0 })),
        "{:?}",
        result.map(|b| b.payloads())
    );
    // In the row loops, whatever decoded the table.
    let mut batch = OtapBatch::new(expired);
    batch.insert(Ty::Spans, real_spans(1000)).unwrap();
    let err = to_trace_request(&batch).unwrap_err();
    assert!(matches!(err, OtapError::Deadline { limit_ms: 0 }), "{err}");
    assert!(err.is_limit());

    let mut batch = OtapBatch::new(expired);
    let ids = UInt16Array::from_iter_values(0..1000);
    let logs = RecordBatch::try_new(
        Arc::new(Schema::new(vec![plain("id", DataType::UInt16)])),
        vec![Arc::new(ids)],
    )
    .unwrap();
    batch.insert(Ty::Logs, logs).unwrap();
    let err = to_logs_request(&batch).unwrap_err();
    assert!(matches!(err, OtapError::Deadline { limit_ms: 0 }), "{err}");
}

#[test]
fn tables_of_another_signal_fail_the_batch() {
    let logs = RecordBatch::try_new(
        Arc::new(Schema::new(vec![plain("id", DataType::UInt16)])),
        vec![Arc::new(UInt16Array::from(vec![0u16]))],
    )
    .unwrap();
    let batch = StreamConsumer::default()
        .consume(records(vec![
            payload(Ty::Spans, ipc(&real_spans(1))),
            payload(Ty::Logs, ipc(&logs)),
        ]))
        .unwrap();
    let err = to_trace_request(&batch).unwrap_err();
    assert!(
        matches!(
            err,
            OtapError::Misplaced {
                payload: "LOGS",
                signal: "traces"
            }
        ),
        "{err}"
    );
    let err = to_logs_request(&batch).unwrap_err();
    assert!(
        matches!(err, OtapError::Misplaced { signal: "logs", .. }),
        "{err}"
    );
}

#[test]
fn payload_type_unknown_is_refused() {
    let mut payload = payload(Ty::Spans, ipc(&real_spans(1)));
    payload.r#type = Ty::Unknown as i32;
    let err = StreamConsumer::default()
        .consume(records(vec![payload]))
        .unwrap_err();
    assert!(matches!(err, OtapError::UnknownPayloadType(0)), "{err}");
}

// ---------------------------------------------------------------------------
// B4. Peer text in errors (they go to the log and to the exporter)
// ---------------------------------------------------------------------------

/// What an error may look like wherever it ends up.
fn assert_tame(err: &OtapError) {
    let text = err.to_string();
    assert!(text.len() <= 512, "{} bytes: {text}", text.len());
    assert!(
        !text.chars().any(char::is_control),
        "control characters: {text:?}"
    );
}

#[test]
fn schema_id_in_an_error_cannot_forge_a_log_line() {
    let mut payload = payload(Ty::Spans, schema_message(&ids_schema()));
    payload.schema_id = "evil\nINFO forged line\r\u{1b}[31m".into();
    let err = StreamConsumer::default()
        .consume(records(vec![payload]))
        .unwrap_err();
    assert!(matches!(err, OtapError::EmptyPayload { .. }), "{err}");
    assert_tame(&err);
    assert!(err.to_string().contains("evil\\nINFO"), "{err}");
}

#[test]
fn column_type_mismatch_does_not_repeat_peer_text() {
    let zone: Arc<str> = format!("UTC\nWARN forged {}", "z".repeat(100_000)).into();
    let odd = DataType::Timestamp(TimeUnit::Nanosecond, Some(zone));
    let mut columns = span_columns(1);
    columns.extra.push((
        Field::new("kind", odd.clone(), true),
        arrow::array::new_null_array(&odd, 1),
    ));
    let err = spans_decoded(&spans_from(1, columns)).unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "SPANS", column, .. } if column == "kind"),
        "{err}"
    );
    assert_tame(&err);
    let text = err.to_string();
    assert!(
        text.contains("expected int32") && text.contains("timestamp"),
        "{text}"
    );
    assert!(!text.contains("forged"), "{text}");

    // The same through a table that never went through the IPC decoder.
    let mut batch = OtapBatch::default();
    let mut columns = span_columns(1);
    columns.extra.push((
        Field::new("kind", odd.clone(), true),
        arrow::array::new_null_array(&odd, 1),
    ));
    batch.insert(Ty::Spans, spans_from(1, columns)).unwrap();
    let err = to_trace_request(&batch).unwrap_err();
    assert_tame(&err);
    assert!(!err.to_string().contains("forged"), "{err}");
}

#[test]
fn field_names_are_capped_and_never_echoed_raw() {
    let name = format!("kind\nERROR forged {}", "n".repeat(5000));
    let schema = Schema::new(vec![Field::new(name, DataType::Int32, true)]);
    let err = consume(crafted(&schema, &BatchHeader::default(), &[])).unwrap_err();
    assert!(matches!(err, OtapError::Malformed { .. }), "{err}");
    assert_tame(&err);
}
