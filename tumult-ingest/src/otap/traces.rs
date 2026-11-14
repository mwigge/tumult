//! `SPANS` (+ `SPAN_ATTRS`, `SPAN_EVENTS`, `SPAN_EVENT_ATTRS`, resource and
//! scope attributes) → `ExportTraceServiceRequest`.
//!
//! Span links (`SPAN_LINKS`, `SPAN_LINK_ATTRS`) are not decoded: no lake
//! column reads them, and the OTLP path drops them in the same place.
//! Attribute and event rows whose parent row is not in the batch fail the
//! batch.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::span::Event;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};

use super::arrays::{orphans, Table};
use super::attrs::Attrs;
use super::common::{Groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

const SPAN_EVENTS: &str = "SPAN_EVENTS";

/// Columns the exporter always writes; a table without them is not spans.
const REQUIRED: [&str; 5] = [
    "start_time_unix_nano",
    "duration_time_unix_nano",
    "trace_id",
    "span_id",
    "name",
];

const TRACE_ID_BYTES: usize = 16;
const SPAN_ID_BYTES: usize = 8;

/// The tables of a traces batch. Link tables are accepted and not stored.
const TABLES: [ArrowPayloadType; 8] = [
    ArrowPayloadType::ResourceAttrs,
    ArrowPayloadType::ScopeAttrs,
    ArrowPayloadType::Spans,
    ArrowPayloadType::SpanAttrs,
    ArrowPayloadType::SpanEvents,
    ArrowPayloadType::SpanLinks,
    ArrowPayloadType::SpanEventAttrs,
    ArrowPayloadType::SpanLinkAttrs,
];

/// Decode one OTAP traces batch into the OTLP request the gRPC/HTTP paths
/// receive.
///
/// # Errors
/// Any missing root table, malformed column, unknown value type, duplicate
/// id, child row without a parent row, or exceeded limit.
pub fn to_trace_request(batch: &OtapBatch) -> Result<ExportTraceServiceRequest, OtapError> {
    batch.only("traces", &TABLES)?;
    let spans_rb = batch
        .get(ArrowPayloadType::Spans)
        .ok_or(OtapError::MissingTable("SPANS"))?;
    let mut root = Root::new("SPANS", spans_rb, batch, &REQUIRED)?;
    let status = root.table.struct_table("status")?;
    let mut span_attrs = Attrs::decode(batch, ArrowPayloadType::SpanAttrs, "SPAN_ATTRS")?;
    let mut events = span_events(batch)?;

    let mut out = ExportTraceServiceRequest::default();
    let mut groups = Groups::default();
    for i in 0..root.len() {
        root.table.tick()?;
        let (r, new_resource) = groups.resource(root.resource_id(i), out.resource_spans.len());
        if new_resource {
            out.resource_spans.push(ResourceSpans {
                resource: root.resource(i)?,
                scope_spans: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let rs = &mut out.resource_spans[r];
        let (s, new_scope) = groups.scope(r, root.scope_id(i), rs.scope_spans.len());
        if new_scope {
            rs.scope_spans.push(ScopeSpans {
                scope: root.scope(i)?,
                spans: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let span = span_at(&root, status.as_ref(), i, &mut span_attrs, &mut events)?;
        rs.scope_spans[s].spans.push(span);
    }
    let left: usize = events.values().map(Vec::len).sum();
    if left > 0 {
        return Err(orphans(SPAN_EVENTS, left));
    }
    span_attrs.finish()?;
    root.finish()?;
    Ok(out)
}

fn span_at(
    root: &Root,
    status: Option<&Table>,
    i: usize,
    span_attrs: &mut Attrs,
    events: &mut HashMap<u32, Vec<Event>>,
) -> Result<Span, OtapError> {
    let t = &root.table;
    let id = root.ids[i];
    let start = t.timestamp_at("start_time_unix_nano", i)?;
    let status = match status {
        Some(s) if s.is_valid(i) => Some(Status {
            code: s.i32_at("code", i)?.unwrap_or(0),
            message: s.string_at("status_message", i)?,
        }),
        _ => None,
    };
    Ok(Span {
        trace_id: t.id_at("trace_id", i, TRACE_ID_BYTES)?,
        span_id: t.id_at("span_id", i, SPAN_ID_BYTES)?,
        trace_state: t.string_at("trace_state", i)?,
        parent_span_id: t.optional_id_at("parent_span_id", i, SPAN_ID_BYTES)?,
        flags: t.u32_at("flags", i)?.unwrap_or(0),
        name: t.string_at("name", i)?,
        kind: t.i32_at("kind", i)?.unwrap_or(0),
        start_time_unix_nano: start,
        end_time_unix_nano: start.saturating_add(t.duration_at("duration_time_unix_nano", i)?),
        attributes: span_attrs.take(id),
        dropped_attributes_count: t.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        events: id.and_then(|id| events.remove(&id)).unwrap_or_default(),
        dropped_events_count: t.u32_at("dropped_events_count", i)?.unwrap_or(0),
        dropped_links_count: t.u32_at("dropped_links_count", i)?.unwrap_or(0),
        status,
        ..Span::default()
    })
}

/// `SPAN_EVENTS` rows keyed by span id. The carrier sorts events by name,
/// so each span's list is restored to time order (what the SDK emitted).
/// Whenever the name repeats the previous row's, `parent_id` is a delta from
/// the previous parent.
fn span_events(batch: &OtapBatch) -> Result<HashMap<u32, Vec<Event>>, OtapError> {
    let mut out: HashMap<u32, Vec<Event>> = HashMap::new();
    let mut event_attrs =
        Attrs::decode(batch, ArrowPayloadType::SpanEventAttrs, "SPAN_EVENT_ATTRS")?;
    let Some(rb) = batch.get(ArrowPayloadType::SpanEvents) else {
        event_attrs.finish()?;
        return Ok(out);
    };
    let t = Table::new(SPAN_EVENTS, rb, batch.budget())?;
    let event_ids = t.unique_ids("id")?;
    let parents = t.parents()?;
    let missing = parents.values.iter().filter(|p| p.is_none()).count();
    if missing > 0 {
        return Err(t.orphans(missing));
    }
    let mut previous: Option<(String, u32)> = None;
    for (i, (raw, event_id)) in parents.values.iter().flatten().zip(&event_ids).enumerate() {
        t.tick()?;
        let name = t.string_at("name", i)?;
        let parent = match &previous {
            Some((prev_name, prev_parent)) if parents.encoded && *prev_name == name => {
                parents.after(*prev_parent, *raw)
            }
            _ => *raw,
        };
        previous = Some((name.clone(), parent));
        out.entry(parent).or_default().push(Event {
            time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
            name,
            attributes: event_attrs.take(*event_id),
            dropped_attributes_count: t.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        });
    }
    event_attrs.finish()?;
    for list in out.values_mut() {
        list.sort_by_key(|e| e.time_unix_nano);
    }
    Ok(out)
}
