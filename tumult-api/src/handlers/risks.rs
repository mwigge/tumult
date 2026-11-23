//! Passive risk scan and posture endpoints.
//!
//! * `GET /api/risks?env=` — the ranked findings of [`tumult_risk::scan`]
//!   over the lake (latest evidence per target and discipline, objective
//!   verdicts, the span call graph, registered definitions, planned manual
//!   exercises), the optional Kubernetes manifests directory and the
//!   suppression table.
//! * `GET /api/posture?env=` — the continuous posture score next to the
//!   risk summary, the coverage matrix and the top findings: the opening
//!   view of the `/posture` page.
//! * `POST /api/risks/{id}/suppress` — accept a finding for a while. The
//!   row rides the single-writer channel and is hash-chained per risk id.
//!
//! Scoped principals see only their own environments; `env` narrows the
//! scan to one of them.

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tumult_compliance::dora::HouseParams;
use tumult_compliance::scoring::{ExperimentScore, RunState};
use tumult_core::types::discipline;
use tumult_lake::{scope, PlanFilter, PlanStatus, Reader};
use tumult_risk::{
    Definition, Dependency, ExperimentLatest, Inputs, KindEvidence, PlannedExercise, Report,
    Suppression, TargetEvidence, Verdict,
};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, unavailable, ApiError};
use crate::sql_util::{env_trace_exists, internal, now_ns, sql_string, with_reader};
use crate::ApiState;

const DAY_NS: i64 = 86_400 * 1_000_000_000;
const MAX_REASON_CHARS: usize = 500;
const MAX_EXPIRY_DAYS: u32 = 3650;
const TOP_RISKS: usize = 5;

#[derive(Deserialize)]
pub(crate) struct ScopeQuery {
    env: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SuppressRequest {
    reason: String,
    expires_in_days: Option<u32>,
    /// Required only while auth is open (no authenticated principal).
    actor: Option<String>,
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

fn str_of(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn in_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| sql_string(id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Discipline of an automated run from its stored kind: chaos when the
/// run recorded none.
fn run_kind(test_kind: Option<String>) -> String {
    test_kind.map_or_else(|| "chaos".into(), |kind| discipline(&kind).to_owned())
}

struct KindRow {
    /// `None` for a manual record the house does not count as a test.
    kind: Option<String>,
    rto_target_s: Option<f64>,
    rto_measured_s: Option<f64>,
}

/// `experiment_id` → discipline and recovery targets, from the journal's
/// `experiments` rows (automated runs) and `manual_experiments` (manual,
/// by the house parameters, as the DORA score reads them).
fn kinds_by_evidence(
    reader: &Reader,
    experiments: &[ExperimentScore],
    params: &HouseParams,
) -> Result<BTreeMap<String, KindRow>, String> {
    let mut out = BTreeMap::new();
    let automated: Vec<String> = experiments
        .iter()
        .filter(|e| e.origin == "automated")
        .filter_map(|e| e.evidence_id.clone())
        .collect();
    if !automated.is_empty() {
        let sql = format!(
            "SELECT experiment_id, test_kind, rto_target_s, rto_measured_s \
             FROM experiments WHERE experiment_id IN ({})",
            in_list(&automated)
        );
        for row in reader.query_json_rows(&sql).map_err(|e| e.to_string())? {
            let Some(id) = str_of(&row, "experiment_id") else {
                continue;
            };
            out.insert(
                id,
                KindRow {
                    kind: Some(run_kind(str_of(&row, "test_kind"))),
                    rto_target_s: row.get("rto_target_s").and_then(Value::as_f64),
                    rto_measured_s: row.get("rto_measured_s").and_then(Value::as_f64),
                },
            );
        }
    }
    let manual: Vec<String> = experiments
        .iter()
        .filter(|e| e.origin == "manual")
        .filter_map(|e| e.evidence_id.clone())
        .collect();
    if !manual.is_empty() {
        let sql = format!(
            "SELECT id, exercise_type FROM manual_experiments WHERE id IN ({})",
            in_list(&manual)
        );
        for row in reader.query_json_rows(&sql).map_err(|e| e.to_string())? {
            let Some(id) = str_of(&row, "id") else {
                continue;
            };
            let kind = str_of(&row, "exercise_type")
                .and_then(|t| params.manual_discipline(&t).map(str::to_owned));
            out.insert(
                id,
                KindRow {
                    kind,
                    rto_target_s: None,
                    rto_measured_s: None,
                },
            );
        }
    }
    Ok(out)
}

/// `experiment_id` → objective verdicts of that run.
fn verdicts_by_evidence(
    reader: &Reader,
    experiments: &[ExperimentScore],
) -> Result<BTreeMap<String, Vec<Verdict>>, String> {
    let ids: Vec<String> = experiments
        .iter()
        .filter_map(|e| e.evidence_id.clone())
        .collect();
    let mut out: BTreeMap<String, Vec<Verdict>> = BTreeMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let sql = format!(
        "SELECT experiment_id, nfr_id, kind, state, criticality, threshold, observed, margin \
         FROM nfr_verdicts WHERE experiment_id IN ({}) ORDER BY nfr_id",
        in_list(&ids)
    );
    for row in reader.query_json_rows(&sql).map_err(|e| e.to_string())? {
        let Some(id) = str_of(&row, "experiment_id") else {
            continue;
        };
        out.entry(id).or_default().push(Verdict {
            nfr_id: str_of(&row, "nfr_id").unwrap_or_default(),
            kind: str_of(&row, "kind").unwrap_or_default(),
            state: str_of(&row, "state").unwrap_or_default(),
            criticality: str_of(&row, "criticality").unwrap_or_default(),
            threshold: row.get("threshold").and_then(Value::as_f64).unwrap_or(0.0),
            observed: row.get("observed").and_then(Value::as_f64),
            margin: row.get("margin").and_then(Value::as_f64),
        });
    }
    Ok(out)
}

/// Fold the latest experiment evidence into one [`TargetEvidence`] per
/// target and environment: the experiments themselves plus a
/// per-discipline rollup. A name tested in two environments is two
/// targets; evidence from one never covers the other. An experiment with
/// no discipline stays in the list and adds to no discipline.
fn targets_from(
    experiments: &[ExperimentScore],
    kinds: &BTreeMap<String, KindRow>,
    verdicts: &BTreeMap<String, Vec<Verdict>>,
) -> Vec<TargetEvidence> {
    let mut by_target: BTreeMap<(String, Option<String>), TargetEvidence> = BTreeMap::new();
    for e in experiments {
        let Some(target) = e.target.clone() else {
            continue;
        };
        let Some(last_run_ns) = e.last_run_ns else {
            continue;
        };
        let row = e.evidence_id.as_ref().and_then(|id| kinds.get(id));
        // No stored row: an automated run that recorded no kind.
        let kind = row.map_or_else(|| Some(run_kind(None)), |r| r.kind.clone());
        let entry = by_target
            .entry((target.clone(), e.environment.clone()))
            .or_insert_with(|| TargetEvidence {
                target,
                environment: e.environment.clone(),
                ..Default::default()
            });
        entry.experiments.push(ExperimentLatest {
            name: e.name.clone(),
            kind: kind.clone(),
            last_run_ns,
            passed: e.state != RunState::Failed,
            outcome: e.last_outcome.clone(),
            evidence_id: e.evidence_id.clone(),
            verdicts: e
                .evidence_id
                .as_ref()
                .and_then(|id| verdicts.get(id))
                .cloned()
                .unwrap_or_default(),
            rto_target_s: row.and_then(|r| r.rto_target_s),
            rto_measured_s: row.and_then(|r| r.rto_measured_s),
        });
        let Some(kind) = kind else {
            continue;
        };
        match entry.kinds.iter_mut().find(|k| k.kind == kind) {
            Some(k) => {
                k.runs += e.runs;
                if last_run_ns > k.last_run_ns {
                    k.last_run_ns = last_run_ns;
                    k.last_outcome.clone_from(&e.last_outcome);
                }
            }
            None => entry.kinds.push(KindEvidence {
                kind,
                runs: e.runs,
                last_run_ns,
                last_outcome: e.last_outcome.clone(),
            }),
        }
    }
    by_target.into_values().collect()
}

/// Callee services in the span call graph with how many distinct services
/// call them and how often: the "untested dependency" rule's input.
fn dependencies(reader: &Reader, envs: &[String]) -> Result<Vec<Dependency>, String> {
    let scope = scope::and(env_trace_exists("c", envs));
    let sql = format!(
        "SELECT c.service_name AS service, COUNT(DISTINCT p.service_name) AS callers, \
         COUNT(*) AS calls \
         FROM spans c JOIN spans p ON c.parent_span_id = p.span_id \
         WHERE p.service_name <> c.service_name AND c.service_name <> ''{scope} \
         GROUP BY 1 ORDER BY 1"
    );
    let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(Dependency {
                service: str_of(row, "service")?,
                callers: row.get("callers").and_then(Value::as_u64).unwrap_or(0),
                calls: row.get("calls").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect())
}

/// Registered experiment definitions (not GameDays) with their run count.
fn definitions(reader: &Reader) -> Result<Vec<Definition>, String> {
    let sql = "SELECT rg.id, rg.name, \
               (SELECT COUNT(*) FROM runs r WHERE r.registry_id = rg.id) AS runs \
               FROM run_registry rg WHERE rg.kind IS DISTINCT FROM 'gameday' ORDER BY rg.name";
    let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(Definition {
                id: str_of(row, "id")?,
                name: str_of(row, "name")?,
                runs: row.get("runs").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect())
}

fn suppressions(reader: &Reader) -> Result<Vec<Suppression>, String> {
    Ok(reader
        .list_risk_suppressions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| Suppression {
            risk_id: r.risk_id,
            reason: r.reason,
            actor: r.actor,
            at_ns: r.at_ns,
            expires_at_ns: r.expires_at_ns,
        })
        .collect())
}

/// The planned manual exercises in `envs` (every environment when empty).
/// A fulfilled or cancelled plan is never due.
fn planned_exercises(reader: &Reader, envs: &[String]) -> Result<Vec<PlannedExercise>, String> {
    let filter = PlanFilter {
        status: Some(PlanStatus::Planned.as_str().into()),
        ..PlanFilter::default()
    };
    let rows = reader.manual_plans(&filter).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter(|row| {
            envs.is_empty()
                || str_of(row, "target_environment").is_some_and(|env| envs.contains(&env))
        })
        .filter_map(|row| {
            Some(PlannedExercise {
                id: str_of(row, "id")?,
                name: str_of(row, "name").unwrap_or_default(),
                exercise_type: str_of(row, "exercise_type").unwrap_or_default(),
                target: str_of(row, "target_system"),
                environment: str_of(row, "target_environment"),
                owner: str_of(row, "owner").unwrap_or_default(),
                due_at_ns: row.get("due_at_ns").and_then(Value::as_i64)?,
            })
        })
        .collect())
}

/// Assemble the scanner inputs from the lake and the manifests directory.
/// `params` are the house parameters in force: they give the discipline
/// of a manual record.
pub(crate) fn gather(
    reader: &Reader,
    as_of_ns: i64,
    envs: &[String],
    manifests_dir: Option<&FsPath>,
    params: &HouseParams,
) -> Result<Inputs, String> {
    let card = tumult_compliance::scoring::compute_scoped(reader, as_of_ns, None, envs)?;
    let kinds = kinds_by_evidence(reader, &card.experiments, params)?;
    let verdicts = verdicts_by_evidence(reader, &card.experiments)?;
    let (workloads, budgets) = match manifests_dir {
        Some(dir) => {
            let m = tumult_risk::manifests::load_dir(dir)
                .map_err(|e| format!("manifests at {}: {e}", dir.display()))?;
            for note in &m.notes {
                tracing::warn!(note, "manifest document skipped");
            }
            (m.workloads, m.budgets)
        }
        None => (Vec::new(), Vec::new()),
    };
    Ok(Inputs {
        as_of_ns,
        targets: targets_from(&card.experiments, &kinds, &verdicts),
        dependencies: dependencies(reader, envs)?,
        definitions: definitions(reader)?,
        workloads,
        budgets,
        suppressions: suppressions(reader)?,
        plans: planned_exercises(reader, envs)?,
    })
}

/// One objective verdict from the latest run of an experiment, flattened
/// for the posture page's NFR heatmap (targets × objectives, hue = margin).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct NfrCell {
    target: String,
    environment: Option<String>,
    experiment: String,
    nfr_id: String,
    kind: String,
    state: String,
    criticality: String,
    threshold: f64,
    observed: Option<f64>,
    margin: Option<f64>,
}

fn nfr_cells(inputs: &Inputs) -> Vec<NfrCell> {
    let mut out: Vec<NfrCell> = inputs
        .targets
        .iter()
        .flat_map(|t| {
            t.experiments.iter().flat_map(move |e| {
                e.verdicts.iter().map(move |v| NfrCell {
                    target: t.target.clone(),
                    environment: t.environment.clone(),
                    experiment: e.name.clone(),
                    nfr_id: v.nfr_id.clone(),
                    kind: v.kind.clone(),
                    state: v.state.clone(),
                    criticality: v.criticality.clone(),
                    threshold: v.threshold,
                    observed: v.observed,
                    margin: v.margin,
                })
            })
        })
        .collect();
    out.sort_by(|a, b| a.target.cmp(&b.target).then(a.nfr_id.cmp(&b.nfr_id)));
    out
}

pub(crate) async fn scan_for(
    state: &ApiState,
    principal: &Principal,
    env: Option<String>,
) -> Result<(Inputs, Report), ApiError> {
    let envs = envs_for(principal, env)?;
    let as_of_ns = now_ns();
    let manifests_dir: Option<PathBuf> = state.manifests_dir().cloned();
    let params = state.score_runtime().params();
    with_reader(&state.db_path, move |reader| {
        let inputs = gather(reader, as_of_ns, &envs, manifests_dir.as_deref(), &params)?;
        let report = tumult_risk::scan(&inputs);
        Ok((inputs, report))
    })
    .await
}

pub(crate) async fn risks(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>, ApiError> {
    let (_, report) = scan_for(&state, &principal, q.env).await?;
    serde_json::to_value(report)
        .map(Json)
        .map_err(|e| internal(e.to_string()))
}

pub(crate) async fn posture(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>, ApiError> {
    let (inputs, report) = scan_for(&state, &principal, q.env).await?;
    let posture = tumult_risk::posture::compute(&inputs);
    let top: Vec<&tumult_risk::Risk> = report.risks.iter().take(TOP_RISKS).collect();
    let body = json!({
        "as_of_ns": report.as_of_ns,
        "posture": posture,
        "summary": report.summary,
        "coverage": report.coverage,
        "top_risks": top,
        "nfrs": nfr_cells(&inputs),
    });
    Ok(Json(body))
}

fn validate_suppression(req: &SuppressRequest) -> Result<(String, Option<i64>), ApiError> {
    let reason = req.reason.trim().to_string();
    if reason.is_empty() {
        return Err(bad_request("reason must not be empty"));
    }
    if reason.chars().count() > MAX_REASON_CHARS {
        return Err(bad_request(format!(
            "reason too long (max {MAX_REASON_CHARS} chars)"
        )));
    }
    let expires_in_ns = match req.expires_in_days {
        None => None,
        Some(0) => return Err(bad_request("expires_in_days must be at least 1")),
        Some(d) if d > MAX_EXPIRY_DAYS => {
            return Err(bad_request(format!(
                "expires_in_days must be at most {MAX_EXPIRY_DAYS}"
            )))
        }
        Some(d) => Some(i64::from(d) * DAY_NS),
    };
    Ok((reason, expires_in_ns))
}

pub(crate) async fn suppress(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<SuppressRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (reason, expires_in_ns) = validate_suppression(&req)?;
    let actor = match principal.actor() {
        Some(a) => a,
        None => req
            .actor
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| bad_request("actor is required while auth is open"))?,
    };
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "risk suppressions are not wired (no ingest handle)",
        ));
    };
    // A suppression must name a finding the principal can currently see.
    let (_, report) = scan_for(&state, &principal, None).await?;
    let known = report
        .risks
        .iter()
        .chain(report.suppressed.iter())
        .any(|r| r.id == id);
    if !known {
        return Err(not_found(format!("no current risk with id {id}")));
    }
    let at_ns = now_ns();
    let expires_at_ns = expires_in_ns.map(|d| at_ns + d);
    let row_id = uuid::Uuid::new_v4().to_string();
    let risk_id = id.clone();
    let reason2 = reason.clone();
    let actor2 = actor.clone();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot2 = std::sync::Arc::clone(&slot);
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            let row = writer
                .insert_risk_suppression(&row_id, &risk_id, &reason2, &actor2, at_ns, expires_at_ns)
                .map_err(|e| e.to_string())?;
            *slot2.lock().map_err(|e| e.to_string())? = Some(row);
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let row = slot
        .lock()
        .map_err(|e| internal(e.to_string()))?
        .take()
        .ok_or_else(|| internal("suppression was not recorded".into()))?;
    // Every Star Map counts suppressions: none may outlive this write.
    state.starmap_cache.clear();
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": row.id,
            "risk_id": row.risk_id,
            "reason": row.reason,
            "actor": row.actor,
            "at_ns": row.at_ns,
            "expires_at_ns": row.expires_at_ns,
            "prev_hash": row.prev_hash,
            "new_hash": row.new_hash,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(name: &str, id: &str, state: RunState, ts: i64, runs: u64) -> ExperimentScore {
        ExperimentScore {
            name: name.into(),
            target: Some("db".into()),
            environment: Some("demo".into()),
            evidence_id: Some(id.into()),
            score: 100,
            state,
            band: "good".into(),
            last_run_ns: Some(ts),
            last_outcome: Some("Completed".into()),
            severity: None,
            runs,
            origin: "automated".into(),
            journal_only: false,
        }
    }

    #[test]
    fn targets_roll_experiments_up_per_discipline() {
        let experiments = vec![
            score("pg-failover", "e1", RunState::Passed, 100, 3),
            score("dr-drill", "e2", RunState::Failed, 200, 1),
            score("cache", "e3", RunState::Passed, 50, 2),
        ];
        let mut kinds = BTreeMap::new();
        kinds.insert(
            "e2".to_string(),
            KindRow {
                kind: Some("robustness".into()),
                rto_target_s: Some(60.0),
                rto_measured_s: Some(90.0),
            },
        );
        let targets = targets_from(&experiments, &kinds, &BTreeMap::new());
        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.target, "db");
        assert_eq!(t.environment.as_deref(), Some("demo"));
        assert_eq!(t.experiments.len(), 3);
        let chaos = t.kinds.iter().find(|k| k.kind == "chaos").unwrap();
        assert_eq!((chaos.runs, chaos.last_run_ns), (5, 100));
        let dr = t.kinds.iter().find(|k| k.kind == "robustness").unwrap();
        assert_eq!((dr.runs, dr.last_run_ns), (1, 200));
        let drill = t.experiments.iter().find(|e| e.name == "dr-drill").unwrap();
        assert!(!drill.passed);
        assert_eq!(drill.rto_target_s, Some(60.0));
        assert_eq!(drill.rto_measured_s, Some(90.0));
    }

    #[test]
    fn one_target_name_in_two_environments_stays_two_rows() {
        let mut prod = score("pg-failover", "e2", RunState::Passed, 200, 4);
        prod.environment = Some("prod".into());
        let mut kinds = BTreeMap::new();
        kinds.insert(
            "e2".to_string(),
            KindRow {
                kind: Some("robustness".into()),
                rto_target_s: None,
                rto_measured_s: None,
            },
        );
        let demo = score("cache", "e1", RunState::Passed, 100, 1);
        let targets = targets_from(&[prod, demo], &kinds, &BTreeMap::new());
        let rows: Vec<(Option<&str>, Vec<&str>)> = targets
            .iter()
            .map(|t| {
                let kinds = t.kinds.iter().map(|k| k.kind.as_str()).collect();
                (t.environment.as_deref(), kinds)
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (Some("demo"), vec!["chaos"]),
                (Some("prod"), vec!["robustness"])
            ]
        );
        assert!(targets.iter().all(|t| t.target == "db"));
        assert!(targets.iter().all(|t| t.experiments.len() == 1));
    }

    #[test]
    fn untargeted_and_never_run_experiments_are_skipped() {
        let mut untargeted = score("x", "e1", RunState::Passed, 1, 1);
        untargeted.target = None;
        let mut never = score("y", "e2", RunState::NeverRun, 1, 0);
        never.last_run_ns = None;
        let targets = targets_from(&[untargeted, never], &BTreeMap::new(), &BTreeMap::new());
        assert!(targets.is_empty());
    }

    #[test]
    fn nfr_cells_flatten_verdicts_per_target_and_objective() {
        let verdict = |id: &str, margin: f64| Verdict {
            nfr_id: id.into(),
            kind: "latency".into(),
            state: if margin >= 0.0 { "met" } else { "breached" }.into(),
            criticality: "major".into(),
            threshold: 100.0,
            observed: Some(100.0 - margin),
            margin: Some(margin),
        };
        let inputs = Inputs {
            targets: vec![TargetEvidence {
                target: "db".into(),
                environment: Some("demo".into()),
                experiments: vec![ExperimentLatest {
                    name: "pg".into(),
                    kind: Some("robustness".into()),
                    last_run_ns: 1,
                    passed: true,
                    outcome: None,
                    evidence_id: None,
                    verdicts: vec![verdict("p99", 10.0), verdict("avail", -2.0)],
                    rto_target_s: None,
                    rto_measured_s: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let cells = nfr_cells(&inputs);
        assert_eq!(cells.len(), 2);
        assert_eq!(
            (cells[0].nfr_id.as_str(), cells[0].state.as_str()),
            ("avail", "breached")
        );
        assert_eq!(cells[1].experiment, "pg");
        assert_eq!(cells[1].margin, Some(10.0));
    }

    /// A register whose one function owns every `svc-` service.
    const REGISTER: &str = "
nodes:
  - {name: platform, kind: domain}
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    profile: full
functions:
  - id: cif-0001
    name: Payments
    tier: critical
    org_unit: platform
    services: [\"svc-*\"]
";

    /// A lake with the register above and, for each exercise type, one
    /// verified record against the service `svc-<type>`.
    fn lake_with_one_record_of_each(
        types: &[tumult_lake::ExerciseType],
    ) -> (tempfile::TempDir, tumult_lake::Store) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).expect("store");
        let writer = store.writer().expect("writer");
        let declared = tumult_compliance::dora::register::parse(REGISTER).expect("register");
        writer
            .sync_ict_register(&declared.ict, 1)
            .expect("sync register");
        for exercise in types {
            let name = exercise.as_str();
            let record = tumult_lake::NewManualExperiment {
                experiment_name: format!("exercise-{name}"),
                exercise_type: *exercise,
                executed_at_ns: now_ns() - DAY_NS,
                hypothesis: "the service holds".into(),
                method: "by hand".into(),
                outcome: tumult_lake::ManualOutcome::Passed,
                hypothesis_met: Some(true),
                findings: None,
                action_items: vec![],
                target_system: Some(format!("svc-{name}")),
                target_environment: Some("prod".into()),
                blast_radius: None,
                recovery_time_s: None,
                duration_s: None,
                entered_by: "alice".into(),
                attestation: "attested".into(),
                renewal_due_ns: None,
                framework_refs: vec![],
                exercise: tumult_lake::ExerciseDetail::default(),
            };
            let id = writer.create_manual_draft(&record).expect("draft");
            writer.submit_manual(&id, None, "alice").expect("submit");
            writer.verify_manual(&id, "bob", None).expect("verify");
        }
        drop(writer);
        (dir, store)
    }

    /// With the default house parameters, every exercise type counts
    /// towards the discipline `HouseParams::manual_discipline` gives it
    /// in the risk view and in the DORA score, or towards none in both.
    #[test]
    fn every_exercise_type_counts_towards_one_discipline_in_the_risk_view_and_in_dora() {
        use tumult_lake::ExerciseType;
        let table = [
            (ExerciseType::GameDay, Some("chaos")),
            (ExerciseType::Tabletop, None),
            (ExerciseType::Failover, Some("robustness")),
            (ExerciseType::Pentest, Some("security")),
            (ExerciseType::Tlpt, Some("security")),
            (ExerciseType::Drill, Some("robustness")),
            (ExerciseType::Other, None),
        ];
        let types: Vec<ExerciseType> = table.iter().map(|(t, _)| *t).collect();
        let (_dir, store) = lake_with_one_record_of_each(&types);
        let reader = store.read_only().expect("reader");
        let now = now_ns();

        let params = HouseParams::default();
        let inputs = gather(&reader, now, &[], None, &params).expect("risk inputs");
        let request = tumult_compliance::dora::loader::Request {
            envs: &[],
            lei: None,
            profile: None,
            as_of_ns: now,
            workloads: &[],
            proof_days: false,
        };
        let loaded =
            tumult_compliance::dora::loader::load(&reader, request, &params).expect("dora");

        for (exercise, want) in table {
            let name = exercise.as_str();
            assert_eq!(params.manual_discipline(name), want, "{name}");
            let target = inputs
                .targets
                .iter()
                .find(|t| t.target == format!("svc-{name}"))
                .unwrap_or_else(|| panic!("no risk target for {name}"));
            let listed: Vec<&str> = target.kinds.iter().map(|k| k.kind.as_str()).collect();
            assert_eq!(listed, want.into_iter().collect::<Vec<_>>(), "{name}");
            assert_eq!(target.experiments.len(), 1, "{name}");
            assert_eq!(target.experiments[0].kind.as_deref(), want, "{name}");
            let evidence = loaded
                .inputs
                .evidence
                .iter()
                .find(|e| e.experiment == format!("exercise-{name}"))
                .unwrap_or_else(|| panic!("no DORA evidence for {name}"));
            assert_eq!(evidence.kind.as_deref().map(discipline), want, "{name}");
        }
    }

    #[test]
    fn a_run_recorded_as_load_counts_as_performance() {
        assert_eq!(run_kind(Some("load".into())), "performance");
        assert_eq!(run_kind(Some("dr".into())), "robustness");
        assert_eq!(run_kind(None), "chaos");
    }

    #[test]
    fn suppression_validation() {
        let ok = SuppressRequest {
            reason: "  accepted  ".into(),
            expires_in_days: Some(30),
            actor: None,
        };
        let (reason, ttl) = validate_suppression(&ok).unwrap();
        assert_eq!(reason, "accepted");
        assert_eq!(ttl, Some(30 * DAY_NS));
        let empty = SuppressRequest {
            reason: " ".into(),
            expires_in_days: None,
            actor: None,
        };
        assert!(validate_suppression(&empty).is_err());
        let zero = SuppressRequest {
            reason: "x".into(),
            expires_in_days: Some(0),
            actor: None,
        };
        assert!(validate_suppression(&zero).is_err());
    }
}
