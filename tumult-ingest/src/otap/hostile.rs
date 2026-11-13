//! Malformed and hostile input: every case must come back as an error (or a
//! bounded result), never as an abort, a panic or an unbounded allocation.
//!
//! These tests hold no database, so the test binary can also run them under
//! an address-space cap (`prlimit --as=3000000000 <test binary> otap::hostile`)
//! where a regression aborts instead of taking the machine down.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, NullArray, RecordBatch,
    RecordBatchOptions, StringArray, StructArray, UInt16Array, UInt32Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::{DictionaryHandling, IpcWriteOptions, StreamWriter};
use arrow::ipc::CompressionType;

use super::limits::OtapLimits;
use super::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords};
use super::{to_logs_request, to_metrics_request, to_trace_request, OtapError, StreamConsumer};

type Ty = ArrowPayloadType;

/// A field whose ids travel as-is.
fn plain(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, true).with_metadata(HashMap::from([(
        "encoding".to_string(),
        "plain".to_string(),
    )]))
}

/// A field whose ids travel delta-encoded (the default).
fn delta(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, true)
}

fn table(columns: Vec<(Field, ArrayRef)>) -> RecordBatch {
    let (fields, arrays): (Vec<Field>, Vec<ArrayRef>) = columns.into_iter().unzip();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

fn u16s(values: impl IntoIterator<Item = u16>) -> ArrayRef {
    Arc::new(UInt16Array::from_iter_values(values))
}

fn strs<'a>(values: impl IntoIterator<Item = &'a str>) -> ArrayRef {
    Arc::new(StringArray::from_iter_values(values))
}

/// The bytes of a fresh IPC stream holding `batches`.
fn ipc_with(batches: &[&RecordBatch], options: IpcWriteOptions) -> Vec<u8> {
    let mut writer =
        StreamWriter::try_new_with_options(Vec::new(), batches[0].schema_ref(), options).unwrap();
    for batch in batches {
        writer.write(batch).unwrap();
    }
    std::mem::take(writer.get_mut())
}

fn ipc(batch: &RecordBatch) -> Vec<u8> {
    ipc_with(&[batch], IpcWriteOptions::default())
}

fn zstd() -> IpcWriteOptions {
    IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::ZSTD))
        .unwrap()
}

fn payload(ty: Ty, record: Vec<u8>) -> ArrowPayload {
    ArrowPayload {
        schema_id: format!("{}-h", ty.as_str_name()),
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

/// One batch with a fresh IPC stream per table.
fn batch_of(tables: Vec<(Ty, RecordBatch)>) -> BatchArrowRecords {
    records(
        tables
            .into_iter()
            .map(|(ty, rb)| payload(ty, ipc(&rb)))
            .collect(),
    )
}

fn traces(records: BatchArrowRecords) -> Result<usize, OtapError> {
    traces_with(OtapLimits::default(), records)
}

/// Decode a traces batch, returning the span count.
fn traces_with(limits: OtapLimits, records: BatchArrowRecords) -> Result<usize, OtapError> {
    let batch = StreamConsumer::new(limits).consume(records)?;
    let request = to_trace_request(&batch)?;
    Ok(request
        .resource_spans
        .iter()
        .flat_map(|r| &r.scope_spans)
        .map(|s| s.spans.len())
        .sum())
}

/// A minimal attribute table: every row is the string attribute `key=value`.
fn attrs(parent: Field, parents: ArrayRef, key: &str, value: &str) -> RecordBatch {
    let n = parents.len();
    table(vec![
        (parent, parents),
        (
            Field::new("key", DataType::Utf8, false),
            strs(std::iter::repeat_n(key, n)),
        ),
        (
            Field::new("type", DataType::UInt8, false),
            Arc::new(UInt8Array::from(vec![1u8; n])),
        ),
        (
            Field::new("str", DataType::Utf8, true),
            strs(std::iter::repeat_n(value, n)),
        ),
    ])
}

/// A SPANS table of only plain ids.
fn spans(ids: impl IntoIterator<Item = u16>) -> RecordBatch {
    table(vec![(plain("id", DataType::UInt16), u16s(ids))])
}

fn limit_named(err: &OtapError, expected: &str) -> bool {
    matches!(err, OtapError::Limit { what, .. } if *what == expected)
}

/// Replace the single occurrence of `from` in `bytes`.
fn patch(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    let hits: Vec<usize> = bytes
        .windows(from.len())
        .enumerate()
        .filter(|(_, w)| *w == from)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "pattern must be unique, found at {hits:?}");
    bytes[hits[0]..hits[0] + to.len()].copy_from_slice(to);
}

/// Where the first record batch message of an IPC stream starts, with its
/// metadata length and declared body length.
fn first_record_batch(bytes: &[u8]) -> (usize, usize, i64) {
    let mut at = 0;
    loop {
        let meta_len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let message = arrow::ipc::root_as_message(&bytes[at + 8..at + 8 + meta_len]).unwrap();
        if message.header_type() == arrow::ipc::MessageHeader::RecordBatch {
            return (at, meta_len, message.bodyLength());
        }
        at += 8 + meta_len + usize::try_from(message.bodyLength()).unwrap();
    }
}

// ---------------------------------------------------------------------------
// 1. Declared row counts
// ---------------------------------------------------------------------------

fn zero_columns(rows: usize) -> RecordBatch {
    RecordBatch::try_new_with_options(
        Arc::new(Schema::empty()),
        vec![],
        &RecordBatchOptions::new().with_row_count(Some(rows)),
    )
    .unwrap()
}

#[test]
fn declared_row_count_without_backing_data_is_refused() {
    // ~200 bytes on the wire, 2^36 rows declared: 512 GB of `Option<u32>`.
    let bomb = ipc(&zero_columns(1 << 36));
    assert!(bomb.len() < 400, "{} bytes", bomb.len());
    let err = traces(records(vec![payload(Ty::Spans, bomb)])).unwrap_err();
    assert!(limit_named(&err, "rows per table"), "{err}");

    let max = usize::try_from(i64::MAX).unwrap();
    let err = traces(records(vec![payload(Ty::Spans, ipc(&zero_columns(max)))])).unwrap_err();
    assert!(limit_named(&err, "rows per table"), "{err}");

    // A negative length is malformed, not a limit.
    let mut negative = ipc(&zero_columns(max));
    patch(
        &mut negative,
        &i64::MAX.to_le_bytes(),
        &(-1i64).to_le_bytes(),
    );
    let err = traces(records(vec![payload(Ty::Spans, negative)])).unwrap_err();
    assert!(matches!(err, OtapError::Malformed { .. }), "{err}");

    // A Null-typed column has no buffers either.
    let nulls = table(vec![(
        Field::new("id", DataType::Null, true),
        Arc::new(NullArray::new(1 << 36)) as ArrayRef,
    )]);
    let err = traces(records(vec![payload(Ty::Spans, ipc(&nulls))])).unwrap_err();
    assert!(limit_named(&err, "rows per table"), "{err}");

    // The limit is the configured one, and one row below it passes.
    let limits = OtapLimits {
        max_rows_per_table: 3,
        ..OtapLimits::default()
    };
    assert_eq!(
        traces_with(limits, batch_of(vec![(Ty::Spans, spans([1, 2, 3]))])).unwrap(),
        3
    );
    let err = traces_with(limits, batch_of(vec![(Ty::Spans, spans([1, 2, 3, 4]))])).unwrap_err();
    assert!(limit_named(&err, "rows per table"), "{err}");
}

// ---------------------------------------------------------------------------
// 2. IPC body length and decompression
// ---------------------------------------------------------------------------

#[test]
fn body_length_beyond_the_payload_is_refused() {
    let blob = table(vec![(
        Field::new("trace_id", DataType::Binary, true),
        Arc::new(BinaryArray::from_iter_values([&[7u8; 1000][..]])) as ArrayRef,
    )]);
    let good = ipc(&blob);
    let (_, _, body_len) = first_record_batch(&good);
    for declared in [1i64 << 46, i64::MAX, -1, -(1 << 40), body_len + 8] {
        let mut bad = good.clone();
        patch(&mut bad, &body_len.to_le_bytes(), &declared.to_le_bytes());
        let err = traces(records(vec![payload(Ty::Spans, bad)])).unwrap_err();
        assert!(
            matches!(&err, OtapError::Malformed { payload: "SPANS", reason } if reason.contains("body length")),
            "declared {declared}: {err}"
        );
    }
    // Truncated in the middle of the body.
    let mut cut = good.clone();
    cut.truncate(good.len() - 16);
    let err = traces(records(vec![payload(Ty::Spans, cut)])).unwrap_err();
    assert!(matches!(err, OtapError::Malformed { .. }), "{err}");
    // A metadata length pointing past the payload.
    let (at, _, _) = first_record_batch(&good);
    let mut meta = good;
    meta[at + 4..at + 8].copy_from_slice(&i32::MAX.to_le_bytes());
    let err = traces(records(vec![payload(Ty::Spans, meta)])).unwrap_err();
    assert!(matches!(err, OtapError::Malformed { .. }), "{err}");
}

#[test]
fn declared_decompressed_size_is_budgeted_before_decompression() {
    let blob = table(vec![(
        Field::new("trace_id", DataType::Binary, true),
        Arc::new(BinaryArray::from_iter_values([&[7u8; 4099][..]])) as ArrayRef,
    )]);
    let good = ipc_with(&[&blob], zstd());
    assert_eq!(
        traces(records(vec![payload(Ty::Spans, good.clone())])).unwrap(),
        1
    );

    // The values buffer declares its uncompressed size in an 8-byte prefix;
    // claim 64 GiB for it.
    // (The last match: the offsets buffer ends in the same bytes.)
    let prefix = good
        .windows(8)
        .rposition(|w| w == 4099i64.to_le_bytes())
        .unwrap();
    let mut bomb = good.clone();
    bomb[prefix..prefix + 8].copy_from_slice(&(64i64 << 30).to_le_bytes());
    let err = traces(records(vec![payload(Ty::Spans, bomb)])).unwrap_err();
    assert!(limit_named(&err, "decoded bytes per batch"), "{err}");

    // A negative declared size other than the "not compressed" marker.
    let mut negative = good;
    negative[prefix..prefix + 8].copy_from_slice(&(-2i64).to_le_bytes());
    let err = traces(records(vec![payload(Ty::Spans, negative)])).unwrap_err();
    assert!(matches!(err, OtapError::Malformed { .. }), "{err}");
}

#[test]
fn an_honest_decompression_bomb_stays_within_the_budget() {
    // 8 MiB of zeros compress to a few hundred bytes.
    let zeros = vec![0u8; 8 << 20];
    let blob = table(vec![(
        Field::new("trace_id", DataType::Binary, true),
        Arc::new(BinaryArray::from_iter_values([&zeros[..]])) as ArrayRef,
    )]);
    let bomb = ipc_with(&[&blob], zstd());
    assert!(bomb.len() < 4096, "{} bytes", bomb.len());
    let limits = OtapLimits {
        max_decoded_bytes_per_batch: 1 << 20,
        ..OtapLimits::default()
    };
    let err = traces_with(limits, records(vec![payload(Ty::Spans, bomb.clone())])).unwrap_err();
    assert!(limit_named(&err, "decoded bytes per batch"), "{err}");

    // The budget is per batch, not per table: two 8 MiB tables against 12 MiB.
    let limits = OtapLimits {
        max_decoded_bytes_per_batch: 12 << 20,
        ..OtapLimits::default()
    };
    let err = traces_with(
        limits,
        records(vec![
            payload(Ty::Spans, bomb.clone()),
            payload(Ty::SpanLinks, bomb),
        ]),
    )
    .unwrap_err();
    assert!(limit_named(&err, "decoded bytes per batch"), "{err}");
}

#[test]
fn lz4_ipc_compression_is_refused() {
    let lz4 = IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::LZ4_FRAME))
        .unwrap();
    let err = traces(records(vec![payload(
        Ty::Spans,
        ipc_with(&[&spans([1])], lz4),
    )]))
    .unwrap_err();
    assert!(
        matches!(&err, OtapError::Malformed { reason, .. } if reason.contains("zstd")),
        "{err}"
    );
}

#[test]
fn dictionary_expansion_is_budgeted() {
    // One 1 MiB dictionary value referenced by 100 000 rows: ~1 MiB on the
    // wire, 100 GiB once expanded.
    let big = "x".repeat(1 << 20);
    let keys = UInt32Array::from(vec![0u32; 100_000]);
    let names = arrow::array::DictionaryArray::new(keys, strs([big.as_str()]));
    let spans = table(vec![(
        Field::new("name", names.data_type().clone(), true),
        Arc::new(names) as ArrayRef,
    )]);
    let err = traces(batch_of(vec![(Ty::Spans, spans)])).unwrap_err();
    assert!(limit_named(&err, "decoded bytes per batch"), "{err}");
}

#[test]
fn delta_dictionary_growth_over_a_stream_is_capped() {
    let dict_type = DataType::Dictionary(Box::new(DataType::UInt32), Box::new(DataType::Utf8));
    let schema = Arc::new(Schema::new(vec![Field::new("name", dict_type, true)]));
    let options = IpcWriteOptions::default().with_dictionary_handling(DictionaryHandling::Delta);
    let mut writer = StreamWriter::try_new_with_options(Vec::new(), &schema, options).unwrap();
    let mut consumer = StreamConsumer::new(OtapLimits {
        max_rows_per_table: 100,
        ..OtapLimits::default()
    });
    let mut all: Vec<String> = Vec::new();
    let mut result = Ok(());
    for round in 0..5 {
        all.extend((0..40).map(|i| format!("name-{round}-{i}")));
        let keys = UInt32Array::from_iter_values(
            (0..40).map(|i| u32::try_from(all.len() - 40 + i).unwrap()),
        );
        let names = arrow::array::DictionaryArray::new(keys, strs(all.iter().map(String::as_str)));
        let rb = RecordBatch::try_new(schema.clone(), vec![Arc::new(names)]).unwrap();
        writer.write(&rb).unwrap();
        let record = std::mem::take(writer.get_mut());
        result = consumer
            .consume(records(vec![payload(Ty::Spans, record)]))
            .map(|_| ());
        if result.is_err() {
            assert!(
                round >= 2,
                "the first 80 delta rows fit, failed in round {round}"
            );
            break;
        }
    }
    let err = result.unwrap_err();
    assert!(limit_named(&err, "dictionary rows per stream"), "{err}");
}

// ---------------------------------------------------------------------------
// 3. Attribute joins
// ---------------------------------------------------------------------------

#[test]
fn duplicate_ids_do_not_multiply_attributes() {
    // 100 000 spans that all claim id 0, 100 000 attributes of span 0:
    // joining by clone would build 10^10 attributes.
    let n = 100_000;
    let err = traces(batch_of(vec![
        (Ty::Spans, spans(std::iter::repeat_n(0, n))),
        (
            Ty::SpanAttrs,
            attrs(
                plain("parent_id", DataType::UInt16),
                u16s(std::iter::repeat_n(0, n)),
                "k",
                "v",
            ),
        ),
    ]))
    .unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "SPANS", column, reason }
            if column == "id" && reason.contains("duplicate id 0")),
        "{err}"
    );

    // Events and data points are joined the same way.
    let events = table(vec![
        (
            plain("id", DataType::UInt32),
            Arc::new(UInt32Array::from(vec![4, 4])) as ArrayRef,
        ),
        (plain("parent_id", DataType::UInt16), u16s([1, 1])),
        (Field::new("name", DataType::Utf8, true), strs(["a", "b"])),
    ]);
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanEvents, events),
    ]))
    .unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "SPAN_EVENTS", reason, .. } if reason.contains("duplicate id 4")),
        "{err}"
    );
}

#[test]
fn attribute_count_is_capped_per_batch() {
    let limits = OtapLimits {
        max_attributes_per_batch: 10,
        ..OtapLimits::default()
    };
    let batch = |n: u16| {
        batch_of(vec![
            (Ty::Spans, spans([1])),
            (
                Ty::SpanAttrs,
                attrs(
                    plain("parent_id", DataType::UInt16),
                    u16s(std::iter::repeat_n(1, usize::from(n) / 2)),
                    "k",
                    "v",
                ),
            ),
            (
                Ty::ResourceAttrs,
                attrs(
                    plain("parent_id", DataType::UInt16),
                    u16s(std::iter::repeat_n(0, usize::from(n) - usize::from(n) / 2)),
                    "service.name",
                    "svc",
                ),
            ),
        ])
    };
    // SPANS has no resource column here, so resource attributes of id 0 are
    // orphans; give the span a resource to keep the cap the only failure.
    let with_resource = |mut records: BatchArrowRecords| {
        let resource =
            StructArray::from(vec![(Arc::new(plain("id", DataType::UInt16)), u16s([0]))]);
        let spans = table(vec![
            (plain("id", DataType::UInt16), u16s([1])),
            (
                Field::new("resource", resource.data_type().clone(), true),
                Arc::new(resource) as ArrayRef,
            ),
        ]);
        records.arrow_payloads[0] = payload(Ty::Spans, ipc(&spans));
        records
    };
    assert_eq!(traces_with(limits, with_resource(batch(10))).unwrap(), 1);
    let err = traces_with(limits, with_resource(batch(11))).unwrap_err();
    assert!(limit_named(&err, "attributes per batch"), "{err}");
}

/// A SPANS table whose rows carry the given resource and scope ids.
fn spans_in(groups: &[(u16, u16)]) -> RecordBatch {
    let n = u16::try_from(groups.len()).unwrap();
    let resource = StructArray::from(vec![(
        Arc::new(plain("id", DataType::UInt16)),
        u16s(groups.iter().map(|g| g.0)),
    )]);
    let scope = StructArray::from(vec![(
        Arc::new(plain("id", DataType::UInt16)),
        u16s(groups.iter().map(|g| g.1)),
    )]);
    table(vec![
        (plain("id", DataType::UInt16), u16s(0..n)),
        (
            Field::new("resource", resource.data_type().clone(), true),
            Arc::new(resource) as ArrayRef,
        ),
        (
            Field::new("scope", scope.data_type().clone(), true),
            Arc::new(scope) as ArrayRef,
        ),
    ])
}

#[test]
fn alternating_resource_ids_share_one_group_each() {
    // Rows alternate between two resources; each resource's attributes must
    // be attached once, not once per alternation.
    let rows: Vec<(u16, u16)> = (0..1000).map(|i| (i % 2, i % 2)).collect();
    let resource_attrs = attrs(
        plain("parent_id", DataType::UInt16),
        u16s((0..200).map(|i| i % 2)),
        "k",
        "v",
    );
    let batch = StreamConsumer::default()
        .consume(batch_of(vec![
            (Ty::Spans, spans_in(&rows)),
            (Ty::ResourceAttrs, resource_attrs),
        ]))
        .unwrap();
    let request = to_trace_request(&batch).unwrap();
    assert_eq!(request.resource_spans.len(), 2);
    for rs in &request.resource_spans {
        assert_eq!(rs.resource.as_ref().unwrap().attributes.len(), 100);
        assert_eq!(rs.scope_spans.len(), 1);
        assert_eq!(rs.scope_spans[0].spans.len(), 500);
    }
}

#[test]
fn scope_attributes_copied_to_many_groups_count_against_the_cap() {
    // One scope id under 100 resources: its 100 attributes are copied into
    // every group, 10 000 in total.
    let rows: Vec<(u16, u16)> = (0..100).map(|i| (i, 7)).collect();
    let scope_attrs = attrs(
        plain("parent_id", DataType::UInt16),
        u16s(std::iter::repeat_n(7, 100)),
        "k",
        "v",
    );
    let tables = || {
        batch_of(vec![
            (Ty::Spans, spans_in(&rows)),
            (Ty::ScopeAttrs, scope_attrs.clone()),
        ])
    };
    assert_eq!(traces(tables()).unwrap(), 100);
    let limits = OtapLimits {
        max_attributes_per_batch: 5_000,
        ..OtapLimits::default()
    };
    let err = traces_with(limits, tables()).unwrap_err();
    assert!(limit_named(&err, "attributes per batch"), "{err}");
}

// ---------------------------------------------------------------------------
// 6. Nothing is dropped silently
// ---------------------------------------------------------------------------

#[test]
fn trailing_record_batch_is_refused() {
    let one = spans([1]);
    let two = spans([2]);
    let record = ipc_with(&[&one, &two], IpcWriteOptions::default());
    let err = traces(records(vec![payload(Ty::Spans, record)])).unwrap_err();
    assert!(
        matches!(&err, OtapError::Malformed { payload: "SPANS", reason } if reason.contains("after the record batch")),
        "{err}"
    );
    // Trailing garbage and an end-of-stream marker are refused the same way.
    for tail in [
        &b"\x01\x02\x03"[..],
        &[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0][..],
    ] {
        let mut record = ipc(&one);
        record.extend_from_slice(tail);
        let err = traces(records(vec![payload(Ty::Spans, record)])).unwrap_err();
        assert!(matches!(err, OtapError::Malformed { .. }), "{err}");
    }
}

#[test]
fn duplicate_payload_type_is_refused() {
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::Spans, spans([2])),
    ]))
    .unwrap_err();
    assert!(matches!(err, OtapError::DuplicatePayload("SPANS")), "{err}");
}

#[test]
fn schema_only_payload_is_refused_and_poisons_the_stream() {
    let schema_only = {
        let writer = StreamWriter::try_new(Vec::new(), spans([1]).schema_ref()).unwrap();
        writer.get_ref().clone()
    };
    let mut consumer = StreamConsumer::default();
    let err = consumer
        .consume(records(vec![payload(Ty::Spans, schema_only)]))
        .unwrap_err();
    assert!(matches!(err, OtapError::EmptyPayload { .. }), "{err}");
    // The IPC state of the stream is unknown now: nothing more is decoded.
    let err = consumer
        .consume(batch_of(vec![(Ty::Spans, spans([1]))]))
        .unwrap_err();
    assert!(matches!(err, OtapError::Poisoned), "{err}");
}

#[test]
fn orphan_children_fail_the_batch() {
    let span_attrs = |parent: u16| {
        attrs(
            plain("parent_id", DataType::UInt16),
            u16s([parent]),
            "k",
            "v",
        )
    };
    let orphan = |err: &OtapError, expected: &str| {
        matches!(err, OtapError::Column { table, column, reason }
            if *table == expected && column == "parent_id" && reason.contains("no parent"))
    };

    assert_eq!(
        traces(batch_of(vec![
            (Ty::Spans, spans([1])),
            (Ty::SpanAttrs, span_attrs(1)),
        ]))
        .unwrap(),
        1
    );
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanAttrs, span_attrs(9)),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "SPAN_ATTRS"), "{err}");

    // Resource attributes of a resource no span refers to.
    let err = traces(batch_of(vec![
        (Ty::Spans, spans_in(&[(0, 0)])),
        (Ty::ResourceAttrs, span_attrs(5)),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "RESOURCE_ATTRS"), "{err}");
    let err = traces(batch_of(vec![
        (Ty::Spans, spans_in(&[(0, 0)])),
        (Ty::ScopeAttrs, span_attrs(5)),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "SCOPE_ATTRS"), "{err}");

    // An event of a span that is not in the batch, and event attributes of
    // an event that is not.
    let event = |parent: u16| {
        table(vec![
            (
                plain("id", DataType::UInt32),
                Arc::new(UInt32Array::from(vec![0])) as ArrayRef,
            ),
            (plain("parent_id", DataType::UInt16), u16s([parent])),
            (Field::new("name", DataType::Utf8, true), strs(["e"])),
        ])
    };
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanEvents, event(2)),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "SPAN_EVENTS"), "{err}");
    let event_attrs = attrs(
        plain("parent_id", DataType::UInt32),
        Arc::new(UInt32Array::from(vec![3])),
        "k",
        "v",
    );
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanEvents, event(1)),
        (Ty::SpanEventAttrs, event_attrs),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "SPAN_EVENT_ATTRS"), "{err}");

    // A child row without any parent id.
    let no_parent = attrs(
        plain("parent_id", DataType::UInt16),
        Arc::new(UInt16Array::from(vec![None])),
        "k",
        "v",
    );
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanAttrs, no_parent),
    ]))
    .unwrap_err();
    assert!(orphan(&err, "SPAN_ATTRS"), "{err}");

    // Log attributes.
    let logs = table(vec![(plain("id", DataType::UInt16), u16s([1]))]);
    let batch = StreamConsumer::default()
        .consume(batch_of(vec![
            (Ty::Logs, logs),
            (Ty::LogAttrs, span_attrs(2)),
        ]))
        .unwrap();
    let err = to_logs_request(&batch).unwrap_err();
    assert!(orphan(&err, "LOG_ATTRS"), "{err}");
}

/// A UNIVARIATE_METRICS table of one metric with the given type code.
fn metrics_table(metric_type: u8) -> RecordBatch {
    table(vec![
        (plain("id", DataType::UInt16), u16s([1])),
        (
            Field::new("metric_type", DataType::UInt8, false),
            Arc::new(UInt8Array::from(vec![metric_type])) as ArrayRef,
        ),
        (Field::new("name", DataType::Utf8, true), strs(["m"])),
    ])
}

fn number_points(parent: u16, int: Option<i64>, double: Option<f64>) -> RecordBatch {
    table(vec![
        (
            plain("id", DataType::UInt32),
            Arc::new(UInt32Array::from(vec![0])) as ArrayRef,
        ),
        (plain("parent_id", DataType::UInt16), u16s([parent])),
        (
            Field::new("int_value", DataType::Int64, true),
            Arc::new(Int64Array::from(vec![int])) as ArrayRef,
        ),
        (
            Field::new("double_value", DataType::Float64, true),
            Arc::new(Float64Array::from(vec![double])) as ArrayRef,
        ),
    ])
}

fn metrics(records: BatchArrowRecords) -> Result<usize, OtapError> {
    let batch = StreamConsumer::default().consume(records)?;
    let request = to_metrics_request(&batch)?;
    Ok(tumult_otlp::metrics_request_to_rows(&request).gauges.len())
}

#[test]
fn orphan_data_points_fail_the_batch() {
    const GAUGE: u8 = 1;
    const HISTOGRAM: u8 = 3;
    assert_eq!(
        metrics(batch_of(vec![
            (Ty::UnivariateMetrics, metrics_table(GAUGE)),
            (Ty::NumberDataPoints, number_points(1, Some(3), None)),
        ]))
        .unwrap(),
        1
    );
    for (metric_type, parent) in [(GAUGE, 2), (HISTOGRAM, 1)] {
        let err = metrics(batch_of(vec![
            (Ty::UnivariateMetrics, metrics_table(metric_type)),
            (Ty::NumberDataPoints, number_points(parent, Some(3), None)),
        ]))
        .unwrap_err();
        assert!(
            matches!(&err, OtapError::Column { table: "NUMBER_DATA_POINTS", reason, .. } if reason.contains("no parent")),
            "type {metric_type}, parent {parent}: {err}"
        );
    }
    // Data point attributes of a point that is not there.
    let dp_attrs = attrs(
        plain("parent_id", DataType::UInt32),
        Arc::new(UInt32Array::from(vec![8])),
        "k",
        "v",
    );
    let err = metrics(batch_of(vec![
        (Ty::UnivariateMetrics, metrics_table(GAUGE)),
        (Ty::NumberDataPoints, number_points(1, Some(3), None)),
        (Ty::NumberDpAttrs, dp_attrs),
    ]))
    .unwrap_err();
    assert!(
        matches!(
            &err,
            OtapError::Column {
                table: "NUMBER_DP_ATTRS",
                ..
            }
        ),
        "{err}"
    );
    // Summary points are only counted, but still need a summary metric.
    let summary = table(vec![(plain("parent_id", DataType::UInt16), u16s([4]))]);
    let err = metrics(batch_of(vec![
        (Ty::UnivariateMetrics, metrics_table(GAUGE)),
        (Ty::SummaryDataPoints, summary),
    ]))
    .unwrap_err();
    assert!(
        matches!(
            &err,
            OtapError::Column {
                table: "SUMMARY_DATA_POINTS",
                ..
            }
        ),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// 7-9. Id width, reader keys, number values
// ---------------------------------------------------------------------------

#[test]
fn u16_parent_delta_wraps_at_the_column_width() {
    // Equal key and value: the second parent id is a delta from the first.
    // 65535 + 2 is 1 in a uint16 column, not 65537.
    let span_attrs = attrs(
        delta("parent_id", DataType::UInt16),
        u16s([65535, 2]),
        "k",
        "v",
    );
    let batch = StreamConsumer::default()
        .consume(batch_of(vec![
            (Ty::Spans, spans([65535, 1])),
            (Ty::SpanAttrs, span_attrs),
        ]))
        .unwrap();
    let request = to_trace_request(&batch).unwrap();
    let spans_out = &request.resource_spans[0].scope_spans[0].spans;
    assert_eq!(spans_out[0].attributes.len(), 1);
    assert_eq!(
        spans_out[1].attributes.len(),
        1,
        "wrapped parent id found its span"
    );

    // Events delta-encode the parent id over equal names the same way.
    let events = table(vec![
        (delta("parent_id", DataType::UInt16), u16s([65535, 2])),
        (Field::new("name", DataType::Utf8, true), strs(["e", "e"])),
    ]);
    let batch = StreamConsumer::default()
        .consume(batch_of(vec![
            (Ty::Spans, spans([65535, 1])),
            (Ty::SpanEvents, events),
        ]))
        .unwrap();
    let request = to_trace_request(&batch).unwrap();
    let spans_out = &request.resource_spans[0].scope_spans[0].spans;
    assert_eq!(spans_out[1].events.len(), 1);
}

#[test]
fn schema_id_reused_across_payload_types_gets_its_own_reader() {
    let mut consumer = StreamConsumer::default();
    let mut first = batch_of(vec![(Ty::Spans, spans([1]))]);
    first.arrow_payloads[0].schema_id = "shared".into();
    consumer.consume(first).unwrap();

    // A LOGS payload under the same schema id must not be read with the
    // SPANS reader (and its dictionaries).
    let logs = table(vec![
        (plain("id", DataType::UInt16), u16s([1])),
        (
            Field::new("severity_text", DataType::Utf8, true),
            strs(["WARN"]),
        ),
    ]);
    let mut second = batch_of(vec![(Ty::Logs, logs)]);
    second.arrow_payloads[0].schema_id = "shared".into();
    let batch = consumer.consume(second).unwrap();
    let request = to_logs_request(&batch).unwrap();
    assert_eq!(
        request.resource_logs[0].scope_logs[0].log_records[0].severity_text,
        "WARN"
    );

    // A continuation (no schema message) labelled with another type's
    // schema id has no reader of its own and is refused.
    let mut writer = StreamWriter::try_new(Vec::new(), spans([1]).schema_ref()).unwrap();
    writer.write(&spans([1])).unwrap();
    writer.get_mut().clear();
    writer.write(&spans([2])).unwrap();
    let continuation = std::mem::take(writer.get_mut());
    let mut third = records(vec![payload(Ty::SpanLinks, continuation)]);
    third.arrow_payloads[0].schema_id = "shared".into();
    let err = consumer.consume(third).unwrap_err();
    assert!(
        matches!(&err, OtapError::Malformed { payload: "SPAN_LINKS", reason } if reason.contains("schema")),
        "{err}"
    );
}

#[test]
fn number_point_with_both_values_is_refused() {
    const GAUGE: u8 = 1;
    let err = metrics(batch_of(vec![
        (Ty::UnivariateMetrics, metrics_table(GAUGE)),
        (Ty::NumberDataPoints, number_points(1, Some(3), Some(3.5))),
    ]))
    .unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "NUMBER_DATA_POINTS", reason, .. }
            if reason.contains("both int_value and double_value")),
        "{err}"
    );
    assert_eq!(
        metrics(batch_of(vec![
            (Ty::UnivariateMetrics, metrics_table(GAUGE)),
            (Ty::NumberDataPoints, number_points(1, None, Some(3.5))),
        ]))
        .unwrap(),
        1
    );
}

// ---------------------------------------------------------------------------
// 12. Error text
// ---------------------------------------------------------------------------

#[test]
fn value_errors_name_the_real_table_and_ipc_errors_carry_no_arrow_text() {
    let mut bad_type = attrs(plain("parent_id", DataType::UInt16), u16s([1]), "k", "v");
    let mut columns = bad_type.columns().to_vec();
    columns[2] = Arc::new(UInt8Array::from(vec![99u8]));
    bad_type = RecordBatch::try_new(bad_type.schema(), columns).unwrap();
    let err = traces(batch_of(vec![
        (Ty::Spans, spans([1])),
        (Ty::SpanAttrs, bad_type),
    ]))
    .unwrap_err();
    assert!(
        matches!(&err, OtapError::Column { table: "SPAN_ATTRS", column, .. } if column == "type"),
        "{err}"
    );

    // A body arrow refuses after the pre-scan: offsets that run backwards.
    let good = ipc(&table(vec![(
        Field::new("name", DataType::Utf8, true),
        strs(["abcdefgh", "ijklmnop"]),
    )]));
    let mut bad = good;
    patch(
        &mut bad,
        &[8, 0, 0, 0, 16, 0, 0, 0],
        &[8, 0, 0, 0, 2, 0, 0, 0],
    );
    let err = traces(records(vec![payload(Ty::Spans, bad)])).unwrap_err();
    assert!(matches!(err, OtapError::Ipc { .. }), "{err}");
    assert_eq!(err.to_string(), "arrow IPC decode failed for SPANS");
}
