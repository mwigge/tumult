//! `LOGS` (+ `LOG_ATTRS`, resource and scope attributes) →
//! `ExportLogsServiceRequest`.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

use super::arrays::Table;
use super::attrs::{any_value_at, Attrs};
use super::common::{Groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// Decode one OTAP logs batch into the OTLP request the gRPC/HTTP paths
/// receive.
///
/// # Errors
/// Any missing root table, malformed column, unknown value type, duplicate
/// id, attribute row without a parent row, or exceeded limit.
pub fn to_logs_request(batch: &OtapBatch) -> Result<ExportLogsServiceRequest, OtapError> {
    let logs_rb = batch
        .get(ArrowPayloadType::Logs)
        .ok_or(OtapError::MissingTable("LOGS"))?;
    let mut root = Root::new("LOGS", logs_rb, batch)?;
    let body = root.table.struct_table("body")?;
    let mut log_attrs = Attrs::decode(batch, ArrowPayloadType::LogAttrs, "LOG_ATTRS")?;

    let mut out = ExportLogsServiceRequest::default();
    let mut groups = Groups::default();
    for i in 0..root.len() {
        let (r, new_resource) = groups.resource(root.resource_id(i), out.resource_logs.len());
        if new_resource {
            out.resource_logs.push(ResourceLogs {
                resource: root.resource(i)?,
                scope_logs: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let rl = &mut out.resource_logs[r];
        let (s, new_scope) = groups.scope(r, root.scope_id(i), rl.scope_logs.len());
        if new_scope {
            rl.scope_logs.push(ScopeLogs {
                scope: root.scope(i)?,
                log_records: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let record = record_at(&root, body.as_ref(), i, &mut log_attrs)?;
        rl.scope_logs[s].log_records.push(record);
    }
    log_attrs.finish()?;
    root.finish()?;
    Ok(out)
}

fn record_at(
    root: &Root,
    body: Option<&Table>,
    i: usize,
    log_attrs: &mut Attrs,
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
        attributes: log_attrs.take(root.ids[i]),
        dropped_attributes_count: t.u32_at("dropped_attributes_count", i)?.unwrap_or(0),
        flags: t.u32_at("flags", i)?.unwrap_or(0),
        trace_id: t.vec_at("trace_id", i)?,
        span_id: t.vec_at("span_id", i)?,
        event_name: t.string_at("event_name", i)?,
    })
}
