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

use std::path::Path as FsPath;

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
    workloads: Vec<Workload>,
    params: HouseParams,
}

impl Scope {
    fn request(&self) -> Request<'_> {
        Request {
            envs: &self.envs,
            lei: self.lei.as_deref(),
            profile: self.profile.as_deref(),
            as_of_ns: self.as_of_ns,
            workloads: &self.workloads,
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

/// Workloads of the manifests directory; none when it is not configured.
fn workloads(dir: Option<&FsPath>) -> Result<Vec<Workload>, String> {
    let Some(dir) = dir else {
        return Ok(Vec::new());
    };
    let manifests = tumult_risk::manifests::load_dir(dir)
        .map_err(|e| format!("manifests at {}: {e}", dir.display()))?;
    for note in &manifests.notes {
        tracing::warn!(note, "manifest document skipped");
    }
    Ok(manifests.workloads.iter().map(workload).collect())
}

fn scope(state: &ApiState, principal: &Principal, q: &ScoreQuery) -> Result<Scope, ApiError> {
    let envs = envs_for(principal, q.env.clone())?;
    let workloads =
        workloads(state.manifests_dir().map(std::path::PathBuf::as_path)).map_err(internal)?;
    Ok(Scope {
        envs,
        lei: q.lei.clone(),
        profile: q.profile.clone(),
        as_of_ns: now_ns(),
        workloads,
        params: state.score_params().clone(),
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

/// Load the legal entity and build one document from its scored matrix.
async fn document(
    state: &ApiState,
    scope: Scope,
    build: impl FnOnce(&Reader, &Scope, &Loaded, &Matrix<'_>) -> Result<Result<Value, Refusal>, String>
        + Send
        + 'static,
) -> Result<Json<Value>, ApiError> {
    let outcome = with_reader(&state.db_path, move |reader| {
        let loaded = match load(reader, &scope)? {
            Ok(loaded) => loaded,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let matrix = Matrix::build(&loaded.inputs, &scope.params);
        build(reader, &scope, &loaded, &matrix)
    })
    .await?;
    Ok(Json(outcome?))
}

pub(crate) async fn dora(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScoreQuery>,
) -> Result<Json<Value>, ApiError> {
    let by = DoraBy::parse(q.by.as_deref()).map_err(bad_request)?;
    let scope = scope(&state, &principal, &q)?;
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
    let scope = scope(&state, &principal, &q)?;
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
    let scope = scope(&state, &principal, &q)?;
    document(&state, scope, move |reader, scope, loaded, matrix| {
        let assessment = report::assess(matrix);
        let history = report::history(reader, &scope.request(), &scope.params, &assessment.entity)
            .map_err(|e| e.to_string())?;
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
        assert_eq!(workloads(None), Ok(Vec::new()));
        let missing = workloads(Some(FsPath::new("/nonexistent/manifests")));
        assert!(missing.is_err());
    }
}
