//! `SPANS` (+ `SPAN_ATTRS`, `SPAN_EVENTS`, `SPAN_EVENT_ATTRS`, resource and
//! scope attributes) → `ExportTraceServiceRequest`.
//!
//! Span links (`SPAN_LINKS`, `SPAN_LINK_ATTRS`) are not decoded: no lake
//! column reads them, and the OTLP path drops them in the same place.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::span::Event;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};

use super::arrays::Table;
use super::attrs::{attributes, AttrMap};
use super::common::{groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// Decode one OTAP traces batch into the OTLP request the gRPC/HTTP paths
/// receive.
///
/// # Errors
/// Any missing root table, malformed column or unknown value type.
pub fn to_trace_request(batch: &OtapBatch) -> Result<ExportTraceServiceRequest, OtapError> {
    let spans_rb = batch
        .get(ArrowPayloadType::Spans)
        .ok_or(OtapError::MissingTable("SPANS"))?;
    let root = Root::new("SPANS", spans_rb, batch)?;
    let status = root.table.struct_table("status")?;
    let span_attrs = attributes(batch.get(ArrowPayloadType::SpanAttrs), "SPAN_ATTRS")?;
    let mut events = span_events(batch)?;

    let mut out = ExportTraceServiceRequest::default();
    for (i, new_resource, new_scope) in groups(&root) {
        if new_resource {
            out.resource_spans.push(ResourceSpans {
                resource: root.resource(i)?,
                scope_spans: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let Some(rs) = out.resource_spans.last_mut() else {
            continue;
        };
        if new_scope {
            rs.scope_spans.push(ScopeSpans {
                scope: root.scope(i)?,
                spans: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let span = span_at(&root, status.as_ref(), i, &span_attrs, &mut events)?;
        if let Some(ss) = rs.scope_spans.last_mut() {
            ss.spans.push(span);
        }
    }
    Ok(out)
}

fn span_at(
    root: &Root,
    status: Option<&Table>,
    i: usize,
    span_attrs: &AttrMap,
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
        trace_id: t.vec_at("trace_id", i)?,
        span_id: t.vec_at("span_id", i)?,
        trace_state: t.string_at("trace_state", i)?,
        parent_span_id: t.vec_at("parent_span_id", i)?,
        flags: t.u32_at("flags", i)?.unwrap_or(0),
        name: t.string_at("name", i)?,
        kind: t.i32_at("kind", i)?.unwrap_or(0),
        start_time_unix_nano: start,
        end_time_unix_nano: start.saturating_add(t.duration_at("duration_time_unix_nano", i)?),
        attributes: id
            .and_then(|id| span_attrs.get(&id).cloned())
            .unwrap_or_default(),
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
fn span_events(batch: &OtapBatch) -> Result<HashMap<u32, Vec<Event>>, OtapError> {
    let mut out: HashMap<u32, Vec<Event>> = HashMap::new();
    let Some(rb) = batch.get(ArrowPayloadType::SpanEvents) else {
        return Ok(out);
    };
    let t = Table::new("SPAN_EVENTS", rb)?;
    let event_ids = t.ids("id")?;
    let parents = t.id_column("parent_id")?.ok_or_else(|| OtapError::Column {
        table: "SPAN_EVENTS",
        column: "parent_id".into(),
        reason: "column is required".into(),
    })?;
    let event_attrs = attributes(
        batch.get(ArrowPayloadType::SpanEventAttrs),
        "SPAN_EVENT_ATTRS",
    )?;
    let mut previous: Option<(String, u32)> = None;
    for (i, (raw, event_id)) in parents.values.iter().zip(&event_ids).enumerate() {
        let Some(raw) = *raw else {
            continue;
        };
        let name = t.string_at("name", i)?;
        let parent = match &previous {
            Some((prev_name, prev_parent)) if parents.encoded && *prev_name == name => {
                prev_parent.wrapping_add(raw)
            }
            _ => raw,
        };
        previous = Some((name.clone(), parent));
        out.entry(parent).or_default().push(Event {
            time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
            name,
            attributes: event_id
                .and_then(|id| event_attrs.get(&id).cloned())
                .unwrap_or_default(),
            dropped_attributes_count: t.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        });
    }
    for list in out.values_mut() {
        list.sort_by_key(|e| e.time_unix_nano);
    }
    Ok(out)
}
