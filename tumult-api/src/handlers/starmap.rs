//! `GET /api/starmap?range=&env=&focus=&depth=&kinds=&budget=&include=` —
//! the Star Map: organisation, ICT entities, systems, components,
//! services, workloads and the evidence against them as one graph.
//!
//! The handler only gathers rows: the service catalogue and call graph
//! from spans, runs and manual records in the window, the risk scan (which
//! brings coverage, posture and the manifests), the `ChaosGraph` tables
//! and `org.yaml`. [`tumult_graph::starmap::assemble`] turns them into the
//! response. Nothing is persisted.
//!
//! Scoped principals see only their environments: every query filters on
//! the environment, and the assembly drops what is left without one.
//!
//! Reading and assembling both happen on the blocking pool. The
//! serialised response is kept for 30 s under a key made of the
//! parameters and the principal's scopes, and carries an `ETag`; a
//! request with `Cache-Control: no-cache` reads the lake again unless
//! the kept response is younger than 2 s (see
//! `crate::handlers::starmap_cache` for the bounds).

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH, RETRY_AFTER as RETRY_AFTER_HEADER,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_compliance::dora::HouseParams;
use tumult_compliance::OrgTree;
use tumult_graph::starmap::{
    assemble, ArticleRow, CallRow, CoverFrom, CoverRow, DefinitionRow, DependsRow, ExperimentRow,
    GapRow, OrgUnitRow, PostureRow, RiskRow, RiskSubject, RunRow, ServiceRow, StarInputs, StarKind,
    StarMap, StarQuery, SuppressionRow, SystemRow, WorkloadRow, DEFAULT_BUDGET, MAX_BUDGET,
    MAX_DEPTH,
};
use tumult_lake::{scope, Reader, RunVerdict};
use tumult_risk::{CoverageStatus, Inputs, Report, Risk};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, ApiError};
use crate::handlers::starmap_cache::{
    CachedMap, Miss, StarMapCache, REFRESH_FLOOR, RETRY_AFTER, TTL,
};
use crate::handlers::{conformance, risks};
use crate::sql_util::{invalid_range, now_ns, parse_range, with_reader, PARENT_LOOKBACK_NS};
use crate::ApiState;

const DEFAULT_RANGE: &str = "7d";
/// The only value `include` accepts.
const INCLUDE_RUNS: &str = "runs";
/// Namespace a manifest without one deploys to.
const DEFAULT_NAMESPACE: &str = "default";
/// Most rows read from one `ChaosGraph` table. Those tables hold the
/// whole history, not a window, so the read is capped; a map built from
/// a capped read says `truncated`.
const GRAPH_ROW_LIMIT: usize = 50_000;

#[derive(Deserialize)]
pub(crate) struct StarMapParams {
    range: Option<String>,
    env: Option<String>,
    focus: Option<String>,
    depth: Option<u8>,
    kinds: Option<String>,
    budget: Option<usize>,
    include: Option<String>,
}

/// The time window and environments every query is confined to.
struct Window {
    from_ns: i64,
    to_ns: i64,
    /// Empty means every environment.
    envs: Vec<String>,
}

/// What a valid request asks for. It carries no time: the window is
/// taken when the lake is read, which can be well after the request
/// arrived when it waited for another build of the same map.
struct Plan {
    query: StarQuery,
    range_s: i64,
    /// Empty means every environment.
    envs: Vec<String>,
}

impl Plan {
    /// The window of this request when the lake is read at `to_ns`.
    fn window(&self, to_ns: i64) -> Window {
        Window {
            from_ns: to_ns - self.range_s * 1_000_000_000,
            to_ns,
            envs: self.envs.clone(),
        }
    }
}

fn parse_kinds(list: &str) -> Result<Vec<StarKind>, ApiError> {
    list.split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| {
            StarKind::parse(token).ok_or_else(|| bad_request(format!("unknown kind {token:?}")))
        })
        .collect()
}

fn check_depth(depth: Option<u8>) -> Result<Option<u8>, ApiError> {
    match depth {
        Some(d) if !(1..=MAX_DEPTH).contains(&d) => Err(bad_request(format!(
            "invalid depth {d}; expected 1 to {MAX_DEPTH}"
        ))),
        other => Ok(other),
    }
}

fn check_budget(budget: Option<usize>) -> Result<usize, ApiError> {
    match budget {
        None => Ok(DEFAULT_BUDGET),
        Some(b) if (1..=MAX_BUDGET).contains(&b) => Ok(b),
        Some(b) => Err(bad_request(format!(
            "invalid budget {b}; expected 1 to {MAX_BUDGET}"
        ))),
    }
}

fn check_include(include: Option<&str>) -> Result<bool, ApiError> {
    match include {
        None | Some("") => Ok(false),
        Some(INCLUDE_RUNS) => Ok(true),
        Some(other) => Err(bad_request(format!(
            "invalid include {other:?}; expected {INCLUDE_RUNS}"
        ))),
    }
}

/// Environments the request may read: the requested one when the
/// principal is allowed it, else the principal's own scopes.
fn envs_for(principal: &Principal, env: Option<&str>) -> Result<Vec<String>, ApiError> {
    let Some(env) = env else {
        return Ok(principal.env_scopes.clone());
    };
    if !principal.env_allowed(env) {
        return Err(forbidden(format!(
            "environment {env} is outside the principal's scopes"
        )));
    }
    Ok(vec![env.to_string()])
}

fn validate(params: StarMapParams, principal: &Principal) -> Result<Plan, ApiError> {
    let range = params.range.unwrap_or_else(|| DEFAULT_RANGE.to_string());
    let Some(range_s) = parse_range(&range) else {
        return Err(bad_request(invalid_range(&range)));
    };
    let envs = envs_for(principal, params.env.as_deref())?;
    let focus = params.focus.filter(|f| !f.is_empty());
    let depth = check_depth(params.depth)?.filter(|_| focus.is_some());
    let kinds = params.kinds.as_deref().map(parse_kinds).transpose()?;
    let query = StarQuery {
        range,
        env: params.env,
        scope: (!envs.is_empty()).then(|| envs.clone()),
        focus,
        depth,
        kinds: kinds.filter(|k| !k.is_empty()),
        budget: check_budget(params.budget)?,
        include_runs: check_include(params.include.as_deref())?,
    };
    Ok(Plan {
        query,
        range_s,
        envs,
    })
}

fn org_units(org: &OrgTree) -> Vec<OrgUnitRow> {
    org.unit_ids()
        .map(|id| OrgUnitRow {
            path: org.path_of(id),
            name: org.node(id).name.clone(),
            kind: org.node(id).kind.clone(),
        })
        .collect()
}

/// Each name with the path of the unit `owner` gives it; names nobody
/// claims are left out. `owner` is always a method of [`OrgTree`], the
/// one place an `org.yaml` pattern is matched.
fn owners(
    org: &OrgTree,
    names: BTreeSet<String>,
    owner: impl Fn(&str) -> Option<usize>,
) -> BTreeMap<String, String> {
    names
        .into_iter()
        .filter_map(|name| {
            let unit = owner(&name)?;
            Some((name, org.path_of(unit)))
        })
        .collect()
}

/// A non-empty string column.
fn text(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn count(row: &Value, key: &str) -> u64 {
    row.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn rows(reader: &Reader, sql: &str) -> Result<Vec<Value>, String> {
    reader.query_json_rows(sql).map_err(|e| e.to_string())
}

/// Rows with whether more were left unread.
type Capped<T> = (Vec<T>, bool);

/// The row limit of each capped source.
#[derive(Clone, Copy)]
struct Caps {
    covers: usize,
    articles: usize,
    depends: usize,
    gaps: usize,
}

impl Caps {
    const fn all(limit: usize) -> Self {
        Self {
            covers: limit,
            articles: limit,
            depends: limit,
            gaps: limit,
        }
    }
}

/// At most `limit` rows of `sql`, in a stable order so that a capped
/// read returns the same rows every time. `source` names what is read,
/// as `table.kind`, in the log and in the error. The statement is never
/// logged as such; the store's own message for a failed read
/// (`error.message`) may quote the part of it that failed.
fn capped_rows(
    reader: &Reader,
    source: &str,
    sql: &str,
    limit: usize,
) -> Result<Capped<Value>, String> {
    let probe = limit.saturating_add(1);
    let mut found =
        rows(reader, &format!("{sql} ORDER BY ALL LIMIT {probe}")).map_err(|error| {
            tracing::warn!(source, error.message = %error, "star map source read failed");
            format!("star map source {source}: {error}")
        })?;
    let cut = found.len() > limit;
    if cut {
        tracing::warn!(
            source,
            limit,
            "star map source read stopped at its row limit"
        );
    }
    found.truncate(limit);
    Ok((found, cut))
}

/// Q1, the service catalogue: one row per distinct resource-attribute
/// tuple. Experiment root spans are left out because their target columns
/// name the system under test, not the system the runner belongs to.
fn services_sql(window: &Window) -> String {
    let attr = |key: &str| format!("NULLIF(resource_attrs['{key}'], '')");
    let scope = scope::and(scope::in_list("target_environment", &window.envs));
    format!(
        "SELECT service_name AS service, {} AS entity_id, {} AS entity_tier, \
         {} AS entity_owner, NULLIF(target_system, '') AS system, {} AS component, \
         NULLIF(target_environment, '') AS environment, {} AS k8s_namespace, \
         {} AS k8s_deployment, MAX(service_version) AS version, COUNT(*) AS spans, \
         MAX(ts_ns) AS last_seen_ns \
         FROM spans WHERE ts_ns >= {} AND ts_ns < {} AND service_name <> '' \
         AND span_name <> 'resilience.experiment'{scope} \
         GROUP BY 1, 2, 3, 4, 5, 6, 7, 8, 9",
        attr("ict.entity.id"),
        attr("ict.entity.tier"),
        attr("ict.entity.owner"),
        attr("resilience.target.component"),
        attr("k8s.namespace.name"),
        attr("k8s.deployment.name"),
        window.from_ns,
        window.to_ns,
    )
}

fn services(
    reader: &Reader,
    window: &Window,
    levels: &BTreeMap<String, &'static str>,
) -> Result<Vec<ServiceRow>, String> {
    let found = rows(reader, &services_sql(window))?;
    Ok(found
        .iter()
        .filter_map(|row| {
            let service = text(row, "service")?;
            Some(ServiceRow {
                conformance: levels.get(&service).map(|level| (*level).to_string()),
                service,
                entity_id: text(row, "entity_id"),
                entity_tier: text(row, "entity_tier"),
                entity_owner: text(row, "entity_owner"),
                system: text(row, "system"),
                component: text(row, "component"),
                environment: text(row, "environment"),
                k8s_namespace: text(row, "k8s_namespace"),
                k8s_deployment: text(row, "k8s_deployment"),
                version: text(row, "version"),
                spans: count(row, "spans"),
                last_seen_ns: row.get("last_seen_ns").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect())
}

/// Q2, observed calls: a child span whose parent belongs to another
/// service. Scoped requests need both ends in an allowed environment.
/// Both sides of the join are bounded in time: the child by the window,
/// the parent by the window plus [`PARENT_LOOKBACK_NS`] before it, so a
/// call whose parent started more than an hour before the window opened
/// is not counted.
fn calls_sql(window: &Window) -> String {
    let child = scope::and(scope::in_list("c.target_environment", &window.envs));
    let parent = scope::and(scope::in_list("p.target_environment", &window.envs));
    format!(
        "SELECT p.service_name AS client, c.service_name AS server, COUNT(*) AS calls, \
         COUNT(*) FILTER (WHERE c.status_code = 'Error') AS errors, \
         AVG(c.duration_ns) AS avg_duration_ns \
         FROM spans c JOIN spans p ON c.parent_span_id = p.span_id \
         WHERE p.service_name <> c.service_name AND p.service_name <> '' \
         AND c.service_name <> '' AND c.ts_ns >= {from} AND c.ts_ns < {to} \
         AND p.ts_ns >= {parent_from} AND p.ts_ns < {to}{child}{parent} \
         GROUP BY 1, 2",
        from = window.from_ns,
        to = window.to_ns,
        parent_from = window.from_ns - PARENT_LOOKBACK_NS,
    )
}

fn calls(reader: &Reader, window: &Window) -> Result<Vec<CallRow>, String> {
    let found = rows(reader, &calls_sql(window))?;
    Ok(found
        .iter()
        .filter_map(|row| {
            Some(CallRow {
                client: text(row, "client")?,
                server: text(row, "server")?,
                calls: count(row, "calls"),
                errors: count(row, "errors"),
                avg_duration_ns: row
                    .get("avg_duration_ns")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            })
        })
        .collect())
}

/// Runs and verified manual records that started inside the window. A run
/// is read from its root, which is built from the journal when the run
/// sent no trace. The outcome of a run is on its root or its completion
/// log; a run completes after it starts, so only logs since the window
/// opened are read.
fn runs_sql(window: &Window) -> String {
    let span_scope = scope::and(scope::in_list("s.target_environment", &window.envs));
    let manual_scope = scope::and(scope::in_list("m.target_environment", &window.envs));
    let (from, to) = (window.from_ns, window.to_ns);
    let status = tumult_lake::run_outcome_sql("s", "l");
    let completions = tumult_lake::run_completions_sql(Some(from));
    format!(
        "SELECT s.experiment_id AS id, any_value(s.experiment_name) AS experiment, \
         any_value(s.target_system) AS system, any_value(s.target_environment) AS environment, \
         MIN(s.ts_ns) AS started_ns, \
         any_value({status}) AS status, \
         false AS manual \
         FROM run_roots s LEFT JOIN ({completions}) l ON s.experiment_id = l.experiment_id \
         WHERE s.experiment_id IS NOT NULL \
           AND s.experiment_name IS NOT NULL \
           AND s.ts_ns >= {from} AND s.ts_ns < {to}{span_scope} \
         GROUP BY s.experiment_id \
         UNION ALL \
         SELECT m.id, m.experiment_name, m.target_system, m.target_environment, \
         m.executed_at_ns, m.outcome_status, true \
         FROM manual_experiments m \
         WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' \
           AND m.executed_at_ns >= {from} AND m.executed_at_ns < {to}{manual_scope}"
    )
}

/// The scorecard's rule: a run fails by its verdict
/// ([`tumult_lake::run_verdict`]), a manual record unless it passed or
/// partly passed, and so does a run that never reported an outcome.
/// `None` for a run that proves nothing: it is no evidence at all.
fn failed(status: Option<&str>) -> Option<bool> {
    let status = status.map(str::to_ascii_lowercase);
    if matches!(status.as_deref(), Some("passed" | "partial")) {
        return Some(false);
    }
    match status.as_deref().and_then(tumult_lake::run_verdict) {
        Some(RunVerdict::NoEvidence) => None,
        verdict => Some(verdict != Some(RunVerdict::Pass)),
    }
}

fn runs(reader: &Reader, window: &Window) -> Result<Vec<RunRow>, String> {
    let found = rows(reader, &runs_sql(window))?;
    Ok(found
        .iter()
        .filter_map(|row| {
            Some(RunRow {
                id: text(row, "id")?,
                experiment: text(row, "experiment")?,
                system: text(row, "system"),
                environment: text(row, "environment"),
                started_ns: row.get("started_ns").and_then(Value::as_i64)?,
                failed: failed(row.get("status").and_then(Value::as_str))?,
                manual: row.get("manual").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

fn status_token(status: CoverageStatus) -> &'static str {
    match status {
        CoverageStatus::Covered => "covered",
        CoverageStatus::Stale => "stale",
        CoverageStatus::Missing => "missing",
    }
}

/// One row per tested system: its latest experiments, the coverage row
/// and the posture, all from the risk scan.
fn systems(inputs: &Inputs, report: &Report) -> Vec<SystemRow> {
    let posture = tumult_risk::posture::compute(inputs);
    inputs
        .targets
        .iter()
        .map(|target| {
            let same = |name: &str, env: &Option<String>| {
                name == target.target && *env == target.environment
            };
            SystemRow {
                system: target.target.clone(),
                environment: target.environment.clone(),
                criticality: target.criticality.clone(),
                posture: posture
                    .targets
                    .iter()
                    .find(|p| same(&p.target, &p.environment))
                    .map(|p| PostureRow {
                        score: p.score,
                        band: p.band.clone(),
                    }),
                disciplines: report
                    .coverage
                    .targets
                    .iter()
                    .filter(|c| same(&c.target, &c.environment))
                    .flat_map(|c| &c.cells)
                    .map(|cell| (cell.kind.clone(), status_token(cell.status).to_string()))
                    .collect(),
                experiments: target
                    .experiments
                    .iter()
                    .map(|e| ExperimentRow {
                        name: e.name.clone(),
                        kind: e.kind.clone(),
                        passed: e.passed,
                        last_run_ns: e.last_run_ns,
                    })
                    .collect(),
            }
        })
        .collect()
}

fn namespace_of(workload: &tumult_risk::Workload) -> String {
    workload
        .namespace
        .clone()
        .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string())
}

fn workloads(inputs: &Inputs) -> Vec<WorkloadRow> {
    inputs
        .workloads
        .iter()
        .map(|w| WorkloadRow {
            name: w.name.clone(),
            namespace: namespace_of(w),
            kind: w.kind.clone(),
            labels: w.labels.clone(),
        })
        .collect()
}

/// Risk subjects the Star Map has a node for: a workload or a registered
/// definition (by id). Every other subject names a discipline or an
/// objective, and the risk then belongs to its target.
fn subjects(inputs: &Inputs) -> BTreeMap<String, RiskSubject> {
    let workloads = inputs.workloads.iter().map(|w| {
        let subject = RiskSubject::Workload {
            namespace: namespace_of(w),
            name: w.name.clone(),
        };
        (w.qualified_name(), subject)
    });
    let definitions = inputs
        .definitions
        .iter()
        .map(|d| (d.id.clone(), RiskSubject::Definition(d.id.clone())));
    workloads.chain(definitions).collect()
}

fn risk_row(risk: &Risk, subjects: &BTreeMap<String, RiskSubject>) -> RiskRow {
    RiskRow {
        target: risk.target.clone(),
        environment: risk.environment.clone(),
        subject: subjects.get(&risk.subject).cloned(),
        severity: risk.severity.as_str().to_string(),
        recommended_kind: risk.recommended_kind.clone(),
        suppression: risk.suppression.as_ref().map(|s| SuppressionRow {
            actor: s.actor.clone(),
            reason: s.reason.clone(),
            expires_at_ns: s.expires_at_ns,
        }),
    }
}

fn risk_rows(inputs: &Inputs, report: &Report) -> Vec<RiskRow> {
    let subjects = subjects(inputs);
    report
        .risks
        .iter()
        .chain(&report.suppressed)
        .map(|risk| risk_row(risk, &subjects))
        .collect()
}

fn definitions(inputs: &Inputs) -> Vec<DefinitionRow> {
    inputs
        .definitions
        .iter()
        .map(|d| DefinitionRow {
            id: Some(d.id.clone()),
            name: d.name.clone(),
        })
        .collect()
}

/// The `ChaosGraph` article id a hand-written framework reference names,
/// for example `DORA Art. 25(1)` to `compliance:DORA/Art.25`. A DORA
/// reference is read as its registry id first (`dora.art25.1` names the
/// same article), as the score reads it. A paragraph suffix falls back to
/// its article. `None` when the registry has no such citation.
fn manual_article(reference: &str) -> Option<String> {
    if let Some(id) = tumult_core::compliance::requirement_id(reference) {
        return tumult_core::compliance::citation_of(&id)
            .map(|c| tumult_graph::compliance_article_id(c.framework, c.control_id));
    }
    let reference = reference.trim();
    reference
        .match_indices(char::is_whitespace)
        .find_map(|(at, _)| {
            let (framework, control) = reference.split_at(at);
            let whole = control.trim();
            let article = whole.split('(').next().unwrap_or(whole);
            [whole, article]
                .into_iter()
                .find_map(|c| tumult_graph::resolve_citation(framework, c))
        })
        .map(|c| tumult_graph::compliance_article_id(c.framework, c.control_id))
}

/// Evidence toward compliance articles: `ChaosGraph` edges from
/// experiments, and the framework references on manual records.
fn covers(reader: &Reader, window: &Window, limit: usize) -> Result<Capped<CoverRow>, String> {
    let (edges, cut) = capped_rows(
        reader,
        "graph_edges.evidence",
        "SELECT DISTINCT src, rel, dst FROM graph_edges \
         WHERE rel IN ('evidences', 'maps_to_compliance')",
        limit,
    )?;
    let mut out: Vec<CoverRow> = edges
        .iter()
        .filter_map(|row| {
            let src = text(row, "src")?;
            Some(CoverRow {
                from: CoverFrom::Experiment(src.strip_prefix("exp:")?.to_string()),
                article: text(row, "dst")?,
                evidenced: text(row, "rel")? == "evidences",
            })
        })
        .collect();
    let scope = scope::and(scope::in_list("m.target_environment", &window.envs));
    let manual = rows(
        reader,
        &format!(
            "SELECT m.id AS id, unnest(m.framework_refs) AS reference \
             FROM manual_experiments m \
             WHERE m.status = 'verified' AND m.executed_at_ns >= {} \
             AND m.executed_at_ns < {}{scope}",
            window.from_ns, window.to_ns
        ),
    )?;
    out.extend(manual.iter().filter_map(|row| {
        Some(CoverRow {
            from: CoverFrom::Manual(text(row, "id")?),
            article: manual_article(&text(row, "reference")?)?,
            evidenced: true,
        })
    }));
    Ok((out, cut))
}

/// `(id, label)` of the graph nodes of one kind.
fn graph_nodes(
    reader: &Reader,
    kind: &str,
    limit: usize,
) -> Result<Capped<(String, String)>, String> {
    let (found, cut) = capped_rows(
        reader,
        &format!("graph_nodes.{kind}"),
        &format!("SELECT id, label FROM graph_nodes WHERE kind = '{kind}'"),
        limit,
    )?;
    let nodes = found
        .iter()
        .filter_map(|row| {
            let id = text(row, "id")?;
            let label = text(row, "label").unwrap_or_else(|| id.clone());
            Some((id, label))
        })
        .collect();
    Ok((nodes, cut))
}

fn articles(reader: &Reader, limit: usize) -> Result<Capped<ArticleRow>, String> {
    let (nodes, cut) = graph_nodes(reader, "compliance_article", limit)?;
    let rows = nodes
        .into_iter()
        .map(|(id, label)| ArticleRow { id, label })
        .collect();
    Ok((rows, cut))
}

fn gaps(reader: &Reader, limit: usize) -> Result<Capped<GapRow>, String> {
    let (nodes, cut) = graph_nodes(reader, "coverage_gap", limit)?;
    let rows = nodes
        .into_iter()
        .map(|(id, label)| GapRow { id, label })
        .collect();
    Ok((rows, cut))
}

/// Declared service dependencies from a loaded topology file.
fn depends(reader: &Reader, limit: usize) -> Result<Capped<DependsRow>, String> {
    let (found, cut) = capped_rows(
        reader,
        "graph_edges.depends_on",
        "SELECT DISTINCT src, dst FROM graph_edges WHERE rel = 'depends_on'",
        limit,
    )?;
    let edges = found
        .iter()
        .filter_map(|row| {
            Some(DependsRow {
                from: text(row, "src")?.strip_prefix("svc:")?.to_string(),
                to: text(row, "dst")?.strip_prefix("svc:")?.to_string(),
            })
        })
        .collect();
    Ok((edges, cut))
}

fn load(
    reader: &Reader,
    window: &Window,
    manifests_dir: Option<&FsPath>,
    org: &OrgTree,
    caps: Caps,
    params: &HouseParams,
) -> Result<StarInputs, String> {
    let risk_inputs = risks::gather(reader, window.to_ns, &window.envs, manifests_dir, params)?;
    let report = tumult_risk::scan(&risk_inputs);
    let levels = conformance::levels(reader, window.from_ns, &window.envs)?;
    let (covers, cut_covers) = covers(reader, window, caps.covers)?;
    let (articles, cut_articles) = articles(reader, caps.articles)?;
    let (depends, cut_depends) = depends(reader, caps.depends)?;
    let (gaps, cut_gaps) = gaps(reader, caps.gaps)?;
    let mut inputs = StarInputs {
        as_of_ns: window.to_ns,
        org_units: org_units(org),
        services: services(reader, window, &levels)?,
        calls: calls(reader, window)?,
        systems: systems(&risk_inputs, &report),
        workloads: workloads(&risk_inputs),
        risks: risk_rows(&risk_inputs, &report),
        definitions: definitions(&risk_inputs),
        runs: runs(reader, window)?,
        covers,
        articles,
        depends,
        gaps,
        truncated: cut_covers || cut_articles || cut_depends || cut_gaps,
        ..StarInputs::default()
    };
    inputs.entity_owners = owners(org, inputs.entity_ids(), |id| org.assign_entity(id));
    inputs.experiment_owners = owners(org, inputs.experiment_names(), |name| {
        org.owner_of_experiment(name)
    });
    Ok(inputs)
}

/// Assemble inside a `starmap.assemble` span that records what was asked
/// for and how large the answer is. `envs` is the number of environments
/// the request is confined to, `0` for all of them.
fn assemble_traced(
    parent: &tracing::Span,
    inputs: &StarInputs,
    query: &StarQuery,
    envs: usize,
) -> StarMap {
    let span = tracing::info_span!(
        parent: parent,
        "starmap.assemble",
        range = %query.range,
        envs,
        nodes = tracing::field::Empty,
        edges = tracing::field::Empty,
        clustered = tracing::field::Empty,
    );
    let _entered = span.enter();
    let map = assemble(inputs, query);
    span.record("nodes", map.counts.nodes);
    span.record("edges", map.counts.edges);
    span.record("clustered", map.counts.clustered);
    map
}

/// The cache key: every parameter that shapes the response, and the
/// principal's scopes, never only the `env` parameter. JSON keeps the
/// parts apart whatever characters they carry.
fn cache_key(query: &StarQuery, scopes: &[String]) -> String {
    let mut scopes = scopes.to_vec();
    scopes.sort();
    scopes.dedup();
    let mut kinds: Vec<&str> = query
        .kinds
        .iter()
        .flatten()
        .map(|kind| kind.as_str())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    json!({
        "range": query.range,
        "scopes": scopes,
        "env": query.env,
        "focus": query.focus,
        "depth": query.depth,
        "kinds": query.kinds.as_ref().map(|_| kinds),
        "budget": query.budget,
        "runs": query.include_runs,
    })
    .to_string()
}

/// Whether the request asks for a response read from the lake now.
fn wants_fresh(headers: &HeaderMap) -> bool {
    headers
        .get_all(CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|directive| directive.trim().eq_ignore_ascii_case("no-cache"))
}

/// The oldest stored response this request accepts. `no-cache` is
/// honoured down to [`REFRESH_FLOOR`] and no further, so no viewer can
/// make every request read the lake.
fn max_age(headers: &HeaderMap) -> Duration {
    if wants_fresh(headers) {
        return REFRESH_FLOOR;
    }
    TTL
}

/// Whether `If-None-Match` names `etag`. The comparison is weak, as the
/// header asks: `W/"x"` matches `"x"`, and `*` matches anything.
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}

/// The response for a serialised map: `304` when the caller already
/// holds it. `private` because the body depends on who asked, `no-cache`
/// so a browser revalidates with the `ETag` instead of trusting a copy.
fn respond(map: &CachedMap, headers: &HeaderMap) -> Response {
    let validators = [
        (ETAG, map.etag.clone()),
        (CACHE_CONTROL, "private, no-cache".to_string()),
    ];
    if not_modified(headers, &map.etag) {
        return (StatusCode::NOT_MODIFIED, validators).into_response();
    }
    let json = [(CONTENT_TYPE, "application/json")];
    (validators, json, map.body.clone()).into_response()
}

/// Read, assemble and serialise on the blocking pool.
async fn build(state: &ApiState, query: StarQuery, window: Window) -> Result<CachedMap, ApiError> {
    let org = Arc::clone(&state.org);
    let manifests_dir: Option<PathBuf> = state.manifests_dir().cloned();
    let params = state.score_runtime().params();
    let parent = tracing::Span::current();
    with_reader(&state.db_path, move |reader| {
        let caps = Caps::all(GRAPH_ROW_LIMIT);
        let inputs = load(
            reader,
            &window,
            manifests_dir.as_deref(),
            &org,
            caps,
            &params,
        )?;
        let map = assemble_traced(&parent, &inputs, &query, window.envs.len());
        serde_json::to_vec(&map)
            .map(CachedMap::new)
            .map_err(|e| e.to_string())
    })
    .await
}

/// The map `plan` asks for: the stored one, or the one `read` returns
/// for the window as it is when the read starts.
async fn cached<F, Fut>(
    cache: &StarMapCache,
    key: &str,
    max_age: Duration,
    plan: Plan,
    read: F,
) -> Result<Arc<CachedMap>, ApiError>
where
    F: FnOnce(StarQuery, Window) -> Fut,
    Fut: Future<Output = Result<CachedMap, ApiError>>,
{
    // The cache calls this when the request's turn to read has come.
    let build = || {
        let window = plan.window(now_ns());
        read(plan.query, window)
    };
    let answered = cache.get_or_build(key, max_age, build).await;
    answered.map_err(|miss| match miss {
        Miss::Busy => busy(),
        Miss::Failed(error) => error,
    })
}

/// `503` with `Retry-After`: more requests wait for the lake than the
/// cache lets wait, or this one waited as long as one may.
fn busy() -> ApiError {
    let retry = [(RETRY_AFTER_HEADER, RETRY_AFTER.as_secs().to_string())];
    let error = json!({"error": "too many star map requests are waiting; try again shortly"});
    (StatusCode::SERVICE_UNAVAILABLE, retry, axum::Json(error))
        .into_response()
        .into()
}

pub(crate) async fn starmap(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<StarMapParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let plan = validate(params, &principal)?;
    let key = cache_key(&plan.query, &principal.env_scopes);
    let max_age = max_age(&headers);
    let read = |query, window| build(&state, query, window);
    let map = cached(&state.starmap_cache, &key, max_age, plan, read).await?;
    Ok(respond(&map, &headers))
}

#[cfg(test)]
mod tests {
    use tumult_otel::logging::testing::{capture_closed_spans, capture_json};

    use super::*;

    fn plan() -> Plan {
        Plan {
            query: StarQuery::default(),
            range_s: 3_600,
            envs: vec![],
        }
    }

    #[tokio::test]
    async fn a_waiter_reads_a_window_that_ends_after_the_write_it_waited_through() {
        let cache = StarMapCache::default();
        let release = tokio::sync::Notify::new();
        // The leader reads the lake, slowly, and then fails.
        let leader = cached(&cache, "k", TTL, plan(), |_, _| async {
            release.notified().await;
            Err(crate::error::unavailable("lake gone"))
        });
        // The waiter arrives while the leader reads.
        let waiter = cached(&cache, "k", TTL, plan(), |_, window: Window| async move {
            Ok(CachedMap::new(window.to_ns.to_string().into_bytes()))
        });
        tokio::pin!(leader, waiter);
        assert!(futures_poll(&mut leader).await.is_none());
        assert!(futures_poll(&mut waiter).await.is_none(), "it queues");

        // A write lands, then the leader gives up and the waiter reads.
        // sleep: the clock moves past the window the leader read before the write
        std::thread::sleep(Duration::from_millis(2));
        cache.clear();
        let cleared_ns = now_ns();
        release.notify_one();
        assert!(leader.await.is_err());
        let Ok(map) = waiter.await else {
            panic!("the waiter's read failed");
        };
        let read_to_ns: i64 = String::from_utf8_lossy(&map.body).parse().expect("to_ns");
        assert!(
            read_to_ns >= cleared_ns,
            "the window ends {} ns before the write",
            cleared_ns - read_to_ns
        );
    }

    #[tokio::test]
    async fn a_request_the_cache_refuses_is_told_when_to_come_back() {
        let cache = StarMapCache::default();
        let mut held = Vec::new();
        // More requests than the cache lets wait, each for its own map
        // and each stuck in its read.
        for i in 0..80 {
            let key = format!("k{i}");
            let cache = &cache;
            let mut ask = Box::pin(async move {
                let stuck = |_, _| std::future::pending::<Result<CachedMap, ApiError>>();
                cached(cache, &key, TTL, plan(), stuck).await
            });
            if let Some(answer) = futures_poll(&mut ask).await {
                let Err(refused) = answer else {
                    panic!("a stuck read answered");
                };
                let response = refused.into_response();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(response.headers()[RETRY_AFTER_HEADER], "2");
                return;
            }
            held.push(ask);
        }
        panic!("eighty stuck requests and none was refused");
    }

    /// Poll `future` once: its output when it is ready already.
    async fn futures_poll<F: Future + Unpin>(future: &mut F) -> Option<F::Output> {
        tokio::select! {
            biased;
            output = future => Some(output),
            () = std::future::ready(()) => None,
        }
    }

    #[test]
    fn outcome_rule_matches_the_scorecard() {
        for passed in ["Completed", "completed", "passed", "partial"] {
            assert_eq!(failed(Some(passed)), Some(false), "{passed}");
        }
        for broke in ["Deviated", "Failed", "failed", "Halted", ""] {
            assert_eq!(failed(Some(broke)), Some(true), "{broke}");
        }
        for nothing in ["Aborted", "interrupted"] {
            assert_eq!(failed(Some(nothing)), None, "{nothing} is no evidence");
        }
        assert_eq!(
            failed(None),
            Some(true),
            "a run with no outcome did not pass"
        );
    }

    #[test]
    fn graph_table_reads_are_capped_and_say_so() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).expect("store");
        let reader = store.read_only().expect("reader");
        // A new lake carries the compliance registry as graph nodes.
        let (all, cut) = graph_nodes(&reader, "compliance_article", GRAPH_ROW_LIMIT).expect("read");
        assert!(all.len() > 3, "{}", all.len());
        assert!(!cut);

        let (some, cut) = graph_nodes(&reader, "compliance_article", 3).expect("read");
        assert!(cut, "rows were left out");
        assert_eq!(some, all[..3], "the same rows first, whatever the cap");

        let (exact, cut) = graph_nodes(&reader, "compliance_article", all.len()).expect("read");
        assert_eq!(exact.len(), all.len());
        assert!(!cut, "a table that fits exactly is whole");

        let (edges, cut) = depends(&reader, 3).expect("read");
        assert!(edges.is_empty() && !cut);
    }

    /// A lake whose graph holds two rows of every capped source, on
    /// top of the compliance registry a new lake carries.
    fn graph_lake() -> (tempfile::TempDir, tumult_lake::Store) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).expect("store");
        let writer = store.writer().expect("writer");
        let seed = "INSERT INTO graph_edges (src, rel, dst, run_id, ts) VALUES \
             ('exp:a', 'evidences', 'compliance:DORA/Art.25', 'r1', 1), \
             ('exp:b', 'evidences', 'compliance:DORA/Art.25', 'r2', 1), \
             ('svc:a', 'depends_on', 'svc:b', 'topology', 1), \
             ('svc:b', 'depends_on', 'svc:c', 'topology', 1); \
             INSERT INTO graph_nodes (id, kind, label) VALUES \
             ('gap:one', 'coverage_gap', 'one'), ('gap:two', 'coverage_gap', 'two');";
        for statement in seed.split(';').filter(|s| !s.trim().is_empty()) {
            writer.execute(statement, ()).expect("seed");
        }
        drop(writer);
        (dir, store)
    }

    fn any_time() -> Window {
        Window {
            from_ns: 0,
            to_ns: now_ns(),
            envs: vec![],
        }
    }

    /// A run of the drill mapped to DORA Art. 25, as its journal says.
    fn drill_run(id: &str, status: &str, started_at_ns: i64) -> tumult_core::types::Journal {
        serde_json::from_value(serde_json::json!({
            "experiment_title": "Purge drill",
            "experiment_id": id,
            "status": status,
            "started_at_ns": started_at_ns,
            "ended_at_ns": started_at_ns + 1,
            "duration_ms": 1,
            "method_results": [],
            "rollback_results": [],
        }))
        .expect("journal")
    }

    #[test]
    fn a_purged_run_is_no_longer_evidence() {
        use tumult_core::types::{Experiment, RegulatoryMapping, RegulatoryRequirement};
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("k.duckdb");
        let drill = Experiment {
            title: "Purge drill".into(),
            regulatory: Some(RegulatoryMapping {
                frameworks: vec!["DORA".into()],
                requirements: vec![RegulatoryRequirement {
                    id: "Art. 25".into(),
                    description: "Testing of ICT tools and systems".into(),
                    evidence: "scenario-based fault injection".into(),
                }],
                tester: None,
            }),
            ..Experiment::default()
        };
        // The old run completed and evidences the article; the new one
        // deviated, so it only maps the drill to it.
        let store = tumult_lake::AnalyticsStore::open(&path).expect("store");
        for journal in [
            drill_run("old-1", "completed", 1_577_836_800_000_000_000),
            drill_run("new-1", "deviated", now_ns()),
        ] {
            store
                .ingest_journal_with_experiment(&journal, Some(&drill))
                .expect("ingest");
        }
        assert_eq!(store.purge_older_than_days(30).expect("purge"), 1);
        drop(store);

        let lake = tumult_lake::Store::open(&path).expect("lake");
        let reader = lake.read_only().expect("reader");
        let (rows, cut) = covers(&reader, &any_time(), GRAPH_ROW_LIMIT).expect("read");
        assert!(!cut);
        let drill_covers: Vec<(&str, bool)> = rows
            .iter()
            .filter(|row| row.from == CoverFrom::Experiment("Purge drill".into()))
            .map(|row| (row.article.as_str(), row.evidenced))
            .collect();
        assert_eq!(drill_covers, [("compliance:DORA/Art.25", false)]);
    }

    #[test]
    fn any_one_capped_source_marks_the_map_truncated() {
        let (_dir, store) = graph_lake();
        let reader = store.read_only().expect("reader");
        let org = OrgTree::empty();
        let read = |caps: Caps| {
            let params = HouseParams::default();
            let inputs = load(&reader, &any_time(), None, &org, caps, &params).expect("load");
            (
                inputs.truncated,
                assemble(&inputs, &StarQuery::default()).truncated,
            )
        };
        let whole = Caps::all(GRAPH_ROW_LIMIT);
        assert_eq!(read(whole), (false, false), "nothing was cut");

        let cut = [
            ("covers", Caps { covers: 1, ..whole }),
            (
                "articles",
                Caps {
                    articles: 1,
                    ..whole
                },
            ),
            (
                "depends",
                Caps {
                    depends: 1,
                    ..whole
                },
            ),
            ("gaps", Caps { gaps: 1, ..whole }),
        ];
        for (source, caps) in cut {
            assert_eq!(read(caps), (true, true), "only {source} was cut");
        }
    }

    #[test]
    fn a_capped_read_is_logged_by_its_source_and_never_by_its_sql() {
        let (_dir, store) = graph_lake();
        let reader = store.read_only().expect("reader");
        let mut cut = false;
        let logs = capture_json(tracing::Level::INFO, || {
            (_, cut) = depends(&reader, 1).expect("read");
        });
        assert!(cut);

        let [event] = logs.events.as_slice() else {
            panic!("one event, not {}", logs.raw);
        };
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["source"], "graph_edges.depends_on");
        assert_eq!(event["limit"], 1);
        assert!(!logs.raw.contains("SELECT"), "{}", logs.raw);
    }

    #[test]
    fn a_failed_source_read_names_the_source() {
        let (_dir, store) = graph_lake();
        let reader = store.read_only().expect("reader");
        let mut failed = None;
        let logs = capture_json(tracing::Level::INFO, || {
            failed = capped_rows(&reader, "graph_edges.nothing", "SELECT nothing FROM", 1).err();
        });
        let message = failed.expect("the statement is not SQL");
        assert!(
            message.starts_with("star map source graph_edges.nothing: "),
            "{message}"
        );

        let [event] = logs.events.as_slice() else {
            panic!("one event, not {}", logs.raw);
        };
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["source"], "graph_edges.nothing");
        let error = event["error.message"].as_str().expect("error.message");
        assert!(!error.is_empty());
        assert!(event.get("sql").is_none(), "{event}");
    }

    #[test]
    fn manual_references_resolve_to_registry_articles() {
        assert_eq!(
            manual_article("DORA Art. 25").as_deref(),
            Some("compliance:DORA/Art.25")
        );
        assert_eq!(
            manual_article("DORA Art. 25(1)").as_deref(),
            Some("compliance:DORA/Art.25"),
            "a paragraph falls back to its article"
        );
        assert_eq!(
            manual_article("dora art25").as_deref(),
            Some("compliance:DORA/Art.25")
        );
        assert_eq!(manual_article("DORA"), None);
        assert_eq!(manual_article("Company policy 7"), None);
    }

    #[test]
    fn a_registry_id_falls_under_the_article_the_old_form_of_it_does() {
        for (registry, old, article) in [
            ("dora.art25.1", "DORA Art. 25(1)", "compliance:DORA/Art.25"),
            (
                "dora.art24.6",
                "DORA Art. 24(6)",
                "compliance:DORA/Art.24(6)",
            ),
            ("dora.art12.1", "DORA Art. 12(1)", "compliance:DORA/Art.12"),
            ("dora.art11", "DORA-Art11", "compliance:DORA/Art.11"),
        ] {
            assert_eq!(
                manual_article(registry).as_deref(),
                Some(article),
                "{registry}"
            );
            assert_eq!(manual_article(old).as_deref(), Some(article), "{old}");
        }
        assert_eq!(
            manual_article("NIS2 Art. 21(2)(c)").as_deref(),
            Some("compliance:NIS2/Art.21(2)(c)")
        );
        assert_eq!(manual_article("dora.art5.2"), None);
    }

    #[test]
    fn parameters_are_bounded() {
        assert!(check_depth(Some(0)).is_err());
        assert!(check_depth(Some(MAX_DEPTH + 1)).is_err());
        assert_eq!(check_depth(Some(MAX_DEPTH)).ok(), Some(Some(MAX_DEPTH)));
        assert_eq!(check_depth(None).ok(), Some(None));
        assert_eq!(check_budget(None).ok(), Some(DEFAULT_BUDGET));
        assert!(check_budget(Some(0)).is_err());
        assert!(check_budget(Some(MAX_BUDGET + 1)).is_err());
        assert_eq!(check_include(Some("runs")).ok(), Some(true));
        assert_eq!(check_include(None).ok(), Some(false));
        assert!(check_include(Some("all")).is_err());
        let kinds = parse_kinds("service, system").ok();
        assert_eq!(kinds, Some(vec![StarKind::Service, StarKind::System]));
        assert!(parse_kinds("service,planet").is_err());
    }

    #[test]
    fn queries_confine_every_table_to_the_window_and_scope() {
        let window = Window {
            from_ns: 100,
            to_ns: 200,
            envs: vec!["demo".into(), "o'hare".into()],
        };
        let scope = "IN ('demo', 'o''hare')";
        let q = services_sql(&window);
        assert!(q.contains("ts_ns >= 100 AND ts_ns < 200"));
        assert!(q.contains(&format!("target_environment {scope}")));
        assert!(q.contains("span_name <> 'resilience.experiment'"));
        assert!(q.contains("resource_attrs['resilience.target.component']"));
        assert!(!q.contains("resilience.target.name"));
        let q = calls_sql(&window);
        assert!(q.contains(&format!("c.target_environment {scope}")));
        assert!(q.contains(&format!("p.target_environment {scope}")));
        let q = runs_sql(&window);
        assert!(q.contains(&format!("s.target_environment {scope}")));
        assert!(q.contains(&format!("m.target_environment {scope}")));
    }

    #[test]
    fn joins_are_bounded_in_time_on_both_sides() {
        let hour = 3_600 * 1_000_000_000_i64;
        let window = Window {
            from_ns: 2 * hour,
            to_ns: 3 * hour,
            envs: vec![],
        };
        let q = calls_sql(&window);
        assert!(q.contains(&format!(
            "c.ts_ns >= {} AND c.ts_ns < {}",
            2 * hour,
            3 * hour
        )));
        assert!(
            q.contains(&format!("p.ts_ns >= {hour} AND p.ts_ns < {}", 3 * hour)),
            "the parent side reaches one hour before the window: {q}"
        );
        let q = runs_sql(&window);
        assert!(
            q.contains(&tumult_lake::run_completions_sql(Some(2 * hour))),
            "the completion log join is bounded: {q}"
        );
    }

    #[test]
    fn owners_come_from_the_org_tree_matcher() {
        let org = OrgTree::from_yaml(
            "nodes:\n  - {name: data, kind: domain}\n  - {name: db-team, parent: data}\n\
             assignments:\n  - team: db-team\n    targets: [\"pg-*\"]\n    entities: [\"CIF-*\"]\n",
        )
        .expect("org fixture");
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_string()).collect();
        let entities = owners(&org, names(&["cif-0001", "CIF-0002", "abc-1"]), |id| {
            org.assign_entity(id)
        });
        assert_eq!(
            entities,
            BTreeMap::from([
                ("CIF-0002".to_string(), "data/db-team".to_string()),
                ("cif-0001".to_string(), "data/db-team".to_string()),
            ]),
            "the pattern matches whatever the case, and keys stay as reported"
        );
        let experiments = owners(&org, names(&["pg-failover", "cache-stampede"]), |name| {
            org.owner_of_experiment(name)
        });
        assert_eq!(
            experiments,
            BTreeMap::from([("pg-failover".to_string(), "data/db-team".to_string())])
        );
    }

    fn query() -> StarQuery {
        StarQuery::default()
    }

    #[test]
    fn the_cache_key_separates_principals_and_parameters() {
        let scopes = |list: &[&str]| -> Vec<String> { list.iter().map(|s| (*s).into()).collect() };
        let base = cache_key(&query(), &scopes(&["demo", "prod"]));
        assert_eq!(
            base,
            cache_key(&query(), &scopes(&["prod", "demo", "demo"])),
            "scope order does not matter"
        );
        assert_ne!(base, cache_key(&query(), &scopes(&["demo"])));
        assert_ne!(base, cache_key(&query(), &scopes(&[])));
        assert_ne!(
            cache_key(&query(), &scopes(&["a,b"])),
            cache_key(&query(), &scopes(&["a", "b"])),
            "scopes cannot be glued together"
        );
        let differing = [
            StarQuery {
                range: "30d".into(),
                ..query()
            },
            StarQuery {
                env: Some("demo".into()),
                ..query()
            },
            StarQuery {
                focus: Some("svc:a".into()),
                ..query()
            },
            StarQuery {
                focus: Some("svc:a".into()),
                depth: Some(3),
                ..query()
            },
            StarQuery {
                kinds: Some(vec![StarKind::Service]),
                ..query()
            },
            StarQuery {
                budget: 7,
                ..query()
            },
            StarQuery {
                include_runs: true,
                ..query()
            },
        ];
        let mut keys: Vec<String> = differing
            .iter()
            .map(|q| cache_key(q, &scopes(&["demo", "prod"])))
            .collect();
        keys.push(base);
        let distinct: BTreeSet<&String> = keys.iter().collect();
        assert_eq!(distinct.len(), keys.len(), "{keys:?}");

        let kinds = |list: Vec<StarKind>| StarQuery {
            kinds: Some(list),
            ..query()
        };
        assert_eq!(
            cache_key(&kinds(vec![StarKind::System, StarKind::Service]), &[]),
            cache_key(&kinds(vec![StarKind::Service, StarKind::System]), &[]),
            "kind order does not matter"
        );
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, value.parse().expect("header value"));
        }
        map
    }

    #[test]
    fn request_headers_choose_fresh_and_not_modified() {
        assert!(!wants_fresh(&headers(&[])));
        assert!(wants_fresh(&headers(&[("cache-control", "no-cache")])));
        assert!(wants_fresh(&headers(&[(
            "cache-control",
            "max-age=0, No-Cache"
        )])));
        assert!(!wants_fresh(&headers(&[("cache-control", "max-age=0")])));

        let etag = "\"abc\"";
        assert!(!not_modified(&headers(&[]), etag));
        assert!(not_modified(
            &headers(&[("if-none-match", "\"abc\"")]),
            etag
        ));
        assert!(not_modified(
            &headers(&[("if-none-match", "\"old\", W/\"abc\"")]),
            etag
        ));
        assert!(not_modified(&headers(&[("if-none-match", "*")]), etag));
        assert!(!not_modified(
            &headers(&[("if-none-match", "\"abd\"")]),
            etag
        ));
    }

    #[test]
    fn a_held_response_is_answered_with_304_and_no_body() {
        let map = CachedMap::new(b"{}".to_vec());
        let full = respond(&map, &headers(&[]));
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[ETAG], map.etag.as_str());
        assert_eq!(full.headers()[CACHE_CONTROL], "private, no-cache");
        assert_eq!(full.headers()[CONTENT_TYPE], "application/json");

        let mut held = HeaderMap::new();
        held.insert(IF_NONE_MATCH, map.etag.parse().expect("etag value"));
        let short = respond(&map, &held);
        assert_eq!(short.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(short.headers()[ETAG], map.etag.as_str());
        assert!(short.headers().get(CONTENT_TYPE).is_none());
    }

    #[test]
    fn assembly_runs_inside_a_span_that_says_what_it_built() {
        let inputs = StarInputs {
            services: vec![
                ServiceRow {
                    service: "checkout".into(),
                    system: Some("shop".into()),
                    environment: Some("demo".into()),
                    ..ServiceRow::default()
                },
                ServiceRow {
                    service: "ledger".into(),
                    system: Some("shop".into()),
                    environment: Some("demo".into()),
                    ..ServiceRow::default()
                },
            ],
            ..StarInputs::default()
        };
        let query = StarQuery {
            range: "14d".into(),
            ..StarQuery::default()
        };
        let (map, spans) =
            capture_closed_spans(|| assemble_traced(&tracing::Span::none(), &inputs, &query, 2));
        assert!(map.counts.nodes > 0);

        let [span] = spans.as_slice() else {
            panic!("one span, not {spans:?}");
        };
        assert_eq!(span.name, "starmap.assemble");
        assert_eq!(
            span.fields,
            serde_json::json!({
                "range": "14d",
                "envs": 2,
                "nodes": map.counts.nodes,
                "edges": map.counts.edges,
                "clustered": 0,
            })
        );

        let open = Window {
            from_ns: 100,
            to_ns: 200,
            envs: vec![],
        };
        assert!(!services_sql(&open).contains("target_environment IN"));
        assert!(!calls_sql(&open).contains(" IN ("));
    }
}
