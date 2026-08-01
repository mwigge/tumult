//! `GET /api/reports*` (v1 digests and v2 compliance reports) and
//! `GET /api/scores*` (resilience scorecard, org tree rollup).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{
    internal, now_ns, parse_range, scope_scorecard, scoped_experiment_names, windows, with_reader,
};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/reports + /api/reports/{name}

/// May a principal with `scopes` see a report artifact whose generation-time
/// coverage is `coverage` (`Some(envs)` = built from those environments
/// only, `None` = global artifact or legacy file with no metadata)?
/// Unscoped principals (empty set) see everything; scoped principals only
/// see artifacts whose recorded coverage lies fully inside their scopes —
/// global and legacy artifacts fail closed (hidden in lists, 404 on fetch,
/// no existence leak).
fn artifact_visible(scopes: &[String], coverage: Option<Vec<String>>) -> bool {
    scopes.is_empty() || coverage.is_some_and(|envs| envs.iter().all(|e| scopes.contains(e)))
}

/// The `env_scopes` array of an artifact's metadata JSON (`null`/absent =
/// global or legacy coverage).
fn meta_coverage(meta: &Value) -> Option<Vec<String>> {
    meta.get("env_scopes")?.as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

/// Coverage sidecar of a v1 digest (`<name>.meta.json` next to the `.html`).
fn digest_coverage(dir: &std::path::Path, name: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join(format!("{name}.meta.json"))).ok()?;
    meta_coverage(&serde_json::from_str::<Value>(&text).ok()?)
}

/// The coverage to record for a freshly generated artifact: the principal's
/// scopes, or `Value::Null` for an unscoped (global) generation.
fn generation_coverage(scopes: &[String]) -> Value {
    if scopes.is_empty() {
        Value::Null
    } else {
        json!(scopes)
    }
}

pub(crate) async fn list_reports(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Json<Value> {
    let mut reports = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state.reports_dir.as_ref()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "html") {
                let name = entry.file_name().to_string_lossy().to_string();
                if !artifact_visible(
                    &principal.env_scopes,
                    digest_coverage(&state.reports_dir, &name),
                ) {
                    continue;
                }
                let meta = entry.metadata().ok();
                let modified_s = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                reports.push(json!({
                    "name": name,
                    "bytes": meta.map_or(0, |m| m.len()),
                    "modified_s": modified_s,
                }));
            }
        }
    }
    // Timestamp-prefixed names sort newest first, lexicographically.
    reports.sort_by_key(|r| r.get("name").and_then(Value::as_str).map(str::to_owned));
    reports.reverse();
    Json(json!({"reports": reports}))
}

pub(crate) async fn get_report(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
) -> Response {
    // No path traversal: a report name is a flat file name only.
    if name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".html")
        || name.len() > 200
    {
        return (StatusCode::BAD_REQUEST, "invalid report name").into_response();
    }
    // Scoped principals cannot open global or legacy digests (404 — no
    // existence leak, matching the trace/metric behaviour).
    if !artifact_visible(
        &principal.env_scopes,
        digest_coverage(&state.reports_dir, &name),
    ) {
        return (StatusCode::NOT_FOUND, "report not found").into_response();
    }
    match std::fs::read_to_string(state.reports_dir.join(&name)) {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct GenerateRequest {
    metric: String,
}

/// `POST /api/reports/generate {metric}` — manual counterpart to the
/// scheduler: render one metric digest now (over all stored data, matching
/// `GET /report?metric=`), write it into the reports dir so it appears in
/// `GET /api/reports`, and return its name. Manual digests carry a
/// `manual_<metric>_<epoch>.html` name, distinct from the scheduler's
/// `report_<epoch>.html`. A scoped principal's digest is confined to its
/// environments, and a `<name>.meta.json` sidecar records that coverage so
/// the list/get endpoints can enforce it on later reads.
pub(crate) async fn generate_report(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<GenerateRequest>,
) -> Result<Json<Value>, Response> {
    let metric = req.metric.trim().to_string();
    if metric.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "metric must not be empty"})),
        )
            .into_response());
    }
    let metrics_dir = state.metrics_dir.as_ref().clone();
    let reports_dir = state.reports_dir.as_ref().clone();
    let llm = state.llm.clone();
    let metric_name = metric.clone();
    let scopes = principal.env_scopes.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let defs =
            tumult_metrics::load_dir(&metrics_dir).map_err(|e| format!("load metrics: {e}"))?;
        let Some(def) = defs.iter().find(|d| d.name == metric_name) else {
            return Ok(None);
        };
        let report = tumult_report::build_report_scoped(
            reader,
            std::slice::from_ref(def),
            &format!("Tumult — {metric_name}"),
            None,
            &scopes,
        )
        .map_err(|e| e.to_string())?;
        Ok(Some(report))
    })
    .await?;
    let Some(report) = body else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {metric:?} not found; see /api/metrics")})),
        )
            .into_response());
    };
    // Best-effort LLM narrative: unreachable/unconfigured LLM or a reply
    // with no grounded sentences leaves the digest unchanged.
    let report =
        tumult_report::narrative::narrate(&llm, report, std::time::Duration::from_secs(30)).await;
    let html = tumult_report::render_html(&report);
    std::fs::create_dir_all(&reports_dir).map_err(|e| internal(e.to_string()))?;
    let now_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = format!("manual_{metric}_{now_s}.html");
    std::fs::write(
        reports_dir.join(format!("{name}.meta.json")),
        json!({"env_scopes": generation_coverage(&principal.env_scopes)}).to_string(),
    )
    .map_err(|e| internal(e.to_string()))?;
    std::fs::write(reports_dir.join(&name), &html).map_err(|e| internal(e.to_string()))?;
    Ok(Json(
        json!({"name": name, "metric": metric, "bytes": html.len()}),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/scores + /api/reports/v2/*

#[derive(Deserialize)]
pub(crate) struct ScoresQuery {
    range: Option<String>,
}

/// `GET /api/scores?range=24h|7d|14d` — resilience scorecard as of now,
/// with the portfolio delta against the previous equal window. Scoped
/// principals get the card of their own environments only (rollups
/// recomputed over the visible experiments).
pub(crate) async fn scores(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScoresQuery>,
) -> Result<Json<Value>, Response> {
    let range = q.range.as_deref().unwrap_or("7d");
    let Some(((from, to), _)) = windows(range) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "range must be one of 24h|7d|14d"})),
        )
            .into_response());
    };
    let scopes = principal.env_scopes.clone();
    let card = with_reader(&state.db_path, move |reader| {
        let mut card = scope_scorecard(
            reader,
            tumult_compliance::scoring::compute(reader, to, None)?,
            &scopes,
        )?;
        let prev = scope_scorecard(
            reader,
            tumult_compliance::scoring::compute(reader, from, None)?,
            &scopes,
        )?;
        card.delta = Some(card.portfolio - prev.portfolio);
        Ok(card)
    })
    .await?;
    Ok(Json(
        serde_json::to_value(card).map_err(|e| internal(e.to_string()))?,
    ))
}

#[derive(Deserialize)]
pub(crate) struct TreeParams {
    node: Option<String>,
    range: Option<String>,
}

/// `GET /api/scores/tree?node=<path>&range=24h|7d|14d` — org rollup for one
/// node: criticality-weighted score recomputed from all leaves in its
/// subtree, coverage, a period sparkline, and one level of child rollups.
pub(crate) async fn scores_tree(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<TreeParams>,
) -> Result<Json<Value>, Response> {
    let node = params.node.unwrap_or_default();
    let node = node.trim_matches('/').to_string();
    if state.org.resolve(&node).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown org node {node:?}")})),
        )
            .into_response());
    }
    let Some(secs) = parse_range(params.range.as_deref().unwrap_or("7d")) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "range must be one of 24h|7d|14d"})),
        )
            .into_response());
    };
    let period_ns = secs * 1_000_000_000;
    let as_of = now_ns();
    let org = state.org.clone();
    let scopes = principal.env_scopes.clone();

    let payload = with_reader(&state.db_path, move |reader| {
        // Scoped principals roll up only the experiments they can see.
        let in_scope = scoped_experiment_names(reader, &scopes)?;
        // Leaves at an instant: every scored experiment plus pending manual
        // records (expected but unscored). Pending status is read as of NOW
        // for every sample point — a documented approximation, since the
        // lifecycle has no history before the audit trail.
        let leaves_at = |t: i64| -> Result<Vec<tumult_compliance::ScoredLeaf>, String> {
            let card = tumult_compliance::scoring::compute(reader, t, None)?;
            let mut leaves: Vec<tumult_compliance::ScoredLeaf> = card
                .experiments
                .iter()
                .map(|e| tumult_compliance::ScoredLeaf {
                    name: e.name.clone(),
                    score: Some(e.score),
                })
                .collect();
            leaves.extend(
                tumult_compliance::scoring::pending_manual_leaves(reader)?
                    .into_iter()
                    .map(|name| tumult_compliance::ScoredLeaf { name, score: None }),
            );
            if let Some(names) = &in_scope {
                leaves.retain(|l| names.contains(&l.name));
            }
            Ok(leaves)
        };

        let current = org
            .compute_node(&node, &leaves_at(as_of)?)
            .ok_or_else(|| format!("unknown org node {node:?}"))?;
        let previous = org
            .compute_node(&node, &leaves_at(as_of - period_ns)?)
            .ok_or_else(|| format!("unknown org node {node:?}"))?;

        const POINTS: i64 = 10;
        let step = period_ns / POINTS;
        let mut sparkline = Vec::with_capacity(POINTS as usize);
        for i in 1..=POINTS {
            let t = as_of - period_ns + step * i;
            let score = org
                .compute_node(&node, &leaves_at(t)?)
                .map_or(0.0, |n| n.score);
            sparkline.push(vec![json!(i), json!(score)]);
        }

        Ok(json!({
            "path": current.path,
            "name": current.name,
            "kind": current.kind,
            "score": current.score,
            "band": current.band,
            "delta": current.score - previous.score,
            "coverage": current.coverage,
            "scored": current.scored,
            "expected": current.expected,
            "weakest": current.weakest,
            "weight": current.weight,
            "sparkline": sparkline,
            "children": current.children,
        }))
    })
    .await?;
    Ok(Json(payload))
}

#[derive(Deserialize)]
pub(crate) struct GenerateV2Request {
    #[serde(rename = "type")]
    kind: String,
    period: Option<String>,
    experiment_id: Option<String>,
    framework: Option<String>,
}

/// `POST /api/reports/v2/generate` — build one compliance-grade report and
/// persist `{id}.pdf`, `{id}.html` and `{id}.json` under `reports/v2/`.
pub(crate) async fn generate_report_v2(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<GenerateV2Request>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Ok(kind) = serde_json::from_value::<tumult_compliance::TemplateKind>(json!(req.kind))
    else {
        return Err(bad(format!(
            "unknown type {:?}; expected executive-digest|game-day|evidence-pack",
            req.kind
        )));
    };
    let period_ns = match req.period.as_deref() {
        None => 7 * 86_400 * 1_000_000_000i64,
        Some(p) => match parse_range(p) {
            Some(secs) => secs * 1_000_000_000,
            None => return Err(bad("period must be one of 24h|7d|14d".into())),
        },
    };
    if kind == tumult_compliance::TemplateKind::GameDay
        && req.experiment_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(bad("game-day requires experiment_id".into()));
    }
    if kind == tumult_compliance::TemplateKind::EvidencePack {
        match req.framework.as_deref() {
            None => return Err(bad("evidence-pack requires framework".into())),
            Some(f)
                if !tumult_compliance::builders::FRAMEWORK_CLAUSES
                    .iter()
                    .any(|(name, _)| *name == f.to_ascii_lowercase()) =>
            {
                return Err(bad(format!(
                    "unknown framework {f:?}; expected dora|nis2|iso27001|soc2"
                )));
            }
            _ => {}
        }
    }

    let generated_at = now_ns();
    let exp_id = req.experiment_id.clone();
    let framework = req.framework.clone();
    let org = state.org.clone();
    let scopes = principal.env_scopes.clone();
    let built = with_reader(&state.db_path, move |reader| match kind {
        tumult_compliance::TemplateKind::ExecutiveDigest => {
            tumult_compliance::builders::build_executive(
                reader,
                &org,
                generated_at,
                period_ns,
                generated_at,
                &scopes,
            )
            .map(Some)
        }
        tumult_compliance::TemplateKind::GameDay => {
            // The builder confines the run's root span to the principal's
            // environments; an out-of-scope id comes back as `None` (404 —
            // no existence leak across scopes).
            tumult_compliance::builders::build_game_day(
                reader,
                exp_id.as_deref().unwrap_or_default(),
                generated_at,
                &scopes,
            )
        }
        tumult_compliance::TemplateKind::EvidencePack => {
            tumult_compliance::builders::build_evidence_pack(
                reader,
                framework.as_deref().unwrap_or_default(),
                Some(period_ns),
                generated_at,
                &scopes,
            )
            .map(Some)
        }
    })
    .await?;
    let Some(doc) = built else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "experiment_id not found"})),
        )
            .into_response());
    };

    let pdf =
        tumult_compliance::typst_pdf::render_pdf(&doc).map_err(|e| internal(e.to_string()))?;
    let html = tumult_compliance::html::render(&doc);
    let sha256: String = {
        use sha2::Digest as _;
        sha2::Sha256::digest(&pdf)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };

    let v2_dir = state.reports_dir.join("v2");
    std::fs::create_dir_all(&v2_dir).map_err(|e| internal(e.to_string()))?;
    let id = &doc.meta.doc_id;
    let meta = json!({
        "doc_id": id,
        "type": req.kind,
        "title": doc.meta.title,
        "created_ns": doc.meta.generated_at_ns,
        "data_as_of_ns": doc.meta.data_as_of_ns,
        "bytes": pdf.len(),
        "sha256": sha256,
        // Generation-time environment coverage: the list/pdf/html endpoints
        // confine scoped principals to artifacts inside their scopes.
        "env_scopes": generation_coverage(&principal.env_scopes),
        "params": {
            "period": req.period,
            "experiment_id": req.experiment_id,
            "framework": req.framework,
        },
    });
    std::fs::write(v2_dir.join(format!("{id}.pdf")), &pdf).map_err(|e| internal(e.to_string()))?;
    std::fs::write(v2_dir.join(format!("{id}.html")), &html)
        .map_err(|e| internal(e.to_string()))?;
    std::fs::write(
        v2_dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| internal(e.to_string()))?;
    Ok(Json(meta))
}

/// `GET /api/reports/v2` — metas of every generated v2 report, newest first.
/// Scoped principals see only reports whose recorded environment coverage
/// lies inside their scopes; global and legacy (pre-coverage) reports fail
/// closed for them.
pub(crate) async fn list_reports_v2(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Json<Value> {
    let mut reports = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state.reports_dir.join("v2")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(meta) = serde_json::from_str::<Value>(&text) {
                        if artifact_visible(&principal.env_scopes, meta_coverage(&meta)) {
                            reports.push(meta);
                        }
                    }
                }
            }
        }
    }
    reports.sort_by_key(|r| r.get("created_ns").and_then(Value::as_i64).unwrap_or(0));
    reports.reverse();
    Json(json!({"reports": reports}))
}

/// A doc id is `KRK-<code>-<yyyymmdd>-<hash6>`: flat, safe charset.
fn valid_doc_id(id: &str) -> bool {
    id.starts_with("KRK-")
        && id.len() <= 100
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether the principal may open the v2 artifact `id`, per the coverage
/// recorded in its `{id}.json` meta (missing meta = legacy = fail closed
/// for scoped principals).
fn v2_artifact_visible(dir: &std::path::Path, id: &str, scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return true;
    }
    let coverage = std::fs::read_to_string(dir.join("v2").join(format!("{id}.json")))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|meta| meta_coverage(&meta));
    artifact_visible(scopes, coverage)
}

pub(crate) async fn get_report_v2_pdf(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Response {
    if !valid_doc_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid document id").into_response();
    }
    if !v2_artifact_visible(&state.reports_dir, &id, &principal.env_scopes) {
        return (StatusCode::NOT_FOUND, "report not found").into_response();
    }
    match std::fs::read(state.reports_dir.join("v2").join(format!("{id}.pdf"))) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/pdf")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}

pub(crate) async fn get_report_v2_html(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Response {
    if !valid_doc_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid document id").into_response();
    }
    if !v2_artifact_visible(&state.reports_dir, &id, &principal.env_scopes) {
        return (StatusCode::NOT_FOUND, "report not found").into_response();
    }
    match std::fs::read_to_string(state.reports_dir.join("v2").join(format!("{id}.html"))) {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}
