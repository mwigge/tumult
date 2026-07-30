//! `ExportLogsServiceRequest` → `Vec<LogRow>`.

use kronika_store::LogRow;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::logs::v1::LogRecord;

use crate::common::{self, ResourceCtx};

/// Timestamp for one log record: `time_unix_nano` (event time) →
/// `observed_time_unix_nano` (SDK observation time) → `received_at_ns`
/// (when kronika received the export). tumult's log records arrive with
/// `time_unix_nano` unset (0), so without this chain every log would be
/// stamped epoch 0 (1970) in the store.
fn log_ts(record: &LogRecord, received_at_ns: i64) -> i64 {
    if record.time_unix_nano != 0 {
        i64::try_from(record.time_unix_nano).unwrap_or(i64::MAX)
    } else if record.observed_time_unix_nano != 0 {
        i64::try_from(record.observed_time_unix_nano).unwrap_or(i64::MAX)
    } else {
        received_at_ns
    }
}

/// Convert an OTLP logs export request into store rows (pure).
/// `received_at_ns` is the wall-clock time the export reached kronika, used
/// only for records that carry no timestamp of their own (see [`log_ts`]).
#[must_use]
pub fn logs_request_to_rows(
    request: &ExportLogsServiceRequest,
    received_at_ns: i64,
) -> Vec<LogRow> {
    let mut out = Vec::new();
    for resource_logs in &request.resource_logs {
        let resource = ResourceCtx::from_resource(resource_logs.resource.as_ref());
        for scope_logs in &resource_logs.scope_logs {
            for record in &scope_logs.log_records {
                out.push(LogRow {
                    ts_ns: log_ts(record, received_at_ns),
                    severity_text: record.severity_text.clone(),
                    body: record
                        .body
                        .as_ref()
                        .map_or_else(String::new, common::any_value_to_string),
                    trace_id: common::hex_id(&record.trace_id),
                    span_id: common::hex_id(&record.span_id),
                    service_name: resource.service_name.clone(),
                    log_attrs: common::unpromoted_attrs(&record.attributes, &[]),
                    resource_attrs: resource.resource_attrs.clone(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;

    #[test]
    fn log_records_convert() {
        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".into(),
                        value: Some(AnyValue {
                            value: Some(Value::StringValue("smdjad".into())),
                        }),
                        key_strindex: 0,
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_774_980_000_000_000_000,
                        severity_text: "INFO".into(),
                        body: Some(AnyValue {
                            value: Some(Value::StringValue("experiment started".into())),
                        }),
                        trace_id: vec![0x01; 16],
                        span_id: vec![0x02; 8],
                        attributes: vec![KeyValue {
                            key: "resilience.experiment.id".into(),
                            value: Some(AnyValue {
                                value: Some(Value::StringValue("exp-1".into())),
                            }),
                            key_strindex: 0,
                        }],
                        ..LogRecord::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let rows = logs_request_to_rows(&request, 42);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.ts_ns, 1_774_980_000_000_000_000);
        assert_eq!(row.severity_text, "INFO");
        assert_eq!(row.body, "experiment started");
        assert_eq!(row.service_name, "smdjad");
        assert_eq!(row.trace_id.as_deref(), Some("01".repeat(16).as_str()));
        // Log attributes are kept verbatim (no promotion on the logs table).
        assert_eq!(row.log_attrs.len(), 1);
    }

    #[test]
    fn timestamp_falls_back_to_observed_then_received() {
        let record = |time: u64, observed: u64| LogRecord {
            time_unix_nano: time,
            observed_time_unix_nano: observed,
            ..LogRecord::default()
        };
        // Real event time wins.
        assert_eq!(
            log_ts(&record(1_785_268_000_000_000_000, 9), 42),
            1_785_268_000_000_000_000
        );
        // tumult's case: no event time → SDK observation time.
        assert_eq!(
            log_ts(&record(0, 1_785_268_000_000_000_001), 42),
            1_785_268_000_000_000_001
        );
        // Neither set → when kronika received the export.
        assert_eq!(
            log_ts(&record(0, 0), 1_785_268_000_000_000_002),
            1_785_268_000_000_000_002
        );
    }
}
