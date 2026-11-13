//! `LOGS` (+ `LOG_ATTRS`, resource and scope attributes) →
//! `ExportLogsServiceRequest`.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

use super::arrays::Table;
use super::attrs::{any_value_at, attributes, AttrMap};
use super::common::{groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// Decode one OTAP logs batch into the OTLP request the gRPC/HTTP paths
/// receive.
///
/// # Errors
/// Any missing root table, malformed column or unknown value type.
pub fn to_logs_request(batch: &OtapBatch) -> Result<ExportLogsServiceRequest, OtapError> {
    let logs_rb = batch
        .get(ArrowPayloadType::Logs)
        .ok_or(OtapError::MissingTable("LOGS"))?;
    let root = Root::new("LOGS", logs_rb, batch)?;
    let body = root.table.struct_table("body")?;
    let log_attrs = attributes(batch.get(ArrowPayloadType::LogAttrs), "LOG_ATTRS")?;

    let mut out = ExportLogsServiceRequest::default();
    for (i, new_resource, new_scope) in groups(&root) {
        if new_resource {
            out.resource_logs.push(ResourceLogs {
                resource: root.resource(i)?,
                scope_logs: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let Some(rl) = out.resource_logs.last_mut() else {
            continue;
        };
        if new_scope {
            rl.scope_logs.push(ScopeLogs {
                scope: root.scope(i)?,
                log_records: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let record = record_at(&root, body.as_ref(), i, &log_attrs)?;
        if let Some(sl) = rl.scope_logs.last_mut() {
            sl.log_records.push(record);
        }
    }
    Ok(out)
}

fn record_at(
    root: &Root,
    body: Option<&Table>,
    i: usize,
    log_attrs: &AttrMap,
) -> Result<LogRecord, OtapError> {
    let t = &root.table;
    let body = match body {
        Some(b) if b.is_valid(i) => any_value_at(b, i)?.map(|v| AnyValue { value: Some(v) }),
        _ => None,
    };
    Ok(LogRecord {
        time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
        observed_time_unix_nano: t.timestamp_at("observed_time_unix_nano", i)?,
        severity_number: t.i32_at("severity_number", i)?.unwrap_or(0),
        severity_text: t.string_at("severity_text", i)?,
        body,
        attributes: root.ids[i]
            .and_then(|id| log_attrs.get(&id).cloned())
            .unwrap_or_default(),
        dropped_attributes_count: t.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        flags: t.u32_at("flags", i)?.unwrap_or(0),
        trace_id: t.vec_at("trace_id", i)?,
        span_id: t.vec_at("span_id", i)?,
        event_name: t.string_at("event_name", i)?,
    })
}
