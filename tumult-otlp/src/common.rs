//! Shared OTLP conversion helpers: attribute flattening, id hex-encoding and
//! `resilience.*` attribute promotion (tumult metadata standard v2.0).

use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::resource::v1::Resource;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tumult_core::compliance::{
    entity_id_is_valid, truncated, ENTITY_ID_ATTR, ENTITY_ID_MALFORMED_ATTR, ENTITY_ID_MAX_LEN,
    ENTITY_ID_TRUNCATION_MARK,
};

/// Seconds between two warnings about a malformed `ict.entity.id`. A
/// sender can repeat a bad id on every resource, so the log gets one
/// event per interval with the number it left out; the counter has all.
pub const MALFORMED_WARN_INTERVAL_S: u64 = 60;

static MALFORMED_ENTITY_IDS: AtomicU64 = AtomicU64::new(0);
static MALFORMED_WARN: WarnLimit = WarnLimit::new();

/// Resources received since start whose `ict.entity.id` was malformed.
#[must_use]
pub fn malformed_entity_ids_total() -> u64 {
    MALFORMED_ENTITY_IDS.load(Ordering::Relaxed)
}

/// Lets one event through per interval and counts the ones it holds back.
struct WarnLimit {
    next_s: AtomicU64,
    suppressed: AtomicU64,
}

impl WarnLimit {
    const fn new() -> Self {
        Self {
            next_s: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
        }
    }

    /// `Some(n)` when an event may be written at `now_s`, with `n` the
    /// number held back since the last one; `None` when this one is
    /// held back too.
    fn allow(&self, now_s: u64, every_s: u64) -> Option<u64> {
        let next = self.next_s.load(Ordering::Relaxed);
        let won = now_s >= next
            && self
                .next_s
                .compare_exchange(next, now_s + every_s, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok();
        if !won {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(self.suppressed.swap(0, Ordering::Relaxed))
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Count a malformed id and, at most once per interval, say so.
fn note_malformed(stored: &str) {
    let total = MALFORMED_ENTITY_IDS.fetch_add(1, Ordering::Relaxed) + 1;
    let Some(suppressed) = MALFORMED_WARN.allow(now_s(), MALFORMED_WARN_INTERVAL_S) else {
        return;
    };
    // `stored` is already cut to the id length; `?` escapes what the
    // sender put in it.
    tracing::warn!(
        "ict.entity.id" = ?stored,
        "ict.entity.id.malformed_total" = total,
        "log.suppressed" = suppressed,
        "malformed ict.entity.id received"
    );
}

/// What the lake stores of a malformed id: the value as received, or,
/// when it is longer than an id can be, its start with the truncation
/// mark as last character. The mark is outside the canonical form, so a
/// cut value never reads as a valid id.
fn bounded_entity_id(value: &str) -> Option<String> {
    if value.len() <= ENTITY_ID_MAX_LEN {
        return None;
    }
    let keep = ENTITY_ID_MAX_LEN - ENTITY_ID_TRUNCATION_MARK.len_utf8();
    let mut cut = truncated(value, keep).to_string();
    cut.push(ENTITY_ID_TRUNCATION_MARK);
    Some(cut)
}

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
    /// OTel semconv deployment environment (current and legacy spellings),
    /// accepted as the target environment when `resilience.target.environment`
    /// is absent.
    pub const DEPLOYMENT_ENVIRONMENT_NAME: &str = "deployment.environment.name";
    pub const DEPLOYMENT_ENVIRONMENT: &str = "deployment.environment";
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
    /// `resilience.target.*` carried as resource attributes: how an
    /// auto-instrumented application declares the system it is, once, at
    /// deploy time (span-level attributes still win when present).
    pub target_system: Option<String>,
    pub target_technology: Option<String>,
    pub target_environment: Option<String>,
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
            target_system: attr_string(attrs, keys::TARGET_SYSTEM),
            target_technology: attr_string(attrs, keys::TARGET_TECHNOLOGY),
            target_environment: attr_string(attrs, keys::TARGET_ENVIRONMENT)
                .or_else(|| attr_string(attrs, keys::DEPLOYMENT_ENVIRONMENT_NAME))
                .or_else(|| attr_string(attrs, keys::DEPLOYMENT_ENVIRONMENT)),
            resource_attrs: flag_malformed_entity_id(unpromoted_attrs(
                attrs,
                &[
                    keys::SERVICE_NAME,
                    keys::SERVICE_VERSION,
                    keys::EXPERIMENT_ID,
                    keys::EXPERIMENT_NAME,
                    keys::EXPERIMENT_TITLE,
                    keys::TARGET_SYSTEM,
                    keys::TARGET_TECHNOLOGY,
                    keys::TARGET_ENVIRONMENT,
                ],
            )),
        }
    }
}

/// Mark a resource whose `ict.entity.id` is not in the canonical form
/// with [`ENTITY_ID_MALFORMED_ATTR`]. The id itself is stored as
/// received: lower-casing it here would hide the sender's mistake and
/// could merge two different ids. Only its length is bounded (see
/// [`bounded_entity_id`]). A flag the sender set is dropped, so the
/// attribute always states this check's own verdict. Every malformed id
/// is counted ([`malformed_entity_ids_total`]).
fn flag_malformed_entity_id(mut attrs: Vec<(String, String)>) -> Vec<(String, String)> {
    attrs.retain(|(key, _)| key != ENTITY_ID_MALFORMED_ATTR);
    let mut malformed = false;
    for (_, value) in attrs
        .iter_mut()
        .filter(|(key, value)| key == ENTITY_ID_ATTR && !entity_id_is_valid(value))
    {
        malformed = true;
        if let Some(cut) = bounded_entity_id(value) {
            *value = cut;
        }
        note_malformed(value);
    }
    if malformed {
        attrs.push((ENTITY_ID_MALFORMED_ATTR.to_string(), "true".to_string()));
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;

    fn resource_with(attrs: &[(&str, &str)]) -> Resource {
        Resource {
            attributes: attrs
                .iter()
                .map(|(k, v)| kv(k, Value::StringValue((*v).into())))
                .collect(),
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }
    }

    fn flagged(attrs: &[(String, String)]) -> bool {
        attrs
            .iter()
            .any(|(k, v)| k == ENTITY_ID_MALFORMED_ATTR && v == "true")
    }

    fn entity_id(attrs: &[(String, String)]) -> &str {
        attrs
            .iter()
            .find(|(k, _)| k == ENTITY_ID_ATTR)
            .map(|(_, v)| v.as_str())
            .expect("the id is kept")
    }

    /// The lake never stores more of an id than an id can be long, and
    /// what it stores of an over-long one can never pass for a valid id.
    #[test]
    fn an_over_long_entity_id_is_stored_truncated_and_flagged() {
        // Truncating this one to 63 bytes would leave a valid-looking id.
        let long = format!("cif-{}", "0".repeat(5_000));
        let markup = format!("<img src=x onerror=alert(1)>{}", "å".repeat(5_000));
        for sent in [long, markup] {
            let resource = resource_with(&[("ict.entity.id", &sent)]);
            let ctx = ResourceCtx::from_resource(Some(&resource));
            let stored = entity_id(&ctx.resource_attrs);
            assert!(stored.len() <= ENTITY_ID_MAX_LEN, "{} bytes", stored.len());
            assert!(stored.ends_with(ENTITY_ID_TRUNCATION_MARK), "{stored}");
            assert!(sent.starts_with(stored.trim_end_matches(ENTITY_ID_TRUNCATION_MARK)));
            assert!(!entity_id_is_valid(stored), "{stored}");
            assert!(flagged(&ctx.resource_attrs));
        }
    }

    #[test]
    fn every_malformed_entity_id_is_counted() {
        let before = malformed_entity_ids_total();
        let bad = resource_with(&[("ict.entity.id", "CF-0042")]);
        let good = resource_with(&[("ict.entity.id", "cif-0042")]);
        let _ = ResourceCtx::from_resource(Some(&bad));
        let _ = ResourceCtx::from_resource(Some(&bad));
        let _ = ResourceCtx::from_resource(Some(&good));
        // Other tests count too, so the total is a floor.
        assert!(malformed_entity_ids_total() >= before + 2);
    }

    #[test]
    fn the_warning_is_written_at_most_once_per_interval_and_says_what_it_left_out() {
        let limit = WarnLimit::new();
        assert_eq!(limit.allow(1_000, 60), Some(0), "the first one is written");
        assert_eq!(limit.allow(1_000, 60), None);
        assert_eq!(limit.allow(1_030, 60), None);
        assert_eq!(limit.allow(1_059, 60), None);
        assert_eq!(limit.allow(1_060, 60), Some(3), "three were left out");
        assert_eq!(limit.allow(1_061, 60), None);
        assert_eq!(limit.allow(1_120, 60), Some(1));
    }

    #[test]
    fn a_malformed_entity_id_is_flagged_and_kept_as_received() {
        let resource = resource_with(&[("service.name", "checkout"), ("ict.entity.id", "CF-0042")]);
        let ctx = ResourceCtx::from_resource(Some(&resource));
        assert!(flagged(&ctx.resource_attrs), "{:?}", ctx.resource_attrs);
        assert!(
            ctx.resource_attrs
                .iter()
                .any(|(k, v)| k == "ict.entity.id" && v == "CF-0042"),
            "the id is not lower-cased or rewritten: {:?}",
            ctx.resource_attrs
        );
    }

    #[test]
    fn a_canonical_entity_id_carries_no_flag() {
        let resource = resource_with(&[("ict.entity.id", "cif-0042")]);
        let ctx = ResourceCtx::from_resource(Some(&resource));
        assert!(!flagged(&ctx.resource_attrs));
        assert_eq!(
            ctx.resource_attrs,
            vec![("ict.entity.id".to_string(), "cif-0042".to_string())]
        );
        let none = ResourceCtx::from_resource(Some(&resource_with(&[("k", "v")])));
        assert!(!flagged(&none.resource_attrs));
    }

    #[test]
    fn the_flag_is_ours_to_set_not_the_senders() {
        let resource = resource_with(&[
            ("ict.entity.id", "cif-0042"),
            (ENTITY_ID_MALFORMED_ATTR, "true"),
        ]);
        let ctx = ResourceCtx::from_resource(Some(&resource));
        assert!(!flagged(&ctx.resource_attrs), "{:?}", ctx.resource_attrs);
        let resource = resource_with(&[
            ("ict.entity.id", "Not An Id"),
            (ENTITY_ID_MALFORMED_ATTR, "false"),
        ]);
        let ctx = ResourceCtx::from_resource(Some(&resource));
        assert!(flagged(&ctx.resource_attrs));
        assert_eq!(
            ctx.resource_attrs
                .iter()
                .filter(|(k, _)| k == ENTITY_ID_MALFORMED_ATTR)
                .count(),
            1
        );
    }

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

    fn any(value: Value) -> AnyValue {
        AnyValue { value: Some(value) }
    }

    #[test]
    fn any_value_complex_variants_render() {
        use opentelemetry_proto::tonic::common::v1::{ArrayValue, KeyValueList};

        // Byte arrays hex-encode like ids do.
        let bytes = any(Value::BytesValue(vec![0x01, 0xff]));
        assert_eq!(any_value_to_string(&bytes), "01ff");

        // Arrays render as a JSON array of rendered elements.
        let array = any(Value::ArrayValue(ArrayValue {
            values: vec![any(Value::StringValue("a".into())), any(Value::IntValue(7))],
        }));
        assert_eq!(any_value_to_string(&array), "[\"a\",\"7\"]");

        // Kvlists render as a JSON object; entries without a value are dropped.
        let kvlist = any(Value::KvlistValue(KeyValueList {
            values: vec![
                kv("k", Value::BoolValue(false)),
                KeyValue {
                    key: "missing".into(),
                    value: None,
                    key_strindex: 0,
                },
            ],
        }));
        assert_eq!(any_value_to_string(&kvlist), "{\"k\":\"false\"}");

        // String-table interned values cannot be resolved here and render empty.
        let interned = any(Value::StringValueStrindex(3));
        assert_eq!(any_value_to_string(&interned), "");

        // An absent value renders empty.
        let absent = AnyValue { value: None };
        assert_eq!(any_value_to_string(&absent), "");
    }

    #[test]
    fn attr_helpers_treat_mismatches_and_missing_values_as_absent() {
        let attrs = vec![
            kv("n", Value::IntValue(41)),
            kv("s", Value::StringValue("x".into())),
            KeyValue {
                key: "noval".into(),
                value: None,
                key_strindex: 0,
            },
        ];
        assert!(attr_str(&attrs, "missing").is_none());
        assert!(attr_str(&attrs, "noval").is_none());
        assert!(attr_string(&attrs, "noval").is_none());
        // A non-bool value is not a bool.
        assert_eq!(attr_bool(&attrs, "n"), None);
        // Ints widen to doubles; strings do not.
        assert_eq!(attr_double(&attrs, "n"), Some(41.0));
        assert_eq!(attr_double(&attrs, "s"), None);
        assert!(attr_double(&attrs, "missing").is_none());
    }

    #[test]
    fn unpromoted_attrs_skip_promoted_keys_and_valueless_entries() {
        let attrs = vec![
            kv("keep", Value::StringValue("v".into())),
            kv("drop", Value::StringValue("x".into())),
            KeyValue {
                key: "noval".into(),
                value: None,
                key_strindex: 0,
            },
        ];
        assert_eq!(
            unpromoted_attrs(&attrs, &["drop"]),
            vec![("keep".to_string(), "v".to_string())]
        );
    }

    #[test]
    fn resource_ctx_defaults_when_resource_is_absent() {
        let ctx = ResourceCtx::from_resource(None);
        assert!(ctx.service_name.is_empty());
        assert!(ctx.service_version.is_none());
        assert!(ctx.experiment_id.is_none());
        assert!(ctx.experiment_name.is_none());
        assert!(ctx.resource_attrs.is_empty());
    }

    #[test]
    fn resource_ctx_experiment_name_prefers_standard_key_then_title() {
        let titled = Resource {
            attributes: vec![
                kv("service.name", Value::StringValue("tumultd".into())),
                kv("service.version", Value::StringValue("1.0.0".into())),
                kv(
                    "resilience.experiment.title",
                    Value::StringValue("cli-title".into()),
                ),
            ],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        };
        let ctx = ResourceCtx::from_resource(Some(&titled));
        assert_eq!(ctx.service_version.as_deref(), Some("1.0.0"));
        assert_eq!(ctx.experiment_name.as_deref(), Some("cli-title"));
        // title is promoted, so nothing remains in the attribute map
        assert!(ctx.resource_attrs.is_empty());

        let both = Resource {
            attributes: vec![
                kv(
                    "resilience.experiment.name",
                    Value::StringValue("standard-name".into()),
                ),
                kv(
                    "resilience.experiment.title",
                    Value::StringValue("cli-title".into()),
                ),
            ],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        };
        let ctx = ResourceCtx::from_resource(Some(&both));
        assert_eq!(ctx.experiment_name.as_deref(), Some("standard-name"));
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
