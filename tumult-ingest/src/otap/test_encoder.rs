//! A test-only OTAP producer.
//!
//! It mirrors the Collector's encoder closely enough to exercise every
//! decode path: delta-encoded ids (with `encoding` metadata), attribute
//! tables sorted by (key, value) with quasi-delta parent ids, span events
//! sorted by name, dictionary-encoded string and binary columns, struct
//! columns with nulls, and zstd-compressed IPC payloads spread over a
//! long-lived stream (schema message once, then record batches only).

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, DictionaryArray, DurationNanosecondArray,
    FixedSizeBinaryArray, Float64Array, Float64Builder, Int32Array, Int64Array, ListBuilder,
    RecordBatch, StringArray, StructArray, TimestampNanosecondArray, UInt16Array, UInt32Array,
    UInt64Array, UInt64Builder, UInt8Array,
};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{Field, Fields, Schema, UInt8Type};
use arrow::ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow::ipc::CompressionType;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{metric::Data, number_data_point};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::Event;

use super::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords};

pub(super) struct Encoder {
    writers: HashMap<i32, StreamWriter<Vec<u8>>>,
    next_batch_id: i64,
    compress: bool,
}

impl Encoder {
    pub fn new(compress: bool) -> Self {
        Self {
            writers: HashMap::new(),
            next_batch_id: 1,
            compress,
        }
    }

    fn payload(&mut self, ty: ArrowPayloadType, batch: &RecordBatch) -> ArrowPayload {
        let writer = match self.writers.entry(ty as i32) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let mut options = IpcWriteOptions::default();
                if self.compress {
                    options = options
                        .try_with_compression(Some(CompressionType::ZSTD))
                        .unwrap();
                }
                e.insert(
                    StreamWriter::try_new_with_options(Vec::new(), batch.schema_ref(), options)
                        .unwrap(),
                )
            }
        };
        writer.write(batch).unwrap();
        ArrowPayload {
            schema_id: format!("{}-1", ty.as_str_name()),
            r#type: ty as i32,
            record: std::mem::take(writer.get_mut()),
        }
    }

    fn records(&mut self, tables: Vec<(ArrowPayloadType, RecordBatch)>) -> BatchArrowRecords {
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;
        BatchArrowRecords {
            batch_id,
            arrow_payloads: tables
                .iter()
                .map(|(ty, rb)| self.payload(*ty, rb))
                .collect(),
            headers: Vec::new(),
        }
    }

    pub fn traces(&mut self, req: &ExportTraceServiceRequest) -> BatchArrowRecords {
        let mut ctx = RootRows::default();
        let mut span_attrs = Vec::new();
        let mut events: Vec<(u16, &Event)> = Vec::new();
        let mut spans = Vec::new();
        for rs in &req.resource_spans {
            let rid = ctx.resource(rs.resource.as_ref(), &rs.schema_url);
            for ss in &rs.scope_spans {
                let sid = ctx.scope(ss.scope.as_ref(), &ss.schema_url);
                for span in &ss.spans {
                    let id = u16::try_from(spans.len()).unwrap();
                    ctx.row(rid, sid, id);
                    span_attrs.extend(span.attributes.iter().map(|kv| (u32::from(id), kv)));
                    events.extend(span.events.iter().map(|e| (id, e)));
                    spans.push(span);
                }
            }
        }
        let mut cols = ctx.columns();
        cols.push(
            "start_time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                spans
                    .iter()
                    .map(|s| s.start_time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "duration_time_unix_nano",
            Arc::new(DurationNanosecondArray::from(
                spans
                    .iter()
                    .map(|s| (s.end_time_unix_nano - s.start_time_unix_nano) as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "trace_id",
            dict8(fixed_binary(
                spans.iter().map(|s| Some(s.trace_id.as_slice())),
                16,
            )),
        );
        cols.push(
            "span_id",
            fixed_binary(spans.iter().map(|s| Some(s.span_id.as_slice())), 8),
        );
        cols.push(
            "parent_span_id",
            fixed_binary(
                spans
                    .iter()
                    .map(|s| (!s.parent_span_id.is_empty()).then_some(s.parent_span_id.as_slice())),
                8,
            ),
        );
        cols.push(
            "trace_state",
            strings(
                spans
                    .iter()
                    .map(|s| (!s.trace_state.is_empty()).then_some(s.trace_state.as_str())),
            ),
        );
        cols.push("flags", u32s(spans.iter().map(|s| s.flags)));
        cols.push(
            "name",
            dict8(strings(spans.iter().map(|s| Some(s.name.as_str())))),
        );
        cols.push(
            "kind",
            Arc::new(Int32Array::from(
                spans.iter().map(|s| s.kind).collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "dropped_attributes_count",
            u32s(spans.iter().map(|s| s.dropped_attributes_count)),
        );
        cols.push(
            "dropped_events_count",
            u32s(spans.iter().map(|s| s.dropped_events_count)),
        );
        cols.push(
            "dropped_links_count",
            u32s(spans.iter().map(|s| s.dropped_links_count)),
        );
        let status_fields = vec![
            Field::new("code", arrow::datatypes::DataType::Int32, true),
            Field::new("status_message", arrow::datatypes::DataType::Utf8, true),
        ];
        let status = StructArray::try_new(
            Fields::from(status_fields),
            vec![
                Arc::new(Int32Array::from(
                    spans
                        .iter()
                        .map(|s| s.status.as_ref().map(|st| st.code))
                        .collect::<Vec<_>>(),
                )),
                strings(
                    spans
                        .iter()
                        .map(|s| s.status.as_ref().map(|st| st.message.as_str())),
                ),
            ],
            Some(NullBuffer::from(
                spans.iter().map(|s| s.status.is_some()).collect::<Vec<_>>(),
            )),
        )
        .unwrap();
        cols.push("status", Arc::new(status));

        // Events sorted by name (what the Collector does); ids follow the
        // sorted order and event attributes reference those ids.
        events.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(&b.0)));
        let mut event_attrs = Vec::new();
        for (i, (_, e)) in events.iter().enumerate() {
            event_attrs.extend(
                e.attributes
                    .iter()
                    .map(|kv| (u32::try_from(i).unwrap(), kv)),
            );
        }
        let mut tables = vec![(ArrowPayloadType::Spans, cols.batch())];
        tables.extend(ctx.attr_tables());
        if !span_attrs.is_empty() {
            tables.push((ArrowPayloadType::SpanAttrs, attrs_table(span_attrs, false)));
        }
        if !events.is_empty() {
            tables.push((ArrowPayloadType::SpanEvents, events_table(&events)));
        }
        if !event_attrs.is_empty() {
            tables.push((
                ArrowPayloadType::SpanEventAttrs,
                attrs_table(event_attrs, true),
            ));
        }
        self.records(tables)
    }

    pub fn logs(&mut self, req: &ExportLogsServiceRequest) -> BatchArrowRecords {
        let mut ctx = RootRows::default();
        let mut log_attrs = Vec::new();
        let mut logs = Vec::new();
        for rl in &req.resource_logs {
            let rid = ctx.resource(rl.resource.as_ref(), &rl.schema_url);
            for sl in &rl.scope_logs {
                let sid = ctx.scope(sl.scope.as_ref(), &sl.schema_url);
                for record in &sl.log_records {
                    let id = u16::try_from(logs.len()).unwrap();
                    ctx.row(rid, sid, id);
                    log_attrs.extend(record.attributes.iter().map(|kv| (u32::from(id), kv)));
                    logs.push(record);
                }
            }
        }
        let mut cols = ctx.columns();
        cols.push(
            "time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                logs.iter()
                    .map(|l| l.time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "observed_time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                logs.iter()
                    .map(|l| l.observed_time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "trace_id",
            fixed_binary(
                logs.iter()
                    .map(|l| (!l.trace_id.is_empty()).then_some(l.trace_id.as_slice())),
                16,
            ),
        );
        cols.push(
            "span_id",
            fixed_binary(
                logs.iter()
                    .map(|l| (!l.span_id.is_empty()).then_some(l.span_id.as_slice())),
                8,
            ),
        );
        cols.push(
            "severity_number",
            Arc::new(Int32Array::from(
                logs.iter().map(|l| l.severity_number).collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "severity_text",
            dict8(strings(logs.iter().map(|l| Some(l.severity_text.as_str())))),
        );
        let bodies: Vec<Option<&any_value::Value>> = logs
            .iter()
            .map(|l| l.body.as_ref().and_then(|b| b.value.as_ref()))
            .collect();
        let (body_fields, body_arrays) = value_columns(&bodies);
        let body = StructArray::try_new(
            Fields::from(body_fields),
            body_arrays,
            Some(NullBuffer::from(
                logs.iter().map(|l| l.body.is_some()).collect::<Vec<_>>(),
            )),
        )
        .unwrap();
        cols.push("body", Arc::new(body));
        cols.push(
            "dropped_attributes_count",
            u32s(logs.iter().map(|l| l.dropped_attributes_count)),
        );
        cols.push("flags", u32s(logs.iter().map(|l| l.flags)));
        cols.push(
            "event_name",
            strings(logs.iter().map(|l| Some(l.event_name.as_str()))),
        );
        let mut tables = vec![(ArrowPayloadType::Logs, cols.batch())];
        tables.extend(ctx.attr_tables());
        if !log_attrs.is_empty() {
            tables.push((ArrowPayloadType::LogAttrs, attrs_table(log_attrs, false)));
        }
        self.records(tables)
    }

    pub fn metrics(&mut self, req: &ExportMetricsServiceRequest) -> BatchArrowRecords {
        let mut ctx = RootRows::default();
        let mut metrics = Vec::new();
        let mut numbers = NumberPoints::default();
        let mut histograms = HistogramPoints::default();
        let mut summaries: Vec<u16> = Vec::new();
        let mut exp_histograms: Vec<u16> = Vec::new();
        for rm in &req.resource_metrics {
            let rid = ctx.resource(rm.resource.as_ref(), &rm.schema_url);
            for sm in &rm.scope_metrics {
                let sid = ctx.scope(sm.scope.as_ref(), &sm.schema_url);
                for metric in &sm.metrics {
                    let id = u16::try_from(metrics.len()).unwrap();
                    ctx.row(rid, sid, id);
                    match &metric.data {
                        Some(Data::Gauge(g)) => numbers.extend(id, &g.data_points),
                        Some(Data::Sum(s)) => numbers.extend(id, &s.data_points),
                        Some(Data::Histogram(h)) => histograms.extend(id, &h.data_points),
                        Some(Data::Summary(s)) => {
                            summaries.extend(std::iter::repeat_n(id, s.data_points.len()));
                        }
                        Some(Data::ExponentialHistogram(e)) => {
                            exp_histograms.extend(std::iter::repeat_n(id, e.data_points.len()));
                        }
                        None => {}
                    }
                    metrics.push(metric);
                }
            }
        }
        let mut cols = ctx.columns();
        cols.push(
            "metric_type",
            Arc::new(UInt8Array::from(
                metrics
                    .iter()
                    .map(|m| match &m.data {
                        Some(Data::Gauge(_)) => 1,
                        Some(Data::Sum(_)) => 2,
                        Some(Data::Histogram(_)) => 3,
                        Some(Data::ExponentialHistogram(_)) => 4,
                        Some(Data::Summary(_)) => 5,
                        None => 0,
                    })
                    .collect::<Vec<u8>>(),
            )),
        );
        cols.push(
            "name",
            dict8(strings(metrics.iter().map(|m| Some(m.name.as_str())))),
        );
        cols.push(
            "description",
            strings(metrics.iter().map(|m| Some(m.description.as_str()))),
        );
        cols.push(
            "unit",
            strings(metrics.iter().map(|m| Some(m.unit.as_str()))),
        );
        cols.push(
            "aggregation_temporality",
            Arc::new(Int32Array::from(
                metrics
                    .iter()
                    .map(|m| match &m.data {
                        Some(Data::Sum(s)) => Some(s.aggregation_temporality),
                        Some(Data::Histogram(h)) => Some(h.aggregation_temporality),
                        Some(Data::ExponentialHistogram(e)) => Some(e.aggregation_temporality),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "is_monotonic",
            Arc::new(BooleanArray::from(
                metrics
                    .iter()
                    .map(|m| match &m.data {
                        Some(Data::Sum(s)) => Some(s.is_monotonic),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )),
        );
        let mut tables = vec![(ArrowPayloadType::UnivariateMetrics, cols.batch())];
        tables.extend(ctx.attr_tables());
        tables.extend(numbers.tables());
        tables.extend(histograms.tables());
        if !summaries.is_empty() {
            tables.push((
                ArrowPayloadType::SummaryDataPoints,
                parent_only_table(&summaries),
            ));
        }
        if !exp_histograms.is_empty() {
            tables.push((
                ArrowPayloadType::ExpHistogramDataPoints,
                parent_only_table(&exp_histograms),
            ));
        }
        self.records(tables)
    }
}

/// Resource and scope bookkeeping shared by the three root tables.
#[derive(Default)]
struct RootRows<'a> {
    resources: Vec<(Option<&'a Resource>, &'a str)>,
    scopes: Vec<(Option<&'a InstrumentationScope>, &'a str)>,
    rows: Vec<(u16, u16, u16)>,
}

impl<'a> RootRows<'a> {
    fn resource(&mut self, resource: Option<&'a Resource>, schema_url: &'a str) -> u16 {
        self.resources.push((resource, schema_url));
        u16::try_from(self.resources.len() - 1).unwrap()
    }

    fn scope(&mut self, scope: Option<&'a InstrumentationScope>, schema_url: &'a str) -> u16 {
        self.scopes.push((scope, schema_url));
        u16::try_from(self.scopes.len() - 1).unwrap()
    }

    fn row(&mut self, rid: u16, sid: u16, id: u16) {
        self.rows.push((rid, sid, id));
    }

    fn columns(&self) -> Columns {
        let mut cols = Columns::default();
        cols.push("id", u16_delta(self.rows.iter().map(|r| r.2)));
        let resource = StructArray::try_new(
            Fields::from(vec![
                delta_field("id", arrow::datatypes::DataType::UInt16),
                Field::new("schema_url", arrow::datatypes::DataType::Utf8, true),
                Field::new(
                    "dropped_attributes_count",
                    arrow::datatypes::DataType::UInt32,
                    true,
                ),
            ]),
            vec![
                u16_delta(self.rows.iter().map(|r| r.0)),
                strings(
                    self.rows
                        .iter()
                        .map(|r| Some(self.resources[r.0 as usize].1)),
                ),
                u32s(self.rows.iter().map(|r| {
                    self.resources[r.0 as usize]
                        .0
                        .map_or(0, |res| res.dropped_attributes_count)
                })),
            ],
            None,
        )
        .unwrap();
        cols.push("resource", Arc::new(resource));
        let scope = StructArray::try_new(
            Fields::from(vec![
                delta_field("id", arrow::datatypes::DataType::UInt16),
                Field::new(
                    "name",
                    arrow::datatypes::DataType::Dictionary(
                        Box::new(arrow::datatypes::DataType::UInt8),
                        Box::new(arrow::datatypes::DataType::Utf8),
                    ),
                    true,
                ),
                Field::new("version", arrow::datatypes::DataType::Utf8, true),
                Field::new(
                    "dropped_attributes_count",
                    arrow::datatypes::DataType::UInt32,
                    true,
                ),
            ]),
            vec![
                u16_delta(self.rows.iter().map(|r| r.1)),
                dict8(strings(self.rows.iter().map(|r| {
                    self.scopes[r.1 as usize].0.map(|s| s.name.as_str())
                }))),
                strings(
                    self.rows
                        .iter()
                        .map(|r| self.scopes[r.1 as usize].0.map(|s| s.version.as_str())),
                ),
                u32s(self.rows.iter().map(|r| {
                    self.scopes[r.1 as usize]
                        .0
                        .map_or(0, |s| s.dropped_attributes_count)
                })),
            ],
            None,
        )
        .unwrap();
        cols.push("scope", Arc::new(scope));
        cols.push(
            "schema_url",
            strings(self.rows.iter().map(|r| Some(self.scopes[r.1 as usize].1))),
        );
        cols
    }

    fn attr_tables(&self) -> Vec<(ArrowPayloadType, RecordBatch)> {
        let mut out = Vec::new();
        let resource_attrs: Vec<(u32, &KeyValue)> = self
            .resources
            .iter()
            .enumerate()
            .flat_map(|(i, (r, _))| {
                r.iter()
                    .flat_map(|r| r.attributes.iter())
                    .map(move |kv| (u32::try_from(i).unwrap(), kv))
            })
            .collect();
        if !resource_attrs.is_empty() {
            out.push((
                ArrowPayloadType::ResourceAttrs,
                attrs_table(resource_attrs, false),
            ));
        }
        let scope_attrs: Vec<(u32, &KeyValue)> = self
            .scopes
            .iter()
            .enumerate()
            .flat_map(|(i, (s, _))| {
                s.iter()
                    .flat_map(|s| s.attributes.iter())
                    .map(move |kv| (u32::try_from(i).unwrap(), kv))
            })
            .collect();
        if !scope_attrs.is_empty() {
            out.push((
                ArrowPayloadType::ScopeAttrs,
                attrs_table(scope_attrs, false),
            ));
        }
        out
    }
}

#[derive(Default)]
struct Columns {
    fields: Vec<Field>,
    arrays: Vec<ArrayRef>,
}

impl Columns {
    fn push(&mut self, name: &str, array: ArrayRef) {
        self.push_field(Field::new(name, array.data_type().clone(), true), array);
    }

    fn push_field(&mut self, field: Field, array: ArrayRef) {
        self.fields.push(field);
        self.arrays.push(array);
    }

    fn batch(self) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(self.fields)), self.arrays).unwrap()
    }
}

fn delta_field(name: &str, data_type: arrow::datatypes::DataType) -> Field {
    Field::new(name, data_type, true).with_metadata(HashMap::from([(
        "encoding".to_string(),
        "delta".to_string(),
    )]))
}

fn u16_delta(values: impl Iterator<Item = u16>) -> ArrayRef {
    let mut prev: u16 = 0;
    Arc::new(UInt16Array::from(
        values
            .map(|v| {
                let delta = v.wrapping_sub(prev);
                prev = v;
                delta
            })
            .collect::<Vec<_>>(),
    ))
}

fn u32s(values: impl Iterator<Item = u32>) -> ArrayRef {
    Arc::new(UInt32Array::from(values.collect::<Vec<_>>()))
}

fn strings<'a>(values: impl Iterator<Item = Option<&'a str>>) -> ArrayRef {
    Arc::new(StringArray::from(values.collect::<Vec<_>>()))
}

fn fixed_binary<'a>(values: impl Iterator<Item = Option<&'a [u8]>>, size: i32) -> ArrayRef {
    Arc::new(FixedSizeBinaryArray::try_from_sparse_iter_with_size(values, size).unwrap())
}

/// Dictionary-encode any array with one key per row (no deduplication:
/// the point is the column *type*, not the compression).
fn dict8(values: ArrayRef) -> ArrayRef {
    let keys = UInt8Array::from(
        (0..values.len())
            .map(|i| u8::try_from(i).unwrap())
            .collect::<Vec<_>>(),
    );
    Arc::new(DictionaryArray::<UInt8Type>::try_new(keys, values).unwrap())
}

fn value_type(value: Option<&any_value::Value>) -> u8 {
    use any_value::Value;
    match value {
        None => 0,
        Some(Value::StringValue(_)) => 1,
        Some(Value::IntValue(_)) => 2,
        Some(Value::DoubleValue(_)) => 3,
        Some(Value::BoolValue(_)) => 4,
        Some(Value::KvlistValue(_)) => 5,
        Some(Value::ArrayValue(_)) => 6,
        Some(Value::BytesValue(_)) => 7,
        Some(_) => 0,
    }
}

fn to_cbor(value: Option<&any_value::Value>) -> ciborium::Value {
    use any_value::Value;
    use ciborium::Value as C;
    match value {
        None => C::Null,
        Some(Value::StringValue(s)) => C::Text(s.clone()),
        Some(Value::BoolValue(b)) => C::Bool(*b),
        Some(Value::IntValue(i)) => C::Integer((*i).into()),
        Some(Value::DoubleValue(d)) => C::Float(*d),
        Some(Value::BytesValue(b)) => C::Bytes(b.clone()),
        Some(Value::ArrayValue(a)) => {
            C::Array(a.values.iter().map(|v| to_cbor(v.value.as_ref())).collect())
        }
        Some(Value::KvlistValue(kv)) => C::Map(
            kv.values
                .iter()
                .map(|kv| {
                    (
                        C::Text(kv.key.clone()),
                        to_cbor(kv.value.as_ref().and_then(|v| v.value.as_ref())),
                    )
                })
                .collect(),
        ),
        Some(_) => C::Null,
    }
}

/// The `type` + typed value columns shared by attribute tables and log bodies.
fn value_columns(values: &[Option<&any_value::Value>]) -> (Vec<Field>, Vec<ArrayRef>) {
    use any_value::Value;
    let types = Arc::new(UInt8Array::from(
        values.iter().map(|v| value_type(*v)).collect::<Vec<_>>(),
    ));
    let strs = dict8(strings(values.iter().map(|v| match v {
        Some(Value::StringValue(s)) => Some(s.as_str()),
        _ => None,
    })));
    let ints = Arc::new(Int64Array::from(
        values
            .iter()
            .map(|v| match v {
                Some(Value::IntValue(i)) => Some(*i),
                _ => None,
            })
            .collect::<Vec<_>>(),
    ));
    let doubles = Arc::new(Float64Array::from(
        values
            .iter()
            .map(|v| match v {
                Some(Value::DoubleValue(d)) => Some(*d),
                _ => None,
            })
            .collect::<Vec<_>>(),
    ));
    let bools = Arc::new(BooleanArray::from(
        values
            .iter()
            .map(|v| match v {
                Some(Value::BoolValue(b)) => Some(*b),
                _ => None,
            })
            .collect::<Vec<_>>(),
    ));
    let bytes = Arc::new(BinaryArray::from(
        values
            .iter()
            .map(|v| match v {
                Some(Value::BytesValue(b)) => Some(b.as_slice()),
                _ => None,
            })
            .collect::<Vec<_>>(),
    ));
    let ser: Vec<Option<Vec<u8>>> = values
        .iter()
        .map(|v| match v {
            Some(Value::KvlistValue(_) | Value::ArrayValue(_)) => {
                let mut out = Vec::new();
                ciborium::into_writer(&to_cbor(*v), &mut out).unwrap();
                Some(out)
            }
            _ => None,
        })
        .collect();
    let ser = Arc::new(BinaryArray::from(
        ser.iter().map(|v| v.as_deref()).collect::<Vec<_>>(),
    ));
    let arrays: Vec<ArrayRef> = vec![types, strs, ints, doubles, bools, bytes, ser];
    let fields = ["type", "str", "int", "double", "bool", "bytes", "ser"]
        .iter()
        .zip(&arrays)
        .map(|(name, a)| Field::new(*name, a.data_type().clone(), true))
        .collect();
    (fields, arrays)
}

/// An attribute table sorted by (key, value) with quasi-delta parent ids:
/// a delta from the previous row whenever key and value repeat.
fn attrs_table(mut rows: Vec<(u32, &KeyValue)>, wide: bool) -> RecordBatch {
    rows.sort_by(|a, b| {
        a.1.key
            .cmp(&b.1.key)
            .then_with(|| format!("{:?}", a.1.value).cmp(&format!("{:?}", b.1.value)))
            .then(a.0.cmp(&b.0))
    });
    let mut raw = Vec::with_capacity(rows.len());
    let mut prev: Option<(u32, &KeyValue)> = None;
    for &(parent, kv) in &rows {
        let same_run = prev.is_some_and(|(_, p)| p.key == kv.key && p.value == kv.value);
        raw.push(if same_run {
            parent.wrapping_sub(prev.unwrap().0)
        } else {
            parent
        });
        prev = Some((parent, kv));
    }
    let parent_id: ArrayRef = if wide {
        Arc::new(UInt32Array::from(raw))
    } else {
        Arc::new(UInt16Array::from(
            raw.iter()
                .map(|v| u16::try_from(*v & 0xffff).unwrap())
                .collect::<Vec<_>>(),
        ))
    };
    let values: Vec<Option<&any_value::Value>> = rows
        .iter()
        .map(|(_, kv)| kv.value.as_ref().and_then(|v| v.value.as_ref()))
        .collect();
    let (value_fields, value_arrays) = value_columns(&values);
    let mut cols = Columns::default();
    cols.push_field(
        Field::new("parent_id", parent_id.data_type().clone(), false),
        parent_id,
    );
    cols.push(
        "key",
        dict8(strings(rows.iter().map(|(_, kv)| Some(kv.key.as_str())))),
    );
    for (field, array) in value_fields.into_iter().zip(value_arrays) {
        cols.push_field(field, array);
    }
    cols.batch()
}

/// `SPAN_EVENTS` sorted by name with ids in sorted order and parent ids
/// delta-encoded inside each run of equal names.
fn events_table(events: &[(u16, &Event)]) -> RecordBatch {
    let mut raw_parent = Vec::with_capacity(events.len());
    let mut prev: Option<(u16, &str)> = None;
    for (parent, e) in events {
        let same_run = prev.is_some_and(|(_, name)| name == e.name);
        raw_parent.push(if same_run {
            parent.wrapping_sub(prev.unwrap().0)
        } else {
            *parent
        });
        prev = Some((*parent, e.name.as_str()));
    }
    let mut cols = Columns::default();
    cols.push_field(
        delta_field("id", arrow::datatypes::DataType::UInt32),
        Arc::new(UInt32Array::from(
            (0..events.len())
                .map(|i| u32::from(i > 0))
                .collect::<Vec<_>>(),
        )),
    );
    cols.push_field(
        Field::new("parent_id", arrow::datatypes::DataType::UInt16, false),
        Arc::new(UInt16Array::from(raw_parent)),
    );
    cols.push(
        "time_unix_nano",
        Arc::new(TimestampNanosecondArray::from(
            events
                .iter()
                .map(|(_, e)| e.time_unix_nano as i64)
                .collect::<Vec<_>>(),
        )),
    );
    cols.push(
        "name",
        strings(events.iter().map(|(_, e)| Some(e.name.as_str()))),
    );
    cols.push(
        "dropped_attributes_count",
        u32s(events.iter().map(|(_, e)| e.dropped_attributes_count)),
    );
    cols.batch()
}

/// Data point parent ids sorted and delta-encoded (plain delta, not keyed).
fn parent_deltas(parents: &[u16]) -> ArrayRef {
    u16_delta(parents.iter().copied())
}

fn point_ids(n: usize) -> (Field, ArrayRef) {
    (
        delta_field("id", arrow::datatypes::DataType::UInt32),
        Arc::new(UInt32Array::from(
            (0..n).map(|i| u32::from(i > 0)).collect::<Vec<_>>(),
        )),
    )
}

#[derive(Default)]
struct NumberPoints<'a> {
    rows: Vec<(
        u16,
        &'a opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    )>,
}

impl<'a> NumberPoints<'a> {
    fn extend(
        &mut self,
        metric: u16,
        points: &'a [opentelemetry_proto::tonic::metrics::v1::NumberDataPoint],
    ) {
        self.rows.extend(points.iter().map(|p| (metric, p)));
    }

    fn tables(&self) -> Vec<(ArrowPayloadType, RecordBatch)> {
        if self.rows.is_empty() {
            return Vec::new();
        }
        let mut cols = Columns::default();
        let (id_field, ids) = point_ids(self.rows.len());
        cols.push_field(id_field, ids);
        cols.push_field(
            Field::new("parent_id", arrow::datatypes::DataType::UInt16, false),
            parent_deltas(&self.rows.iter().map(|r| r.0).collect::<Vec<_>>()),
        );
        cols.push(
            "start_time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                self.rows
                    .iter()
                    .map(|r| r.1.start_time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                self.rows
                    .iter()
                    .map(|r| r.1.time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "int_value",
            Arc::new(Int64Array::from(
                self.rows
                    .iter()
                    .map(|r| match r.1.value {
                        Some(number_data_point::Value::AsInt(i)) => Some(i),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "double_value",
            Arc::new(Float64Array::from(
                self.rows
                    .iter()
                    .map(|r| match r.1.value {
                        Some(number_data_point::Value::AsDouble(d)) => Some(d),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push("flags", u32s(self.rows.iter().map(|r| r.1.flags)));
        let attrs: Vec<(u32, &KeyValue)> = self
            .rows
            .iter()
            .enumerate()
            .flat_map(|(i, (_, p))| {
                p.attributes
                    .iter()
                    .map(move |kv| (u32::try_from(i).unwrap(), kv))
            })
            .collect();
        let mut out = vec![(ArrowPayloadType::NumberDataPoints, cols.batch())];
        if !attrs.is_empty() {
            out.push((ArrowPayloadType::NumberDpAttrs, attrs_table(attrs, true)));
        }
        out
    }
}

#[derive(Default)]
struct HistogramPoints<'a> {
    rows: Vec<(
        u16,
        &'a opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint,
    )>,
}

impl<'a> HistogramPoints<'a> {
    fn extend(
        &mut self,
        metric: u16,
        points: &'a [opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint],
    ) {
        self.rows.extend(points.iter().map(|p| (metric, p)));
    }

    fn tables(&self) -> Vec<(ArrowPayloadType, RecordBatch)> {
        if self.rows.is_empty() {
            return Vec::new();
        }
        let mut cols = Columns::default();
        let (id_field, ids) = point_ids(self.rows.len());
        cols.push_field(id_field, ids);
        cols.push_field(
            Field::new("parent_id", arrow::datatypes::DataType::UInt16, false),
            parent_deltas(&self.rows.iter().map(|r| r.0).collect::<Vec<_>>()),
        );
        cols.push(
            "start_time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                self.rows
                    .iter()
                    .map(|r| r.1.start_time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "time_unix_nano",
            Arc::new(TimestampNanosecondArray::from(
                self.rows
                    .iter()
                    .map(|r| r.1.time_unix_nano as i64)
                    .collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "count",
            Arc::new(UInt64Array::from(
                self.rows.iter().map(|r| r.1.count).collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "sum",
            Arc::new(Float64Array::from(
                self.rows.iter().map(|r| r.1.sum).collect::<Vec<_>>(),
            )),
        );
        let mut buckets = ListBuilder::new(UInt64Builder::new());
        let mut bounds = ListBuilder::new(Float64Builder::new());
        for (_, p) in &self.rows {
            buckets.values().append_slice(&p.bucket_counts);
            buckets.append(true);
            bounds.values().append_slice(&p.explicit_bounds);
            bounds.append(true);
        }
        cols.push("bucket_counts", Arc::new(buckets.finish()));
        cols.push("explicit_bounds", Arc::new(bounds.finish()));
        cols.push("flags", u32s(self.rows.iter().map(|r| r.1.flags)));
        cols.push(
            "min",
            Arc::new(Float64Array::from(
                self.rows.iter().map(|r| r.1.min).collect::<Vec<_>>(),
            )),
        );
        cols.push(
            "max",
            Arc::new(Float64Array::from(
                self.rows.iter().map(|r| r.1.max).collect::<Vec<_>>(),
            )),
        );
        let attrs: Vec<(u32, &KeyValue)> = self
            .rows
            .iter()
            .enumerate()
            .flat_map(|(i, (_, p))| {
                p.attributes
                    .iter()
                    .map(move |kv| (u32::try_from(i).unwrap(), kv))
            })
            .collect();
        let mut out = vec![(ArrowPayloadType::HistogramDataPoints, cols.batch())];
        if !attrs.is_empty() {
            out.push((ArrowPayloadType::HistogramDpAttrs, attrs_table(attrs, true)));
        }
        out
    }
}

/// The minimal shape of a data point table whose points the lake rejects
/// (summaries, exponential histograms): ids and parent ids only.
fn parent_only_table(parents: &[u16]) -> RecordBatch {
    let mut cols = Columns::default();
    let (id_field, ids) = point_ids(parents.len());
    cols.push_field(id_field, ids);
    cols.push_field(
        Field::new("parent_id", arrow::datatypes::DataType::UInt16, false),
        parent_deltas(parents),
    );
    cols.push(
        "time_unix_nano",
        Arc::new(TimestampNanosecondArray::from(vec![0i64; parents.len()])),
    );
    cols.batch()
}
