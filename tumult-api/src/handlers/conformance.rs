//! `GET /api/otel/conformance?range=&env=` — what the lake actually received
//! from each service against the naming contract (see
//! docs/guides/otel-autoinstrumentation.md). Per `service.name`: how many
//! spans arrived in the window, which contract attributes every span
//! carried (target columns, promoted from span or resource attributes; the
//! ICT spine from `resource_attrs`), the resulting level and what is
//! missing. Scoped principals see only their environments.
//!
//! The report also lists every `ict.entity.id` that is not in the
//! canonical form, exactly as received, per service. The ingest path
//! flags such resources; ids stored before that check existed are found
//! by running the same check here. The list is bounded both ways: an id
//! is shown cut to one character past the longest valid one, and at most
//! `MALFORMED_LIMIT` rows are returned; the document says when either
//! bound cut something.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tumult_core::compliance::{
    check_entity_id, ENTITY_ID_ATTR, ENTITY_ID_MALFORMED_ATTR, ENTITY_ID_MAX_LEN, ENTITY_ID_PATTERN,
};
use tumult_lake::scope;

use crate::auth::Principal;
use crate::error::{bad_request, ApiError};
use crate::sql_util::{now_ns, parse_range, with_reader};
use crate::ApiState;

/// Contract keys, in the order they are reported. The first three decide
/// `target`; all seven decide `spine`.
const TARGET_KEYS: [&str; 3] = [
    "resilience.target.system",
    "resilience.target.technology",
    "resilience.target.environment",
];
const SPINE_KEYS: [&str; 4] = [
    "ict.schema.version",
    "ict.entity.id",
    "ict.entity.tier",
    "ict.entity.owner",
];
/// Reported but not required for a level.
const RECOMMENDED_KEYS: [&str; 2] = ["resilience.target.component", "service.version"];

#[derive(Deserialize)]
pub(crate) struct ConformanceQuery {
    range: Option<String>,
    env: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Level {
    None,
    Target,
    Spine,
}

impl Level {
    /// Wire token, the same one the level serialises to.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Target => "target",
            Self::Spine => "spine",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct KeyPresence {
    key: String,
    /// Spans that carried the key, out of `spans`.
    present: u64,
    required_for: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ServiceConformance {
    service: String,
    spans: u64,
    last_seen_ns: Option<i64>,
    level: Level,
    missing: Vec<String>,
    keys: Vec<KeyPresence>,
}

/// Fold one SQL row (`service_name`, `spans`, `last_seen_ns`, and one
/// `k<i>` count per contract key) into the report line.
fn service_from(row: &Value) -> Option<ServiceConformance> {
    let service = row.get("service_name")?.as_str()?.to_string();
    let spans = row.get("spans").and_then(Value::as_u64).unwrap_or(0);
    let count = |i: usize| {
        row.get(format!("k{i}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let mut keys = Vec::new();
    let mut i = 0;
    for (group, required_for) in [
        (&TARGET_KEYS[..], "target"),
        (&SPINE_KEYS[..], "spine"),
        (&RECOMMENDED_KEYS[..], "recommended"),
    ] {
        for key in group {
            keys.push(KeyPresence {
                key: (*key).to_string(),
                present: count(i),
                required_for,
            });
            i += 1;
        }
    }
    let (level, missing) = level_for(spans, &keys);
    Some(ServiceConformance {
        service,
        spans,
        last_seen_ns: row.get("last_seen_ns").and_then(Value::as_i64),
        level,
        missing,
        keys,
    })
}

/// A key counts as carried only when every span in the window had it: a
/// contract that holds on half the spans is not a contract.
pub(crate) fn level_for(spans: u64, keys: &[KeyPresence]) -> (Level, Vec<String>) {
    let complete = |required_for: &str| {
        keys.iter()
            .filter(|k| k.required_for == required_for)
            .all(|k| spans > 0 && k.present == spans)
    };
    let missing: Vec<String> = keys
        .iter()
        .filter(|k| k.required_for != "recommended" && k.present < spans)
        .map(|k| k.key.clone())
        .collect();
    let level = if spans == 0 || !complete("target") {
        Level::None
    } else if complete("spine") {
        Level::Spine
    } else {
        Level::Target
    };
    (level, missing)
}

fn sql(from_ns: i64, scope: &str) -> String {
    let promoted = |col: &str| format!("COUNT(*) FILTER (WHERE {col} IS NOT NULL)");
    let resource = |key: &str| {
        format!("COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), '{key}') OR list_contains(map_keys(span_attrs), '{key}'))")
    };
    let mut cols = vec![
        promoted("target_system"),
        promoted("target_technology"),
        promoted("target_environment"),
    ];
    cols.extend(SPINE_KEYS.iter().map(|k| resource(k)));
    cols.push(resource("resilience.target.component"));
    cols.push(promoted("service_version"));
    let selected: Vec<String> = cols
        .into_iter()
        .enumerate()
        .map(|(i, c)| format!("{c} AS k{i}"))
        .collect();
    format!(
        "SELECT service_name, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns, {} \
         FROM spans WHERE ts_ns >= {from_ns} AND service_name <> ''{scope} \
         GROUP BY 1 ORDER BY 1",
        selected.join(", ")
    )
}

/// The most malformed ids one response lists.
pub(crate) const MALFORMED_LIMIT: usize = 500;
/// The most characters of a malformed id that are read from the lake:
/// one past the longest valid id, so an over-long id still reads as one.
const SHOWN_ID_CHARS: usize = ENTITY_ID_MAX_LEN + 1;

/// One malformed `ict.entity.id` as a service sent it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MalformedEntityId {
    service: String,
    /// The value as received, never lower-cased or repaired, cut to
    /// [`SHOWN_ID_CHARS`] characters.
    entity_id: String,
    /// Whether the stored id was longer than what is shown.
    truncated: bool,
    spans: u64,
    last_seen_ns: Option<i64>,
    /// Whether the ingest check marked it; `false` for older rows.
    flagged_at_ingest: bool,
    reason: &'static str,
}

/// The distinct (`service.name`, `ict.entity.id`) pairs of the window
/// whose id is not canonical. The lake does the check, cuts the id and
/// stops one row past the limit, so neither a long id nor many ids cost
/// more than the bound.
fn entity_ids_sql(from_ns: i64, scope: &str) -> String {
    let attr = |key: &str| format!("COALESCE(resource_attrs['{key}'], span_attrs['{key}'])");
    format!(
        "SELECT service_name, LEFT({id}, {shown}) AS entity_id, COUNT(*) AS spans, \
         MAX(ts_ns) AS last_seen_ns, \
         COUNT(*) FILTER (WHERE {flag} = 'true') AS flagged, \
         MAX(length({id})) > {shown} AS cut \
         FROM spans WHERE ts_ns >= {from_ns} AND service_name <> '' \
         AND {id} IS NOT NULL \
         AND (length({id}) > {max} OR NOT regexp_full_match({id}, '{pattern}')){scope} \
         GROUP BY 1, 2 ORDER BY 1, 2 LIMIT {limit}",
        id = attr(ENTITY_ID_ATTR),
        flag = attr(ENTITY_ID_MALFORMED_ATTR),
        shown = SHOWN_ID_CHARS,
        max = ENTITY_ID_MAX_LEN,
        pattern = ENTITY_ID_PATTERN,
        limit = MALFORMED_LIMIT + 1,
    )
}

/// The row as a malformed id, or `None` when the id is canonical.
fn malformed_from(row: &Value) -> Option<MalformedEntityId> {
    let entity_id = row.get("entity_id")?.as_str()?;
    let fault = check_entity_id(entity_id).err()?;
    Some(MalformedEntityId {
        service: row.get("service_name")?.as_str()?.to_string(),
        entity_id: fault.value,
        truncated: row.get("cut").and_then(Value::as_bool).unwrap_or(false),
        spans: row.get("spans").and_then(Value::as_u64).unwrap_or(0),
        last_seen_ns: row.get("last_seen_ns").and_then(Value::as_i64),
        flagged_at_ingest: row.get("flagged").and_then(Value::as_u64).unwrap_or(0) > 0,
        reason: fault.reason,
    })
}

/// Conformance level per service since `from_ns`, confined to `envs`
/// (empty = every environment). The Star Map colours services by it.
pub(crate) fn levels(
    reader: &tumult_lake::Reader,
    from_ns: i64,
    envs: &[String],
) -> Result<std::collections::BTreeMap<String, &'static str>, String> {
    let scope = scope::and(scope::in_list("target_environment", envs));
    let rows = reader
        .query_json_rows(&sql(from_ns, &scope))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(service_from)
        .map(|s| (s.service, s.level.as_str()))
        .collect())
}

pub(crate) async fn conformance(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ConformanceQuery>,
) -> Result<Json<Value>, ApiError> {
    let range = q.range.unwrap_or_else(|| "24h".into());
    let Some(range_s) = parse_range(&range) else {
        return Err(bad_request(format!("invalid range {range}")));
    };
    let range_ns = range_s * 1_000_000_000;
    let envs: Vec<String> = match q.env {
        Some(env) if !principal.env_allowed(&env) => {
            return Err(crate::error::forbidden(format!(
                "environment {env} is outside the principal's scopes"
            )))
        }
        Some(env) => vec![env],
        None => principal.env_scopes.clone(),
    };
    let as_of_ns = now_ns();
    let scope = scope::and(scope::in_list("target_environment", &envs));
    let query = sql(as_of_ns - range_ns, &scope);
    let ids_query = entity_ids_sql(as_of_ns - range_ns, &scope);
    let (services, mut malformed): (Vec<ServiceConformance>, Vec<MalformedEntityId>) =
        with_reader(&state.db_path, move |reader| {
            let rows = reader.query_json_rows(&query).map_err(|e| e.to_string())?;
            let ids = reader
                .query_json_rows(&ids_query)
                .map_err(|e| e.to_string())?;
            Ok((
                rows.iter().filter_map(service_from).collect(),
                ids.iter().filter_map(malformed_from).collect(),
            ))
        })
        .await?;
    let more = malformed.len() > MALFORMED_LIMIT;
    malformed.truncate(MALFORMED_LIMIT);
    let summary = |level: Level| services.iter().filter(|s| s.level == level).count();
    Ok(Json(serde_json::json!({
        "as_of_ns": as_of_ns,
        "range": range,
        "contract": {
            "target": TARGET_KEYS,
            "spine": SPINE_KEYS,
            "recommended": RECOMMENDED_KEYS,
        },
        "entity_id_pattern": ENTITY_ID_PATTERN,
        "summary": {
            "services": services.len(),
            "spine": summary(Level::Spine),
            "target": summary(Level::Target),
            "none": summary(Level::None),
            "malformed_entity_ids": malformed.len(),
        },
        "services": services,
        "malformed_entity_ids": malformed,
        "malformed_entity_ids_truncated": more,
        "malformed_entity_ids_limit": MALFORMED_LIMIT,
    })))
}

/// The statements of the conformance view, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    let scope = scope::and(scope::in_list("target_environment", &["prod".to_string()]));
    tumult_test_utils::sql_statements![
        sql(1_774_900_000_000_000_000, ""),
        sql(1_774_900_000_000_000_000, &scope),
        entity_ids_sql(1_774_900_000_000_000_000, &scope),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(target: [u64; 3], spine: [u64; 4]) -> Vec<KeyPresence> {
        let mut out = Vec::new();
        for (i, k) in TARGET_KEYS.iter().enumerate() {
            out.push(KeyPresence {
                key: (*k).into(),
                present: target[i],
                required_for: "target",
            });
        }
        for (i, k) in SPINE_KEYS.iter().enumerate() {
            out.push(KeyPresence {
                key: (*k).into(),
                present: spine[i],
                required_for: "spine",
            });
        }
        out.push(KeyPresence {
            key: "service.version".into(),
            present: 0,
            required_for: "recommended",
        });
        out
    }

    #[test]
    fn every_span_must_carry_the_target_for_the_target_level() {
        let (level, missing) = level_for(10, &keys([10, 10, 10], [0, 0, 0, 0]));
        assert_eq!(level, Level::Target);
        assert_eq!(missing, SPINE_KEYS.map(String::from).to_vec());
        let (level, missing) = level_for(10, &keys([10, 9, 10], [0, 0, 0, 0]));
        assert_eq!(level, Level::None);
        assert_eq!(missing[0], "resilience.target.technology");
        assert!(
            !missing.contains(&"service.version".to_string()),
            "recommended keys never count as missing"
        );
    }

    #[test]
    fn the_spine_on_top_of_the_target_is_spine_level() {
        let (level, missing) = level_for(4, &keys([4, 4, 4], [4, 4, 4, 4]));
        assert_eq!(level, Level::Spine);
        assert!(missing.is_empty());
        let (level, _) = level_for(0, &keys([0, 0, 0], [0, 0, 0, 0]));
        assert_eq!(level, Level::None);
    }

    #[test]
    fn rows_fold_into_services() {
        let mut row = serde_json::Map::new();
        row.insert("service_name".into(), "checkout".into());
        row.insert("spans".into(), 3.into());
        row.insert("last_seen_ns".into(), 99.into());
        for i in 0..9 {
            row.insert(format!("k{i}"), 3.into());
        }
        let s = service_from(&Value::Object(row)).unwrap();
        assert_eq!(
            (s.service.as_str(), s.spans, s.last_seen_ns),
            ("checkout", 3, Some(99))
        );
        assert_eq!(s.level, Level::Spine);
        assert_eq!(
            serde_json::json!(s.level),
            s.level.as_str(),
            "token and serialised form agree"
        );
        assert_eq!(s.keys.len(), 9);
        assert_eq!(s.keys[8].key, "service.version");
    }

    #[test]
    fn only_non_canonical_ids_fold_into_the_malformed_list() {
        let row = |id: &str, flagged: u64| {
            serde_json::json!({
                "service_name": "checkout", "entity_id": id, "spans": 4,
                "last_seen_ns": 7, "flagged": flagged, "cut": false,
            })
        };
        assert_eq!(malformed_from(&row("cif-0042", 0)), None);
        let bad = malformed_from(&row("CF-0042", 4)).unwrap();
        assert_eq!(bad.entity_id, "CF-0042", "kept as received");
        assert_eq!((bad.spans, bad.last_seen_ns), (4, Some(7)));
        assert!(bad.flagged_at_ingest);
        assert_eq!(bad.reason, "must start with a lower-case letter");
        assert!(
            !malformed_from(&row("cif_0042", 0))
                .unwrap()
                .flagged_at_ingest
        );
    }

    #[test]
    fn the_id_query_reads_the_id_and_the_ingest_flag() {
        let q = entity_ids_sql(100, " AND target_environment IN ('demo')");
        assert!(q.contains("resource_attrs['ict.entity.id']"));
        assert!(q.contains("resource_attrs['tumult.ict.entity.id.malformed']"));
        assert!(q.contains("ts_ns >= 100") && q.contains("IN ('demo')"));
    }

    #[test]
    fn the_id_query_is_bounded_in_length_and_in_rows() {
        let q = entity_ids_sql(100, "");
        assert!(q.contains("LEFT(COALESCE(resource_attrs['ict.entity.id']"));
        assert!(q.contains(", 64) AS entity_id"), "{q}");
        assert!(q.ends_with("LIMIT 501"), "{q}");
        assert!(q.contains("NOT regexp_full_match("), "{q}");
        let cut = serde_json::json!({
            "service_name": "s", "entity_id": "X".repeat(64), "spans": 1,
            "last_seen_ns": 1, "flagged": 0, "cut": true,
        });
        let bad = malformed_from(&cut).unwrap();
        assert!(bad.truncated);
        assert_eq!(bad.reason, "longer than 63 characters");
    }

    #[test]
    fn sql_counts_each_contract_key_in_order() {
        let q = sql(100, " AND target_environment IN ('demo')");
        assert!(q.contains("ts_ns >= 100"));
        assert!(q.contains("AS k0") && q.contains("AS k8"));
        assert!(q.contains("map_keys(resource_attrs), 'ict.entity.owner'"));
        assert!(q.contains("target_environment IN ('demo')"));
    }
}
