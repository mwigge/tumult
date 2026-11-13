//! DORA coverage and the internal maturity level.
//!
//! * `GET /api/dora?lei=&profile=&by=entity|org&env=` — coverage per
//!   article and per function (or org unit) of one legal entity, the
//!   expectation table and the reporting-readiness facts.
//! * `GET /api/dora/{article}?entity=&lei=&profile=&env=` — one article
//!   with every evidence row behind each cell.
//! * `GET /api/maturity?lei=&profile=&by=node|entity|org&env=` — the
//!   level of the legal entity with its pillars and gates, its history by
//!   quarter, and the per-node roll-up with the cap chain.
//!
//! Every number is served with its unknown share. `profile` scores the
//! same lake under another entity profile without changing the register.
//! Scoped principals see only their own environments; `env` narrows the
//! score to one of them.
//!
//! Every document carries the `params` block and the register state of
//! [`crate::score::ScoreRuntime`], and is reused from its cache for a few
//! seconds: a score is at most the cache TTL behind the lake.

use std::path::Path as FsPath;
use std::sync::Arc;

use crate::score::Stamp;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;
use tumult_compliance::dora::coverage::Matrix;
use tumult_compliance::dora::loader::{self, LoadError, Loaded, Request};
use tumult_compliance::dora::model::Workload;
use tumult_compliance::dora::params::HouseParams;
use tumult_compliance::dora::report::{self, DoraBy, MaturityBy, ReportError};
use tumult_lake::Reader;

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, ApiError};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

#[derive(Deserialize)]
pub(crate) struct ScoreQuery {
    lei: Option<String>,
    profile: Option<String>,
    by: Option<String>,
    env: Option<String>,
    /// `/api/dora/{article}` only: narrow the cells to one function.
    entity: Option<String>,
}

/// Why a score could not be served.
enum Refusal {
    /// The request names something that does not exist (400).
    Request(String),
    /// The article or function is not there (404).
    Missing(String),
}

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Request(msg) => bad_request(msg),
            Refusal::Missing(msg) => not_found(msg),
        }
    }
}

/// What every score request carries once the principal is checked.
struct Scope {
    envs: Vec<String>,
    lei: Option<String>,
    profile: Option<String>,
    as_of_ns: i64,
    workloads: Arc<Vec<Workload>>,
    /// The daemon was started with the proof opt-in.
    proof_days: bool,
    params: Arc<HouseParams>,
    /// What the document says about those parameters and the register.
    stamp: Stamp,
    /// The generation of the score runtime the request started under.
    generation: u64,
    /// Where the response is cached; `None` when it is not.
    key: Option<String>,
}

impl Scope {
    fn request(&self) -> Request<'_> {
        Request {
            envs: &self.envs,
            lei: self.lei.as_deref(),
            profile: self.profile.as_deref(),
            as_of_ns: self.as_of_ns,
            workloads: &self.workloads,
            proof_days: self.proof_days,
        }
    }
}

fn envs_for(principal: &Principal, env: Option<String>) -> Result<Vec<String>, ApiError> {
    let Some(env) = env else {
        return Ok(principal.env_scopes.clone());
    };
    if !principal.env_allowed(&env) {
        return Err(forbidden(format!(
            "environment {env} is outside the principal's scopes"
        )));
    }
    Ok(vec![env])
}

/// The node id of a manifest workload and the service whose telemetry
/// joins it: the `app` label, else the workload's own name.
fn workload(w: &tumult_risk::Workload) -> Workload {
    let namespace = w.namespace.as_deref().unwrap_or("default");
    let service = w.labels.get("app").unwrap_or(&w.name);
    Workload {
        id: format!("wl:{namespace}/{}/{}", w.kind.to_lowercase(), w.name),
        service: Some(service.clone()),
    }
}

/// Workloads of the manifests directory and a note for each document it
/// skipped; nothing when no directory is configured. Reads the directory:
/// requests use the copy [`crate::score::ScoreRuntime`] holds.
///
/// # Errors
/// Returns a message when the directory cannot be read.
pub fn read_manifests(dir: Option<&FsPath>) -> Result<(Vec<Workload>, Vec<String>), String> {
    let Some(dir) = dir else {
        return Ok((Vec::new(), Vec::new()));
    };
    let manifests = tumult_risk::manifests::load_dir(dir)
        .map_err(|e| format!("manifests at {}: {e}", dir.display()))?;
    let workloads = manifests.workloads.iter().map(workload).collect();
    Ok((workloads, manifests.notes))
}

/// A longer key is a request nobody repeats; it is served uncached.
const CACHE_KEY_MAX_LEN: usize = 512;

/// Everything a score document depends on besides the lake and the
/// runtime: the endpoint, the environments the caller may see and the
/// query. Two callers with different scopes never share an entry.
fn cache_key(
    endpoint: &str,
    article: Option<&str>,
    envs: &[String],
    q: &ScoreQuery,
) -> Option<String> {
    let key =
        serde_json::json!([endpoint, article, envs, q.lei, q.profile, q.by, q.entity]).to_string();
    (key.len() <= CACHE_KEY_MAX_LEN).then_some(key)
}

fn scope(
    state: &ApiState,
    principal: &Principal,
    q: &ScoreQuery,
    endpoint: &str,
    article: Option<&str>,
) -> Result<Scope, ApiError> {
    let envs = envs_for(principal, q.env.clone())?;
    let runtime = state.score_runtime();
    // Read before the parameters and the workloads: a reload in between
    // moves the generation on, and this response is then not cached.
    let generation = runtime.generation();
    let workloads = runtime.workloads().map_err(internal)?;
    let view = runtime.view();
    Ok(Scope {
        key: cache_key(endpoint, article, &envs, q),
        generation,
        envs,
        lei: q.lei.clone(),
        profile: q.profile.clone(),
        as_of_ns: now_ns(),
        workloads,
        proof_days: runtime.proof_days(),
        params: view.params,
        stamp: view.stamp,
    })
}

/// A load failure is the caller's when it names an unknown LEI or
/// profile, and the store's otherwise.
fn load(reader: &Reader, scope: &Scope) -> Result<Result<Loaded, Refusal>, String> {
    match loader::load(reader, scope.request(), &scope.params) {
        Ok(loaded) => Ok(Ok(loaded)),
        Err(LoadError::Store(e)) => Err(e),
        Err(e) => Ok(Err(Refusal::Request(e.to_string()))),
    }
}

/// Load the legal entity and build one document from its scored matrix,
/// or reuse the one built for the same request a moment ago.
async fn document(
    state: &ApiState,
    scope: Scope,
    build: impl FnOnce(&Reader, &Scope, &Loaded, &Matrix<'_>) -> Result<Result<Value, Refusal>, String>
        + Send
        + 'static,
) -> Result<Json<Value>, ApiError> {
    let runtime = Arc::clone(state.score_runtime());
    let (key, generation) = (scope.key.clone(), scope.generation);
    let stamp = scope.stamp.clone();
    let hit = key.as_deref().and_then(|key| runtime.cached(key));
    if let Some(doc) = hit {
        return Ok(Json(doc));
    }
    let outcome = with_reader(&state.db_path, move |reader| {
        let loaded = match load(reader, &scope)? {
            Ok(loaded) => loaded,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let matrix = Matrix::build(&loaded.inputs, &scope.params);
        build(reader, &scope, &loaded, &matrix)
    })
    .await?;
    let mut doc = outcome?;
    stamp.apply(&mut doc);
    if let Some(key) = key {
        runtime.store(key, generation, &doc);
    }
    Ok(Json(doc))
}

pub(crate) async fn dora(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScoreQuery>,
) -> Result<Json<Value>, ApiError> {
    let by = DoraBy::parse(q.by.as_deref()).map_err(bad_request)?;
    let scope = scope(&state, &principal, &q, "dora", None)?;
    document(&state, scope, move |_, _, loaded, matrix| {
        Ok(Ok(report::dora(loaded, matrix, by)))
    })
    .await
}

pub(crate) async fn article(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(key): Path<String>,
    Query(q): Query<ScoreQuery>,
) -> Result<Json<Value>, ApiError> {
    let scope = scope(&state, &principal, &q, "article", Some(&key))?;
    let entity = q.entity;
    document(&state, scope, move |_, _, loaded, matrix| {
        let doc = report::article(loaded, matrix, &key, entity.as_deref());
        Ok(doc.map_err(|e: ReportError| Refusal::Missing(e.to_string())))
    })
    .await
}

pub(crate) async fn maturity(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScoreQuery>,
) -> Result<Json<Value>, ApiError> {
    let by = MaturityBy::parse(q.by.as_deref()).map_err(bad_request)?;
    let scope = scope(&state, &principal, &q, "maturity", None)?;
    document(&state, scope, move |reader, _, loaded, matrix| {
        let assessment = report::assess(matrix);
        let history = report::history(reader, loaded, &assessment).map_err(|e| e.to_string())?;
        Ok(Ok(report::maturity(loaded, &assessment, by, history)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn manifest(name: &str, namespace: Option<&str>, app: Option<&str>) -> tumult_risk::Workload {
        tumult_risk::Workload {
            name: name.into(),
            namespace: namespace.map(str::to_owned),
            kind: "Deployment".into(),
            replicas: Some(1),
            labels: app
                .map(|a| BTreeMap::from([("app".to_string(), a.to_string())]))
                .unwrap_or_default(),
            containers: Vec::new(),
            source: "k8s/app.yaml".into(),
        }
    }

    #[test]
    fn a_workload_joins_its_service_by_the_app_label_then_by_name() {
        let labelled = workload(&manifest("api-worker", Some("shop"), Some("checkout")));
        assert_eq!(labelled.id, "wl:shop/deployment/api-worker");
        assert_eq!(labelled.service.as_deref(), Some("checkout"));
        let bare = workload(&manifest("batch-worker", None, None));
        assert_eq!(bare.id, "wl:default/deployment/batch-worker");
        assert_eq!(bare.service.as_deref(), Some("batch-worker"));
    }

    #[test]
    fn no_manifests_directory_means_no_workloads() {
        assert_eq!(read_manifests(None), Ok((Vec::new(), Vec::new())));
        let missing = read_manifests(Some(FsPath::new("/nonexistent/manifests")));
        assert!(missing.is_err());
    }

    fn query(lei: Option<&str>, entity: Option<&str>) -> ScoreQuery {
        ScoreQuery {
            lei: lei.map(str::to_owned),
            profile: None,
            by: None,
            env: None,
            entity: entity.map(str::to_owned),
        }
    }

    /// Two requests share a cached response only when the endpoint, the
    /// visible environments and every query field are the same.
    #[test]
    fn the_cache_key_separates_scopes_endpoints_and_query_fields() {
        let demo = vec!["demo".to_string()];
        let base = cache_key("dora", None, &[], &query(None, None)).unwrap();
        assert_eq!(
            cache_key("dora", None, &[], &query(None, None)).unwrap(),
            base
        );
        let others = [
            cache_key("maturity", None, &[], &query(None, None)),
            cache_key("dora", Some("24"), &[], &query(None, None)),
            cache_key("dora", None, &demo, &query(None, None)),
            cache_key("dora", None, &[], &query(Some("X"), None)),
            cache_key("dora", None, &[], &query(None, Some("X"))),
        ];
        let mut seen = vec![base];
        for key in others.into_iter().flatten() {
            assert!(!seen.contains(&key), "{key} is not its own entry");
            seen.push(key);
        }
        assert_eq!(seen.len(), 6);
        // A field cannot be made to read as its neighbour.
        assert_ne!(
            cache_key("dora", None, &[], &query(Some("a\",\"b"), None)),
            cache_key("dora", None, &[], &query(Some("a"), Some("b")))
        );
        let long = "x".repeat(CACHE_KEY_MAX_LEN);
        assert_eq!(
            cache_key("dora", None, &[], &query(Some(&long), None)),
            None
        );
    }
}
