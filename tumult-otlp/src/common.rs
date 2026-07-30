//! Shared OTLP conversion helpers: attribute flattening, id hex-encoding and
//! `resilience.*` attribute promotion (tumult metadata standard v2.0).

use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::resource::v1::Resource;

/// The promoted, materialized `resilience.*` span attributes. Everything else
/// lands in the attribute maps.
pub mod keys {
    pub const EXPERIMENT_ID: &str = "resilience.experiment.id";
    pub const EXPERIMENT_NAME: &str = "resilience.experiment.name";
    /// What tumult's CLI runner actually sets on the experiment root span
    /// (the standard's `experiment.name` is emitted by the MCP layer only).
    pub const EXPERIMENT_TITLE: &str = "resilience.experiment.title";
    pub const OUTCOME_STATUS: &str = "resilience.outcome.status";
    pub const OUTCOME_HYPOTHESIS_MET: &str = "resilience.outcome.hypothesis_met";
    pub const OUTCOME_RECOVERY_TIME_S: &str = "resilience.outcome.recovery_time_s";
    pub const FAULT_TYPE: &str = "resilience.fault.type";
    pub const FAULT_SUBTYPE: &str = "resilience.fault.subtype";
    pub const FAULT_SEVERITY: &str = "resilience.fault.severity";
    pub const FAULT_BLAST_RADIUS: &str = "resilience.fault.blast_radius";
    pub const FAULT_PLUGIN: &str = "resilience.fault.plugin";
    /// The plugin dimension tumult's instruments actually use
    /// (`tumult.actions.total` etc.); `fault.plugin` is the standard's name.
    pub const PLUGIN_NAME: &str = "resilience.plugin.name";
    pub const TARGET_SYSTEM: &str = "resilience.target.system";
    pub const TARGET_TECHNOLOGY: &str = "resilience.target.technology";
    pub const TARGET_ENVIRONMENT: &str = "resilience.target.environment";
    pub const SERVICE_NAME: &str = "service.name";
    pub const SERVICE_VERSION: &str = "service.version";
}

/// Render an OTLP `AnyValue` as a plain string (complex values become JSON).
pub fn any_value_to_string(value: &AnyValue) -> String {
    match &value.value {
        Some(any_value::Value::StringValue(s)) => s.clone(),
        Some(any_value::Value::BoolValue(b)) => b.to_string(),
        Some(any_value::Value::IntValue(i)) => i.to_string(),
        Some(any_value::Value::DoubleValue(d)) => d.to_string(),
        Some(any_value::Value::BytesValue(b)) => hex_encode(b),
        Some(any_value::Value::ArrayValue(a)) => serde_json::Value::Array(
            a.values
                .iter()
                .map(|v| serde_json::Value::String(any_value_to_string(v)))
                .collect(),
        )
        .to_string(),
        Some(any_value::Value::KvlistValue(kv)) => {
            let map: serde_json::Map<String, serde_json::Value> = kv
                .values
                .iter()
                .filter_map(|kv| {
                    kv.value.as_ref().map(|v| {
                        (
                            kv.key.clone(),
                            serde_json::Value::String(any_value_to_string(v)),
                        )
                    })
                })
                .collect();
            serde_json::Value::Object(map).to_string()
        }
        // String-table interned values (OTLP string interning) cannot be
        // resolved without the message's string table; treat as absent.
        // TODO(otel): resolve StringValueStrindex if tumult/smedja ever
        // enable string interning on the wire.
        Some(any_value::Value::StringValueStrindex(_)) => String::new(),
        None => String::new(),
    }
}

/// Hex-encode an OTLP trace/span id (`Vec<u8>` → lowercase hex).
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Hex-encode an id, mapping the empty (absent) id to `None`.
#[must_use]
pub fn hex_id(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(hex_encode(bytes))
    }
}

/// Find a string attribute by key.
pub fn attr_str<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a AnyValue> {
    attrs
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.as_ref())
}

/// Find a string attribute by key and render it as a string.
pub fn attr_string(attrs: &[KeyValue], key: &str) -> Option<String> {
    attr_str(attrs, key).map(any_value_to_string)
}

/// Find a boolean attribute by key.
pub fn attr_bool(attrs: &[KeyValue], key: &str) -> Option<bool> {
    match attr_str(attrs, key)?.value {
        Some(any_value::Value::BoolValue(b)) => Some(b),
        _ => None,
    }
}

/// Find a double attribute by key (ints are widened).
pub fn attr_double(attrs: &[KeyValue], key: &str) -> Option<f64> {
    match attr_str(attrs, key)?.value {
        Some(any_value::Value::DoubleValue(d)) => Some(d),
        Some(any_value::Value::IntValue(i)) => Some(i as f64),
        _ => None,
    }
}

/// Flatten the attributes that were NOT promoted into `(key, value)` pairs.
pub fn unpromoted_attrs(attrs: &[KeyValue], promoted: &[&str]) -> Vec<(String, String)> {
    attrs
        .iter()
        .filter(|kv| !promoted.contains(&kv.key.as_str()))
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .map(|v| (kv.key.clone(), any_value_to_string(v)))
        })
        .collect()
}

/// What we need from a resource: service identity plus (per the tumult
/// standard, §OTel Integration Rules) `resilience.experiment.*`, which is set
/// as a *resource* attribute on the experiment root span.
pub struct ResourceCtx {
    pub service_name: String,
    pub service_version: Option<String>,
    pub experiment_id: Option<String>,
    pub experiment_name: Option<String>,
    pub resource_attrs: Vec<(String, String)>,
}

impl ResourceCtx {
    /// Extract the resource context from an OTLP resource.
    #[must_use]
    pub fn from_resource(resource: Option<&Resource>) -> Self {
        let attrs: &[KeyValue] = resource.map_or(&[], |r| r.attributes.as_slice());
        Self {
            service_name: attr_string(attrs, keys::SERVICE_NAME).unwrap_or_default(),
            service_version: attr_string(attrs, keys::SERVICE_VERSION),
            experiment_id: attr_string(attrs, keys::EXPERIMENT_ID),
            experiment_name: attr_string(attrs, keys::EXPERIMENT_NAME)
                .or_else(|| attr_string(attrs, keys::EXPERIMENT_TITLE)),
            resource_attrs: unpromoted_attrs(
                attrs,
                &[
                    keys::SERVICE_NAME,
                    keys::SERVICE_VERSION,
                    keys::EXPERIMENT_ID,
                    keys::EXPERIMENT_NAME,
                    keys::EXPERIMENT_TITLE,
                ],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;

    fn kv(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            key_strindex: 0,
        }
    }

    #[test]
    fn any_value_variants_render() {
        let attrs = vec![
            kv("s", Value::StringValue("x".into())),
            kv("b", Value::BoolValue(true)),
            kv("i", Value::IntValue(42)),
            kv("d", Value::DoubleValue(1.5)),
        ];
        assert_eq!(attr_string(&attrs, "s").as_deref(), Some("x"));
        assert_eq!(attr_bool(&attrs, "b"), Some(true));
        assert_eq!(attr_string(&attrs, "i").as_deref(), Some("42"));
        assert_eq!(attr_double(&attrs, "d"), Some(1.5));
    }

    #[test]
    fn hex_ids() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_id(&[]), None);
        assert_eq!(hex_id(&[0x00]).as_deref(), Some("00"));
    }

    #[test]
    fn resource_ctx_promotes_and_excludes() {
        let resource = Resource {
            attributes: vec![
                kv("service.name", Value::StringValue("tumult".into())),
                kv(
                    "resilience.experiment.id",
                    Value::StringValue("exp-9".into()),
                ),
                kv("k8s.cluster", Value::StringValue("prod-eu-01".into())),
            ],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        };
        let ctx = ResourceCtx::from_resource(Some(&resource));
        assert_eq!(ctx.service_name, "tumult");
        assert_eq!(ctx.experiment_id.as_deref(), Some("exp-9"));
        assert_eq!(
            ctx.resource_attrs,
            vec![("k8s.cluster".to_string(), "prod-eu-01".to_string())]
        );
    }
}
