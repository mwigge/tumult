//! `GET /api/otel/conformance?range=&env=` — what the lake actually received
//! from each service against the naming contract (see
//! docs/guides/otel-autoinstrumentation.md). Per `service.name`: how many
//! spans arrived in the window, which contract attributes every span
//! carried (target columns, promoted from span or resource attributes; the
//! ICT spine from `resource_attrs`), the resulting level and what is
//! missing. Scoped principals see only their environments.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::Principal;
use crate::error::{bad_request, ApiError};
use crate::sql_util::{and_pred, env_scope_where, now_ns, parse_range, with_reader};
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

/// Conformance level per service since `from_ns`, confined to `envs`
/// (empty = every environment). The Star Map colours services by it.
pub(crate) fn levels(
    reader: &tumult_lake::Reader,
    from_ns: i64,
    envs: &[String],
) -> Result<std::collections::BTreeMap<String, &'static str>, String> {
    let scope = and_pred(env_scope_where("target_environment", envs));
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
    let scope = and_pred(env_scope_where("target_environment", &envs));
    let query = sql(as_of_ns - range_ns, &scope);
    let services: Vec<ServiceConformance> = with_reader(&state.db_path, move |reader| {
        let rows = reader.query_json_rows(&query).map_err(|e| e.to_string())?;
        Ok(rows.iter().filter_map(service_from).collect())
    })
    .await?;
    let summary = |level: Level| services.iter().filter(|s| s.level == level).count();
    Ok(Json(serde_json::json!({
        "as_of_ns": as_of_ns,
        "range": range,
        "contract": {
            "target": TARGET_KEYS,
            "spine": SPINE_KEYS,
            "recommended": RECOMMENDED_KEYS,
        },
        "summary": {
            "services": services.len(),
            "spine": summary(Level::Spine),
            "target": summary(Level::Target),
            "none": summary(Level::None),
        },
        "services": services,
    })))
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
    fn sql_counts_each_contract_key_in_order() {
        let q = sql(100, " AND target_environment IN ('demo')");
        assert!(q.contains("ts_ns >= 100"));
        assert!(q.contains("AS k0") && q.contains("AS k8"));
        assert!(q.contains("map_keys(resource_attrs), 'ict.entity.owner'"));
        assert!(q.contains("target_environment IN ('demo')"));
    }
}
