//! `ExportTraceServiceRequest` → `Vec<SpanRow>`.

use tumult_lake::SpanRow;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use opentelemetry_proto::tonic::trace::v1::Span;

use crate::common::{self, keys, ResourceCtx};

const PROMOTED_SPAN_KEYS: &[&str] = &[
    keys::EXPERIMENT_ID,
    keys::EXPERIMENT_NAME,
    keys::EXPERIMENT_TITLE,
    keys::OUTCOME_STATUS,
    keys::OUTCOME_HYPOTHESIS_MET,
    keys::OUTCOME_RECOVERY_TIME_S,
    keys::FAULT_TYPE,
    keys::FAULT_SUBTYPE,
    keys::FAULT_SEVERITY,
    keys::FAULT_BLAST_RADIUS,
    keys::FAULT_PLUGIN,
    keys::PLUGIN_NAME,
    keys::TARGET_SYSTEM,
    keys::TARGET_TECHNOLOGY,
    keys::TARGET_ENVIRONMENT,
];

fn span_kind_str(kind: i32) -> String {
    match SpanKind::try_from(kind).unwrap_or(SpanKind::Unspecified) {
        SpanKind::Unspecified => "Unspecified",
        SpanKind::Internal => "Internal",
        SpanKind::Server => "Server",
        SpanKind::Client => "Client",
        SpanKind::Producer => "Producer",
        SpanKind::Consumer => "Consumer",
    }
    .to_string()
}

fn status_code_str(code: i32) -> String {
    match StatusCode::try_from(code).unwrap_or(StatusCode::Unset) {
        StatusCode::Unset => "Unset",
        StatusCode::Ok => "Ok",
        StatusCode::Error => "Error",
    }
    .to_string()
}

/// Serialise span events as a JSON array string.
fn events_json(span: &Span) -> String {
    if span.events.is_empty() {
        return "[]".to_string();
    }
    let events: Vec<serde_json::Value> = span
        .events
        .iter()
        .map(|e| {
            let attrs: serde_json::Map<String, serde_json::Value> = e
                .attributes
                .iter()
                .filter_map(|kv| {
                    kv.value.as_ref().map(|v| {
                        (
                            kv.key.clone(),
                            serde_json::Value::String(common::any_value_to_string(v)),
                        )
                    })
                })
                .collect();
            serde_json::json!({
                "time_unix_nano": e.time_unix_nano,
                "name": e.name,
                "attributes": serde_json::Value::Object(attrs),
            })
        })
        .collect();
    serde_json::Value::Array(events).to_string()
}

fn span_to_row(span: &Span, resource: &ResourceCtx) -> SpanRow {
    let attrs = &span.attributes;
    // Span attributes win over resource attributes for experiment identity
    // (the standard sets experiment.* on both the root span's resource and
    // the span itself; child spans may only have it via the resource).
    // tumult's CLI runner names the experiment via `resilience.experiment.title`.
    let experiment_id =
        common::attr_string(attrs, keys::EXPERIMENT_ID).or_else(|| resource.experiment_id.clone());
    let experiment_name = common::attr_string(attrs, keys::EXPERIMENT_NAME)
        .or_else(|| common::attr_string(attrs, keys::EXPERIMENT_TITLE))
        .or_else(|| resource.experiment_name.clone());

    let (status_code, status_message) = span.status.as_ref().map_or_else(
        || ("Unset".to_string(), String::new()),
        |s| (status_code_str(s.code), s.message.clone()),
    );

    SpanRow {
        ts_ns: i64::try_from(span.start_time_unix_nano).unwrap_or(i64::MAX),
        trace_id: common::hex_encode(&span.trace_id),
        span_id: common::hex_encode(&span.span_id),
        parent_span_id: common::hex_id(&span.parent_span_id),
        span_name: span.name.clone(),
        span_kind: span_kind_str(span.kind),
        duration_ns: i64::try_from(
            span.end_time_unix_nano
                .saturating_sub(span.start_time_unix_nano),
        )
        .unwrap_or(i64::MAX),
        status_code,
        status_message,
        service_name: resource.service_name.clone(),
        service_version: resource.service_version.clone(),
        experiment_id,
        experiment_name,
        outcome_status: common::attr_string(attrs, keys::OUTCOME_STATUS),
        fault_type: common::attr_string(attrs, keys::FAULT_TYPE),
        fault_subtype: common::attr_string(attrs, keys::FAULT_SUBTYPE),
        fault_severity: common::attr_string(attrs, keys::FAULT_SEVERITY),
        blast_radius: common::attr_string(attrs, keys::FAULT_BLAST_RADIUS),
        target_system: common::attr_string(attrs, keys::TARGET_SYSTEM),
        target_technology: common::attr_string(attrs, keys::TARGET_TECHNOLOGY),
        target_environment: common::attr_string(attrs, keys::TARGET_ENVIRONMENT),
        plugin_name: common::attr_string(attrs, keys::FAULT_PLUGIN)
            .or_else(|| common::attr_string(attrs, keys::PLUGIN_NAME)),
        hypothesis_met: common::attr_bool(attrs, keys::OUTCOME_HYPOTHESIS_MET),
        recovery_time_s: common::attr_double(attrs, keys::OUTCOME_RECOVERY_TIME_S),
        span_attrs: common::unpromoted_attrs(attrs, PROMOTED_SPAN_KEYS),
        resource_attrs: resource.resource_attrs.clone(),
        events: events_json(span),
    }
}

/// Convert an OTLP trace export request into store rows (pure).
#[must_use]
pub fn trace_request_to_spans(request: &ExportTraceServiceRequest) -> Vec<SpanRow> {
    let mut out = Vec::new();
    for resource_spans in &request.resource_spans {
        let resource = ResourceCtx::from_resource(resource_spans.resource.as_ref());
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                out.push(span_to_row(span, &resource));
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
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Status};

    fn kv(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            key_strindex: 0,
        }
    }

    fn experiment_request() -> ExportTraceServiceRequest {
        let span = Span {
            trace_id: vec![0xab; 16],
            span_id: vec![0xcd; 8],
            parent_span_id: vec![],
            name: "resilience.experiment".into(),
            kind: SpanKind::Internal as i32,
            start_time_unix_nano: 1_774_980_000_000_000_000,
            end_time_unix_nano: 1_774_980_300_000_000_000,
            attributes: vec![
                kv(keys::EXPERIMENT_ID, Value::StringValue("exp-1".into())),
                kv(
                    keys::EXPERIMENT_NAME,
                    Value::StringValue("pg-failover".into()),
                ),
                kv(keys::OUTCOME_STATUS, Value::StringValue("completed".into())),
                kv(keys::OUTCOME_HYPOTHESIS_MET, Value::BoolValue(true)),
                kv(keys::OUTCOME_RECOVERY_TIME_S, Value::DoubleValue(12.5)),
                kv(keys::FAULT_TYPE, Value::StringValue("termination".into())),
                kv(keys::TARGET_SYSTEM, Value::StringValue("database".into())),
                kv(
                    "resilience.estimate.rationale",
                    Value::StringValue("primary restart is fast".into()),
                ),
            ],
            events: vec![],
            status: Some(Status {
                code: StatusCode::Ok as i32,
                message: String::new(),
            }),
            ..Span::default()
        };
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![
                        kv("service.name", Value::StringValue("tumult".into())),
                        kv("service.version", Value::StringValue("2.18.0".into())),
                    ],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![span],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    #[test]
    fn experiment_span_promotes_resilience_attributes() {
        let rows = trace_request_to_spans(&experiment_request());
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.ts_ns, 1_774_980_000_000_000_000);
        assert_eq!(row.duration_ns, 300_000_000_000);
        assert_eq!(row.span_name, "resilience.experiment");
        assert_eq!(row.span_kind, "Internal");
        assert_eq!(row.status_code, "Ok");
        assert_eq!(row.service_name, "tumult");
        assert_eq!(row.service_version.as_deref(), Some("2.18.0"));
        assert_eq!(row.experiment_id.as_deref(), Some("exp-1"));
        assert_eq!(row.outcome_status.as_deref(), Some("completed"));
        assert_eq!(row.hypothesis_met, Some(true));
        assert_eq!(row.recovery_time_s, Some(12.5));
        assert_eq!(row.fault_type.as_deref(), Some("termination"));
        assert_eq!(row.target_system.as_deref(), Some("database"));
        // Promoted keys are excluded from the map; the rest stays.
        assert_eq!(
            row.span_attrs,
            vec![(
                "resilience.estimate.rationale".to_string(),
                "primary restart is fast".to_string()
            )]
        );
        // Ids are hex-encoded.
        assert_eq!(row.trace_id, "ab".repeat(16));
        assert_eq!(row.span_id, "cd".repeat(8));
        assert_eq!(row.parent_span_id, None);
    }

    #[test]
    fn resource_experiment_id_fills_child_spans() {
        let mut request = experiment_request();
        // Child span carries no experiment.* of its own.
        let resource_spans = &mut request.resource_spans[0];
        resource_spans
            .resource
            .as_mut()
            .unwrap()
            .attributes
            .push(kv(
                keys::EXPERIMENT_ID,
                Value::StringValue("exp-from-resource".into()),
            ));
        let child = &mut resource_spans.scope_spans[0].spans[0];
        child.attributes.retain(|kv| kv.key != keys::EXPERIMENT_ID);

        let rows = trace_request_to_spans(&request);
        assert_eq!(rows[0].experiment_id.as_deref(), Some("exp-from-resource"));
    }
}
