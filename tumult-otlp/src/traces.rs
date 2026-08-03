//! `ExportTraceServiceRequest` → `Vec<SpanRow>`.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use opentelemetry_proto::tonic::trace::v1::Span;
use tumult_lake::SpanRow;

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

    fn bare_span() -> Span {
        Span {
            trace_id: vec![0x01; 16],
            span_id: vec![0x02; 8],
            start_time_unix_nano: 10,
            end_time_unix_nano: 25,
            ..Span::default()
        }
    }

    fn request_for(spans: Vec<Span>, resource_attrs: Vec<KeyValue>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: resource_attrs,
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    #[test]
    fn span_kind_and_status_strings_cover_all_variants() {
        assert_eq!(span_kind_str(SpanKind::Unspecified as i32), "Unspecified");
        assert_eq!(span_kind_str(SpanKind::Internal as i32), "Internal");
        assert_eq!(span_kind_str(SpanKind::Server as i32), "Server");
        assert_eq!(span_kind_str(SpanKind::Client as i32), "Client");
        assert_eq!(span_kind_str(SpanKind::Producer as i32), "Producer");
        assert_eq!(span_kind_str(SpanKind::Consumer as i32), "Consumer");
        // Unknown discriminants fall back to the unspecified label.
        assert_eq!(span_kind_str(99), "Unspecified");

        assert_eq!(status_code_str(StatusCode::Unset as i32), "Unset");
        assert_eq!(status_code_str(StatusCode::Ok as i32), "Ok");
        assert_eq!(status_code_str(StatusCode::Error as i32), "Error");
        assert_eq!(status_code_str(77), "Unset");
    }

    #[test]
    fn span_identity_falls_back_through_title_and_resource() {
        let mut aliased = bare_span();
        aliased.attributes = vec![
            kv(
                keys::EXPERIMENT_TITLE,
                Value::StringValue("cli-title".into()),
            ),
            kv(keys::PLUGIN_NAME, Value::StringValue("alias-plugin".into())),
            kv(keys::FAULT_SUBTYPE, Value::StringValue("signal".into())),
            kv(keys::FAULT_SEVERITY, Value::StringValue("high".into())),
            kv(
                keys::FAULT_BLAST_RADIUS,
                Value::StringValue("single-pod".into()),
            ),
            kv(
                keys::TARGET_TECHNOLOGY,
                Value::StringValue("postgres".into()),
            ),
            kv(
                keys::TARGET_ENVIRONMENT,
                Value::StringValue("staging".into()),
            ),
        ];

        // The standard's plugin name wins over the instrument alias.
        let mut standard = bare_span();
        standard.attributes = vec![
            kv(keys::FAULT_PLUGIN, Value::StringValue("std-plugin".into())),
            kv(keys::PLUGIN_NAME, Value::StringValue("alias-plugin".into())),
        ];

        // No identity attributes at all: everything comes from the resource.
        let child = bare_span();

        let request = request_for(
            vec![aliased, standard, child],
            vec![
                kv(keys::EXPERIMENT_ID, Value::StringValue("res-exp".into())),
                kv(keys::EXPERIMENT_NAME, Value::StringValue("res-name".into())),
            ],
        );
        let rows = trace_request_to_spans(&request);
        assert_eq!(rows.len(), 3);

        let row = &rows[0];
        assert_eq!(row.experiment_id.as_deref(), Some("res-exp"));
        assert_eq!(row.experiment_name.as_deref(), Some("cli-title"));
        assert_eq!(row.plugin_name.as_deref(), Some("alias-plugin"));
        assert_eq!(row.fault_subtype.as_deref(), Some("signal"));
        assert_eq!(row.fault_severity.as_deref(), Some("high"));
        assert_eq!(row.blast_radius.as_deref(), Some("single-pod"));
        assert_eq!(row.target_technology.as_deref(), Some("postgres"));
        assert_eq!(row.target_environment.as_deref(), Some("staging"));
        // Every attribute was promoted, so the map stays empty.
        assert!(row.span_attrs.is_empty());
        // No status set on the span.
        assert_eq!(row.status_code, "Unset");
        assert!(row.status_message.is_empty());

        assert_eq!(rows[1].plugin_name.as_deref(), Some("std-plugin"));
        assert_eq!(rows[2].experiment_name.as_deref(), Some("res-name"));
    }

    #[test]
    fn span_events_serialize_to_a_json_array() {
        use opentelemetry_proto::tonic::trace::v1::span::Event;

        let mut span = bare_span();
        span.status = Some(Status {
            code: StatusCode::Error as i32,
            message: "boom".into(),
        });
        span.events = vec![Event {
            time_unix_nano: 11,
            name: "resilience.hypothesis.evaluated".into(),
            attributes: vec![
                kv(keys::OUTCOME_STATUS, Value::StringValue("met".into())),
                KeyValue {
                    key: "skip".into(),
                    value: None,
                    key_strindex: 0,
                },
            ],
            ..Event::default()
        }];

        let rows = trace_request_to_spans(&request_for(vec![span], vec![]));
        let row = &rows[0];
        assert_eq!(row.status_code, "Error");
        assert_eq!(row.status_message, "boom");

        let events: serde_json::Value = serde_json::from_str(&row.events).unwrap();
        assert_eq!(events[0]["name"], "resilience.hypothesis.evaluated");
        assert_eq!(events[0]["time_unix_nano"], 11);
        assert_eq!(events[0]["attributes"]["resilience.outcome.status"], "met");
        // Valueless event attributes are dropped from the JSON object.
        assert!(events[0]["attributes"].get("skip").is_none());
    }

    #[test]
    fn timestamps_saturate_and_duration_clamps_at_zero() {
        let mut span = bare_span();
        span.start_time_unix_nano = u64::MAX;
        // End before start: the duration must clamp to zero, not wrap.
        span.end_time_unix_nano = 5;
        span.parent_span_id = vec![0x03; 8];

        let rows = trace_request_to_spans(&request_for(vec![span], vec![]));
        let row = &rows[0];
        assert_eq!(row.ts_ns, i64::MAX);
        assert_eq!(row.duration_ns, 0);
        assert_eq!(row.parent_span_id.as_deref(), Some("0303030303030303"));
    }
}
