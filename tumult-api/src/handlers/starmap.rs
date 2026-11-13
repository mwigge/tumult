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

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;
use tumult_compliance::OrgTree;
use tumult_graph::starmap::{
    assemble, ArticleRow, CallRow, CoverFrom, CoverRow, DefinitionRow, DependsRow, ExperimentRow,
    GapRow, GlobRow, OrgUnitRow, PostureRow, RiskRow, RunRow, ServiceRow, StarInputs, StarKind,
    StarQuery, SuppressionRow, SystemRow, WorkloadRow, DEFAULT_BUDGET, MAX_BUDGET, MAX_DEPTH,
};
use tumult_lake::Reader;
use tumult_risk::{CoverageStatus, Inputs, Report, Risk};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, ApiError};
use crate::handlers::{conformance, risks};
use crate::sql_util::{and_pred, env_scope_where, internal, now_ns, parse_range, with_reader};
use crate::ApiState;

const DEFAULT_RANGE: &str = "7d";
/// The only value `include` accepts.
const INCLUDE_RUNS: &str = "runs";
/// Namespace a manifest without one deploys to.
const DEFAULT_NAMESPACE: &str = "default";

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

/// The org tree flattened to rows, taken before the blocking read.
struct OrgRows {
    units: Vec<OrgUnitRow>,
    entity_globs: Vec<GlobRow>,
    experiment_globs: Vec<GlobRow>,
}

/// The shared ranges plus `30d`, which only the Star Map offers: an
/// estate changes slowly, and a month of telemetry still shows a service
/// that reports weekly.
fn range_seconds(range: &str) -> Option<i64> {
    if range == "30d" {
        return Some(30 * 86_400);
    }
    parse_range(range)
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

fn validate(params: StarMapParams, principal: &Principal) -> Result<(StarQuery, Window), ApiError> {
    let range = params.range.unwrap_or_else(|| DEFAULT_RANGE.to_string());
    let Some(range_s) = range_seconds(&range) else {
        return Err(bad_request(format!(
            "invalid range {range:?}; expected 24h|7d|14d|30d"
        )));
    };
    let envs = envs_for(principal, params.env.as_deref())?;
    let focus = params.focus.filter(|f| !f.is_empty());
    let depth = check_depth(params.depth)?.filter(|_| focus.is_some());
    let kinds = params.kinds.as_deref().map(parse_kinds).transpose()?;
    let to_ns = now_ns();
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
    let window = Window {
        from_ns: to_ns - range_s * 1_000_000_000,
        to_ns,
        envs,
    };
    Ok((query, window))
}

fn org_rows(org: &OrgTree) -> OrgRows {
    let glob = |(pattern, unit): (&str, usize)| GlobRow {
        pattern: pattern.to_string(),
        path: org.path_of(unit),
    };
    OrgRows {
        units: org
            .unit_ids()
            .map(|id| OrgUnitRow {
                path: org.path_of(id),
                name: org.node(id).name.clone(),
                kind: org.node(id).kind.clone(),
            })
            .collect(),
        entity_globs: org.entity_globs().into_iter().map(glob).collect(),
        experiment_globs: org.target_globs().into_iter().map(glob).collect(),
    }
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

/// Q1, the service catalogue: one row per distinct resource-attribute
/// tuple. Experiment root spans are left out because their target columns
/// name the system under test, not the system the runner belongs to.
fn services_sql(window: &Window) -> String {
    let attr = |key: &str| format!("NULLIF(resource_attrs['{key}'], '')");
    let scope = and_pred(env_scope_where("target_environment", &window.envs));
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
fn calls_sql(window: &Window) -> String {
    let child = and_pred(env_scope_where("c.target_environment", &window.envs));
    let parent = and_pred(env_scope_where("p.target_environment", &window.envs));
    format!(
        "SELECT p.service_name AS client, c.service_name AS server, COUNT(*) AS calls, \
         COUNT(*) FILTER (WHERE c.status_code = 'Error') AS errors, \
         AVG(c.duration_ns) AS avg_duration_ns \
         FROM spans c JOIN spans p ON c.parent_span_id = p.span_id \
         WHERE p.service_name <> c.service_name AND p.service_name <> '' \
         AND c.service_name <> '' AND c.ts_ns >= {} AND c.ts_ns < {}{child}{parent} \
         GROUP BY 1, 2",
        window.from_ns, window.to_ns,
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

/// Runs and verified manual records that started inside the window. The
/// outcome of a run is on its root span or its completion log.
fn runs_sql(window: &Window) -> String {
    let span_scope = and_pred(env_scope_where("s.target_environment", &window.envs));
    let manual_scope = and_pred(env_scope_where("m.target_environment", &window.envs));
    let (from, to) = (window.from_ns, window.to_ns);
    format!(
        "SELECT s.experiment_id AS id, any_value(s.experiment_name) AS experiment, \
         any_value(s.target_system) AS system, any_value(s.target_environment) AS environment, \
         MIN(s.ts_ns) AS started_ns, \
         any_value(COALESCE(s.outcome_status, l.log_attrs['status'])) AS status, \
         false AS manual \
         FROM spans s LEFT JOIN logs l \
           ON l.log_attrs['experiment_id'] = s.experiment_id \
          AND l.body = 'experiment.completed' \
         WHERE s.span_name = 'resilience.experiment' AND s.experiment_id IS NOT NULL \
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

/// The scorecard's rule: completed, passed and partial pass; anything
/// else, including a run that never reported an outcome, failed.
fn failed(status: Option<&str>) -> bool {
    let status = status.map(str::to_ascii_lowercase);
    !matches!(status.as_deref(), Some("completed" | "passed" | "partial"))
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
                failed: failed(row.get("status").and_then(Value::as_str)),
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

/// Risk subjects the Star Map has a node for: a workload (by its
/// `namespace/name`) or a registered definition (by id). Every other
/// subject names a discipline or an objective, and the risk then belongs
/// to its target.
fn subjects(inputs: &Inputs) -> BTreeMap<String, String> {
    let workloads = inputs.workloads.iter().map(|w| {
        (
            w.qualified_name(),
            format!("{}/{}", namespace_of(w), w.name),
        )
    });
    let definitions = inputs
        .definitions
        .iter()
        .map(|d| (d.id.clone(), d.id.clone()));
    workloads.chain(definitions).collect()
}

fn risk_row(risk: &Risk, subjects: &BTreeMap<String, String>) -> RiskRow {
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
/// for example `DORA Art. 25(1)` to `compliance:DORA/Art.25`. A paragraph
/// suffix falls back to its article. `None` when the registry has no such
/// citation.
fn manual_article(reference: &str) -> Option<String> {
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
fn covers(reader: &Reader, window: &Window) -> Result<Vec<CoverRow>, String> {
    let edges = rows(
        reader,
        "SELECT DISTINCT src, rel, dst FROM graph_edges \
         WHERE rel IN ('evidences', 'maps_to_compliance')",
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
    let scope = and_pred(env_scope_where("m.target_environment", &window.envs));
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
    Ok(out)
}

fn graph_nodes(reader: &Reader, kind: &str) -> Result<Vec<(String, String)>, String> {
    let found = rows(
        reader,
        &format!("SELECT id, label FROM graph_nodes WHERE kind = '{kind}'"),
    )?;
    Ok(found
        .iter()
        .filter_map(|row| {
            let id = text(row, "id")?;
            let label = text(row, "label").unwrap_or_else(|| id.clone());
            Some((id, label))
        })
        .collect())
}

fn articles(reader: &Reader) -> Result<Vec<ArticleRow>, String> {
    let nodes = graph_nodes(reader, "compliance_article")?;
    Ok(nodes
        .into_iter()
        .map(|(id, label)| ArticleRow { id, label })
        .collect())
}

fn gaps(reader: &Reader) -> Result<Vec<GapRow>, String> {
    let nodes = graph_nodes(reader, "coverage_gap")?;
    Ok(nodes
        .into_iter()
        .map(|(id, label)| GapRow { id, label })
        .collect())
}

/// Declared service dependencies from a loaded topology file.
fn depends(reader: &Reader) -> Result<Vec<DependsRow>, String> {
    let found = rows(
        reader,
        "SELECT DISTINCT src, dst FROM graph_edges WHERE rel = 'depends_on'",
    )?;
    Ok(found
        .iter()
        .filter_map(|row| {
            Some(DependsRow {
                from: text(row, "src")?.strip_prefix("svc:")?.to_string(),
                to: text(row, "dst")?.strip_prefix("svc:")?.to_string(),
            })
        })
        .collect())
}

fn load(
    reader: &Reader,
    window: &Window,
    manifests_dir: Option<&FsPath>,
    org: OrgRows,
) -> Result<StarInputs, String> {
    let risk_inputs = risks::gather(reader, window.to_ns, &window.envs, manifests_dir)?;
    let report = tumult_risk::scan(&risk_inputs);
    let levels = conformance::levels(reader, window.from_ns, &window.envs)?;
    Ok(StarInputs {
        as_of_ns: window.to_ns,
        org_units: org.units,
        entity_globs: org.entity_globs,
        experiment_globs: org.experiment_globs,
        services: services(reader, window, &levels)?,
        calls: calls(reader, window)?,
        systems: systems(&risk_inputs, &report),
        workloads: workloads(&risk_inputs),
        risks: risk_rows(&risk_inputs, &report),
        definitions: definitions(&risk_inputs),
        runs: runs(reader, window)?,
        covers: covers(reader, window)?,
        articles: articles(reader)?,
        depends: depends(reader)?,
        gaps: gaps(reader)?,
    })
}

pub(crate) async fn starmap(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<StarMapParams>,
) -> Result<Json<Value>, ApiError> {
    let (query, window) = validate(params, &principal)?;
    let org = org_rows(&state.org);
    let manifests_dir: Option<PathBuf> = state.manifests_dir().cloned();
    let inputs = with_reader(&state.db_path, move |reader| {
        load(reader, &window, manifests_dir.as_deref(), org)
    })
    .await?;
    let map = assemble(&inputs, &query);
    serde_json::to_value(map)
        .map(Json)
        .map_err(|e| internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_rule_matches_the_scorecard() {
        for passed in ["Completed", "completed", "passed", "partial"] {
            assert!(!failed(Some(passed)), "{passed}");
        }
        for broke in ["Deviated", "Failed", "failed", "Aborted", ""] {
            assert!(failed(Some(broke)), "{broke}");
        }
        assert!(failed(None), "a run with no outcome did not pass");
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
    fn parameters_are_bounded() {
        assert_eq!(range_seconds("30d"), Some(30 * 86_400));
        assert_eq!(range_seconds("7d"), Some(7 * 86_400));
        assert_eq!(range_seconds("1y"), None);
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

        let open = Window {
            from_ns: 100,
            to_ns: 200,
            envs: vec![],
        };
        assert!(!services_sql(&open).contains("target_environment IN"));
        assert!(!calls_sql(&open).contains(" IN ("));
    }
}
