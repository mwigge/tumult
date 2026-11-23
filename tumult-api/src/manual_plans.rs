//! Planned manual exercises (`/api/manual/plans*`).
//!
//! A plan says which exercise is due when, and who owns it. It can be
//! edited while it is planned; the verified record that names it fulfils
//! it (see [`crate::manual`]). Writes ride the single writer like the
//! record writes, with the same roles and environment scopes. The acting
//! identity is the authenticated principal, or while auth is open the
//! `created_by`, `updated_by` or `by` field. Every body refuses a field it
//! does not know (422, naming the field).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use tumult_compliance::dora::HouseParams;
use tumult_lake::{ExerciseType, ManualError, NewExercisePlan, PlanEdit, PlanError, PlanFilter};

use crate::auth::Principal;
use crate::error::ApiError;
use crate::manual::{actor_or, check_environment, exec_scoped, manual_error, Subject};
use crate::sql_util::{internal, with_reader};
use crate::ApiState;

/// Days between two runs of a DR exercise when its plan names none.
const DR_INTERVAL_DAYS: i32 = 365;

/// A bad value of a plan body is a bad plan, not a bad record.
fn plan_error(err: ManualError) -> ApiError {
    match err {
        ManualError::Invalid(msg) => manual_error(&PlanError::Invalid(msg).into()),
        other => manual_error(&other),
    }
}

/// A field that was sent, even as `null`: `Some(None)` clears it. A field
/// left out stays `None`.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Body of `POST /api/manual/plans`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    name: String,
    exercise_type: String,
    target_system: Option<String>,
    target_environment: Option<String>,
    /// The function of the register the exercise tests.
    function_id: Option<String>,
    /// What the exercise will test.
    scope: Option<String>,
    owner: String,
    planned_at_ns: Option<i64>,
    due_at_ns: i64,
    /// Days from one run to the next. A plan of a type the house counts
    /// as DR is due every 365 days when it names none.
    interval_days: Option<i32>,
    rto_target_s: Option<f64>,
    rpo_target_s: Option<f64>,
    created_by: Option<String>,
}

impl PlanRequest {
    fn into_plan(self, params: &HouseParams) -> Result<NewExercisePlan, ManualError> {
        let exercise_type = ExerciseType::parse(&self.exercise_type)?;
        let dr = params.manual_kind(exercise_type.as_str()) == Some("dr");
        Ok(NewExercisePlan {
            name: self.name,
            exercise_type,
            target_system: self.target_system,
            target_environment: self.target_environment,
            function_id: self.function_id,
            scope: self.scope,
            owner: self.owner,
            planned_at_ns: self.planned_at_ns,
            due_at_ns: self.due_at_ns,
            interval_days: self.interval_days.or(dr.then_some(DR_INTERVAL_DAYS)),
            rto_target_s: self.rto_target_s,
            rpo_target_s: self.rpo_target_s,
        })
    }
}

/// Body of `PATCH /api/manual/plans/{id}`: each field sent replaces the
/// stored one, and `null` clears an optional field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPatch {
    name: Option<String>,
    exercise_type: Option<String>,
    #[serde(default, deserialize_with = "present")]
    target_system: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    target_environment: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    function_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    scope: Option<Option<String>>,
    owner: Option<String>,
    #[serde(default, deserialize_with = "present")]
    planned_at_ns: Option<Option<i64>>,
    due_at_ns: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    interval_days: Option<Option<i32>>,
    #[serde(default, deserialize_with = "present")]
    rto_target_s: Option<Option<f64>>,
    #[serde(default, deserialize_with = "present")]
    rpo_target_s: Option<Option<f64>>,
    updated_by: Option<String>,
}

impl PlanPatch {
    fn into_edit(self) -> Result<PlanEdit, ManualError> {
        Ok(PlanEdit {
            name: self.name,
            exercise_type: self
                .exercise_type
                .as_deref()
                .map(ExerciseType::parse)
                .transpose()?,
            target_system: self.target_system,
            target_environment: self.target_environment,
            function_id: self.function_id,
            scope: self.scope,
            owner: self.owner,
            planned_at_ns: self.planned_at_ns,
            due_at_ns: self.due_at_ns,
            interval_days: self.interval_days,
            rto_target_s: self.rto_target_s,
            rpo_target_s: self.rpo_target_s,
        })
    }
}

/// Body of `POST /api/manual/plans/{id}/cancel`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    by: Option<String>,
    reason: Option<String>,
}

/// `POST /api/manual/plans`: plan an exercise.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<PlanRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let by = actor_or(&principal, req.created_by.clone(), "created_by").map_err(plan_error)?;
    check_environment(&principal, req.target_environment.as_deref())?;
    let params = state.score_runtime().params();
    let plan = req.into_plan(&params).map_err(plan_error)?;
    let id = exec_scoped(&state, &principal, Vec::new(), move |w| {
        w.create_manual_plan(&plan, &by)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

#[derive(Deserialize)]
pub struct PlanListParams {
    status: Option<String>,
    /// The target system.
    target: Option<String>,
    /// Only plans due before this time, in unix nanoseconds.
    due_before: Option<i64>,
}

/// `GET /api/manual/plans?status=&target=&due_before=`: the plans, the
/// soonest due first.
pub async fn list(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<PlanListParams>,
) -> Result<Json<Value>, ApiError> {
    let filter = PlanFilter {
        status: params.status,
        target_system: params.target,
        due_before_ns: params.due_before,
    };
    let rows = with_reader(&state.db_path, move |reader| {
        let mut rows = reader.manual_plans(&filter).map_err(|e| e.to_string())?;
        rows.retain(|row| {
            principal.env_allowed(row["target_environment"].as_str().unwrap_or_default())
        });
        Ok(rows)
    })
    .await?;
    Ok(Json(json!({"plans": rows})))
}

/// `GET /api/manual/plans/{id}`: one plan with its audit trail. A plan in
/// an environment outside the principal's scopes is 404.
pub async fn detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let detail = with_reader(&state.db_path, move |reader| {
        reader.manual_plan_detail(&id).map_err(|e| e.to_string())
    })
    .await?;
    let not_found = || crate::error::not_found("plan not found");
    let Some(detail) = detail else {
        return Err(not_found());
    };
    if !principal.env_scopes.is_empty() {
        let env = detail.plan["target_environment"].as_str();
        if !env.is_some_and(|e| principal.env_scopes.iter().any(|s| s == e)) {
            return Err(not_found());
        }
    }
    Ok(Json(
        serde_json::to_value(detail).map_err(|e| internal(e.to_string()))?,
    ))
}

/// `PATCH /api/manual/plans/{id}`: change a planned plan.
pub async fn edit(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<PlanPatch>,
) -> Result<Json<Value>, ApiError> {
    let by = actor_or(&principal, req.updated_by.clone(), "updated_by").map_err(plan_error)?;
    if let Some(env) = &req.target_environment {
        check_environment(&principal, env.as_deref())?;
    }
    let edit = req.into_edit().map_err(plan_error)?;
    let subjects = vec![Subject::Plan(id.clone())];
    exec_scoped(&state, &principal, subjects, move |w| {
        w.edit_manual_plan(&id, &edit, &by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

/// `POST /api/manual/plans/{id}/cancel`: cancel a planned plan, with an
/// optional reason kept in its audit trail.
pub async fn cancel(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<CancelRequest>,
) -> Result<Json<Value>, ApiError> {
    let by = actor_or(&principal, req.by.clone(), "by").map_err(plan_error)?;
    let subjects = vec![Subject::Plan(id.clone())];
    exec_scoped(&state, &principal, subjects, move |w| {
        w.cancel_manual_plan(&id, &by, req.reason.as_deref())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}
