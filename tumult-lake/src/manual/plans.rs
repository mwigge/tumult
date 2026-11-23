//! Planned exercises (schema v22): which exercise is due when, and who
//! owns it.
//!
//! A plan is `planned` until a verified record fulfils it or someone
//! cancels it, and only a planned plan can be edited. Every write seals the
//! plan again and appends a row to `manual_plan_audit` whose `prev_hash` is
//! the seal before the write and whose `new_hash` is the seal after it.
//! When a record that names a plan is verified, the plan is fulfilled. A
//! plan with an interval then comes back: a new plan with the same
//! content, due one interval after the day the exercise ran.

use duckdb::{params, Connection};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::hash::{fetch_plan_row, now_ns, reseal_plan, status_of, ulid};
use super::{ExerciseType, ManualError, PlanError};
use crate::{with_tx, Reader, SqlParam, StoreError, Writer};

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// The longest interval a plan takes, ten years.
const MAX_INTERVAL_DAYS: i32 = 3650;

/// Where a plan is in its life. Only a `planned` plan can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Planned,
    /// A verified record fulfilled it.
    Fulfilled,
    Cancelled,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Fulfilled => "fulfilled",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The content of a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct NewExercisePlan {
    pub name: String,
    pub exercise_type: ExerciseType,
    pub target_system: Option<String>,
    pub target_environment: Option<String>,
    /// The function of the register the exercise tests.
    pub function_id: Option<String>,
    /// What the exercise will test.
    pub scope: Option<String>,
    /// Who makes sure the exercise runs.
    pub owner: String,
    /// When the exercise is set to run.
    pub planned_at_ns: Option<i64>,
    /// When the exercise must have run.
    pub due_at_ns: i64,
    /// Days from one run of the exercise to the next. A plan with an
    /// interval comes back when a record fulfils it.
    pub interval_days: Option<i32>,
    pub rto_target_s: Option<f64>,
    pub rpo_target_s: Option<f64>,
}

impl NewExercisePlan {
    fn validate(&self) -> Result<(), PlanError> {
        for (field, value) in [("name", &self.name), ("owner", &self.owner)] {
            if value.trim().is_empty() {
                return Err(PlanError::Invalid(format!("'{field}' must not be empty")));
            }
        }
        for (field, value) in [
            ("due_at_ns", Some(self.due_at_ns)),
            ("planned_at_ns", self.planned_at_ns),
        ] {
            if value.is_some_and(|at| at <= 0) {
                return Err(PlanError::Invalid(format!(
                    "'{field}' must be a positive unix-nanos timestamp"
                )));
            }
        }
        if self.planned_at_ns.is_some_and(|at| at > self.due_at_ns) {
            return Err(PlanError::Invalid(
                "'planned_at_ns' must not be after 'due_at_ns'".into(),
            ));
        }
        if self
            .interval_days
            .is_some_and(|days| !(1..=MAX_INTERVAL_DAYS).contains(&days))
        {
            return Err(PlanError::Invalid(format!(
                "'interval_days' must be from 1 to {MAX_INTERVAL_DAYS}"
            )));
        }
        for (field, value) in [
            ("rto_target_s", self.rto_target_s),
            ("rpo_target_s", self.rpo_target_s),
        ] {
            if value.is_some_and(|v| !(v.is_finite() && v >= 0.0)) {
                return Err(PlanError::Invalid(format!(
                    "'{field}' must be a number of seconds at or above 0"
                )));
            }
        }
        Ok(())
    }

    /// The plan stored in `row`.
    fn from_row(row: &Value) -> Result<Self, ManualError> {
        let text = |k: &str| row.get(k).and_then(Value::as_str);
        let owned = |k: &str| text(k).map(str::to_owned);
        let num = |k: &str| row.get(k).and_then(Value::as_f64);
        let int = |k: &str| row.get(k).and_then(Value::as_i64);
        Ok(Self {
            name: text("name").unwrap_or_default().to_owned(),
            exercise_type: ExerciseType::parse(text("exercise_type").unwrap_or_default())?,
            target_system: owned("target_system"),
            target_environment: owned("target_environment"),
            function_id: owned("function_id"),
            scope: owned("scope"),
            owner: text("owner").unwrap_or_default().to_owned(),
            planned_at_ns: int("planned_at_ns"),
            due_at_ns: int("due_at_ns").unwrap_or(0),
            interval_days: int("interval_days").and_then(|d| i32::try_from(d).ok()),
            rto_target_s: num("rto_target_s"),
            rpo_target_s: num("rpo_target_s"),
        })
    }
}

/// A change to a plan: each field that is `Some` replaces the stored one,
/// and `Some(None)` clears an optional field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanEdit {
    pub name: Option<String>,
    pub exercise_type: Option<ExerciseType>,
    pub target_system: Option<Option<String>>,
    pub target_environment: Option<Option<String>>,
    pub function_id: Option<Option<String>>,
    pub scope: Option<Option<String>>,
    pub owner: Option<String>,
    pub planned_at_ns: Option<Option<i64>>,
    pub due_at_ns: Option<i64>,
    pub interval_days: Option<Option<i32>>,
    pub rto_target_s: Option<Option<f64>>,
    pub rpo_target_s: Option<Option<f64>>,
}

/// Put `value` in `field` when the edit sets it, and note the change.
fn set<T: Clone + Serialize>(
    changes: &mut Map<String, Value>,
    name: &str,
    value: Option<&T>,
    field: &mut T,
) {
    if let Some(value) = value {
        field.clone_from(value);
        changes.insert(name.into(), json!(value));
    }
}

impl PlanEdit {
    /// Apply the edit to `plan`; returns each field it set, with the new value.
    fn apply(&self, plan: &mut NewExercisePlan) -> Map<String, Value> {
        let mut changes = Map::new();
        let c = &mut changes;
        set(c, "name", self.name.as_ref(), &mut plan.name);
        if let Some(exercise_type) = self.exercise_type {
            plan.exercise_type = exercise_type;
            c.insert("exercise_type".into(), json!(exercise_type.as_str()));
        }
        set(
            c,
            "target_system",
            self.target_system.as_ref(),
            &mut plan.target_system,
        );
        set(
            c,
            "target_environment",
            self.target_environment.as_ref(),
            &mut plan.target_environment,
        );
        set(
            c,
            "function_id",
            self.function_id.as_ref(),
            &mut plan.function_id,
        );
        set(c, "scope", self.scope.as_ref(), &mut plan.scope);
        set(c, "owner", self.owner.as_ref(), &mut plan.owner);
        set(
            c,
            "planned_at_ns",
            self.planned_at_ns.as_ref(),
            &mut plan.planned_at_ns,
        );
        set(c, "due_at_ns", self.due_at_ns.as_ref(), &mut plan.due_at_ns);
        set(
            c,
            "interval_days",
            self.interval_days.as_ref(),
            &mut plan.interval_days,
        );
        set(
            c,
            "rto_target_s",
            self.rto_target_s.as_ref(),
            &mut plan.rto_target_s,
        );
        set(
            c,
            "rpo_target_s",
            self.rpo_target_s.as_ref(),
            &mut plan.rpo_target_s,
        );
        changes
    }
}

/// What [`Reader::manual_plans`] lists. Every field that is set narrows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanFilter {
    pub status: Option<String>,
    pub target_system: Option<String>,
    /// Only plans due before this time.
    pub due_before_ns: Option<i64>,
}

/// One plan and its audit trail, as JSON objects straight from the store.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlanDetail {
    pub plan: Value,
    pub audit: Vec<Value>,
}

fn require_actor(by: &str) -> Result<(), PlanError> {
    if by.trim().is_empty() {
        return Err(PlanError::Invalid(
            "the acting user must not be empty".into(),
        ));
    }
    Ok(())
}

/// Append one plan audit row (no transaction; caller wraps).
fn append_plan_audit(
    conn: &Connection,
    plan_id: &str,
    by: &str,
    action: &str,
    diff: &Value,
    prev_hash: Option<&str>,
    new_hash: &str,
) -> Result<(), ManualError> {
    conn.execute(
        "INSERT INTO manual_plan_audit (
            id, plan_id, changed_by, changed_at_ns, action, diff, prev_hash, new_hash
         ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?)",
        params![
            ulid(),
            plan_id,
            by,
            now_ns(),
            action,
            diff.to_string(),
            prev_hash,
            new_hash,
        ],
    )?;
    Ok(())
}

/// Insert `plan` as `planned`, seal it and write its `create` audit row
/// (no transaction; caller wraps). Returns the new id.
fn insert_plan(
    conn: &Connection,
    plan: &NewExercisePlan,
    by: &str,
    diff: &Value,
) -> Result<String, ManualError> {
    let id = ulid();
    conn.execute(
        "INSERT INTO manual_exercise_plans (
            id, name, exercise_type, target_system, target_environment, function_id,
            scope, owner, planned_at_ns, due_at_ns, interval_days, rto_target_s,
            rpo_target_s, status, created_by, created_at_ns, content_hash
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            id,
            plan.name,
            plan.exercise_type.as_str(),
            plan.target_system,
            plan.target_environment,
            plan.function_id,
            plan.scope,
            plan.owner,
            plan.planned_at_ns,
            plan.due_at_ns,
            plan.interval_days,
            plan.rto_target_s,
            plan.rpo_target_s,
            PlanStatus::Planned.as_str(),
            by,
            now_ns(),
            // Replaced by the seal below, in the same transaction.
            "",
        ],
    )?;
    let hash = reseal_plan(conn, &id)?;
    append_plan_audit(conn, &id, by, "create", diff, None, &hash)?;
    Ok(id)
}

/// The row of plan `id` while it is planned. `action` names what the
/// caller is about to do, for the error.
fn planned_row(conn: &Connection, id: &str, action: &str) -> Result<Value, ManualError> {
    let row = fetch_plan_row(conn, id)?.ok_or_else(|| PlanError::NotFound(id.to_string()))?;
    let status = status_of(&row);
    if status != PlanStatus::Planned.as_str() {
        return Err(PlanError::WrongStatus {
            status,
            action: action.into(),
        }
        .into());
    }
    Ok(row)
}

/// Seal plan `id` again after a write and add the audit row that chains
/// the seal of `before` to the new one.
fn seal_and_audit(
    conn: &Connection,
    id: &str,
    by: &str,
    action: &str,
    diff: &Value,
    before: &Value,
) -> Result<(), ManualError> {
    let prev_hash = before.get("content_hash").and_then(Value::as_str);
    let hash = reseal_plan(conn, id)?;
    append_plan_audit(conn, id, by, action, diff, prev_hash, &hash)
}

/// Move plan `id` to `status`, done by `by` now.
fn close(conn: &Connection, id: &str, status: PlanStatus, by: &str) -> Result<(), ManualError> {
    conn.execute(
        "UPDATE manual_exercise_plans SET status = ?, updated_by = ?, updated_at_ns = ? \
         WHERE id = ?",
        params![status.as_str(), by, now_ns(), id],
    )?;
    Ok(())
}

/// The row of plan `id` when a record of `exercise_type` can fulfil it:
/// the plan exists, is planned and plans an exercise of that type.
pub(super) fn usable_plan(
    conn: &Connection,
    id: &str,
    exercise_type: &str,
) -> Result<Value, ManualError> {
    let row = fetch_plan_row(conn, id)?
        .ok_or_else(|| PlanError::Unusable(format!("plan '{id}' does not exist")))?;
    let status = status_of(&row);
    if status != PlanStatus::Planned.as_str() {
        return Err(PlanError::Unusable(format!("plan '{id}' is {status}, not planned")).into());
    }
    let planned = row
        .get("exercise_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if planned != exercise_type {
        return Err(PlanError::Unusable(format!(
            "plan '{id}' plans a {planned} exercise, not a {exercise_type}"
        ))
        .into());
    }
    Ok(row)
}

/// What a record tests, held against the plan it names.
pub(super) struct Tested<'a> {
    pub(super) target_system: Option<&'a str>,
    pub(super) target_environment: Option<&'a str>,
    /// The functions the record links to.
    pub(super) functions: Vec<&'a str>,
}

/// Refuse plan `row` (id `id`) unless the record tests what it planned:
/// the plan's target system, target environment and function, each one
/// that is set, must be the record's.
pub(super) fn check_tested(row: &Value, id: &str, tested: &Tested<'_>) -> Result<(), ManualError> {
    let planned = |k: &str| row.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    for (field, has) in [
        ("target_system", tested.target_system),
        ("target_environment", tested.target_environment),
    ] {
        let Some(want) = planned(field) else {
            continue;
        };
        if has != Some(want) {
            let has = has.map_or_else(|| "none".to_string(), |h| format!("'{h}'"));
            return Err(PlanError::Unusable(format!(
                "plan '{id}' plans {field} '{want}', the record has {has}"
            ))
            .into());
        }
    }
    if let Some(want) = planned("function_id") {
        if !tested.functions.contains(&want) {
            return Err(PlanError::Unusable(format!(
                "plan '{id}' plans function_id '{want}', the record links no such function"
            ))
            .into());
        }
    }
    Ok(())
}

/// The functions stored record `id` links to.
fn linked_functions(conn: &Connection, id: &str) -> Result<Vec<String>, ManualError> {
    let rows = crate::query_json_rows_bound(
        conn,
        "SELECT linked_id FROM manual_experiment_links \
         WHERE experiment_id = ? AND kind = 'function' ORDER BY ordinal",
        &[id],
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            r.get("linked_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

/// Fulfil plan `plan_id` with the record in `record`, verified by `by` (no
/// transaction; caller wraps). The plan is checked again: it must still
/// be planned and still plan what the record tested. A plan with an
/// interval comes back, due one interval after the exercise ran.
pub(super) fn fulfil(
    conn: &Connection,
    plan_id: &str,
    record: &Value,
    by: &str,
) -> Result<(), ManualError> {
    let text = |k: &str| record.get(k).and_then(Value::as_str);
    let record_id = text("id").unwrap_or_default();
    let row = usable_plan(conn, plan_id, text("exercise_type").unwrap_or_default())?;
    let functions = linked_functions(conn, record_id)?;
    let tested = Tested {
        target_system: text("target_system"),
        target_environment: text("target_environment"),
        functions: functions.iter().map(String::as_str).collect(),
    };
    check_tested(&row, plan_id, &tested)?;
    let plan = NewExercisePlan::from_row(&row)?;
    close(conn, plan_id, PlanStatus::Fulfilled, by)?;
    let next_plan_id = match plan.interval_days {
        None => None,
        Some(days) => {
            let ran_at = record
                .get("executed_at_ns")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let next = NewExercisePlan {
                planned_at_ns: None,
                due_at_ns: ran_at.saturating_add(i64::from(days) * DAY_NS),
                ..plan
            };
            let diff =
                json!({"via": "fulfil", "previous_plan_id": plan_id, "record_id": record_id});
            Some(insert_plan(conn, &next, by, &diff)?)
        }
    };
    let diff = json!({"record_id": record_id, "next_plan_id": next_plan_id});
    seal_and_audit(conn, plan_id, by, "fulfil", &diff, &row)
}

impl Writer {
    /// Create a plan, `planned`. Returns the new id.
    ///
    /// # Errors
    /// Returns `Plan(Invalid)` for bad content or an empty `by`, `Store` on
    /// write failure.
    pub fn create_manual_plan(
        &self,
        plan: &NewExercisePlan,
        by: &str,
    ) -> Result<String, ManualError> {
        plan.validate()?;
        require_actor(by)?;
        let diff = json!({
            "name": plan.name,
            "exercise_type": plan.exercise_type.as_str(),
            "due_at_ns": plan.due_at_ns,
        });
        with_tx(&self.conn, || insert_plan(&self.conn, plan, by, &diff))
    }

    /// Change a planned plan. The audit row lists each field set, with its
    /// new value.
    ///
    /// # Errors
    /// Returns `Plan(NotFound)`, `Plan(WrongStatus)` when the plan is no
    /// longer planned, or `Plan(Invalid)` for an edit that names no field or
    /// leaves bad content.
    pub fn edit_manual_plan(&self, id: &str, edit: &PlanEdit, by: &str) -> Result<(), ManualError> {
        require_actor(by)?;
        if *edit == PlanEdit::default() {
            return Err(PlanError::Invalid("an edit names no field".into()).into());
        }
        with_tx(&self.conn, || {
            let row = planned_row(&self.conn, id, "edit")?;
            let mut plan = NewExercisePlan::from_row(&row)?;
            let changes = edit.apply(&mut plan);
            plan.validate()?;
            self.conn.execute(
                "UPDATE manual_exercise_plans SET
                    name = ?, exercise_type = ?, target_system = ?, target_environment = ?,
                    function_id = ?, scope = ?, owner = ?, planned_at_ns = ?, due_at_ns = ?,
                    interval_days = ?, rto_target_s = ?, rpo_target_s = ?,
                    updated_by = ?, updated_at_ns = ?
                 WHERE id = ?",
                params![
                    plan.name,
                    plan.exercise_type.as_str(),
                    plan.target_system,
                    plan.target_environment,
                    plan.function_id,
                    plan.scope,
                    plan.owner,
                    plan.planned_at_ns,
                    plan.due_at_ns,
                    plan.interval_days,
                    plan.rto_target_s,
                    plan.rpo_target_s,
                    by,
                    now_ns(),
                    id,
                ],
            )?;
            let diff = json!({"fields": changes});
            seal_and_audit(&self.conn, id, by, "edit", &diff, &row)
        })
    }

    /// Cancel a planned plan. A cancelled plan is never due.
    ///
    /// # Errors
    /// Returns `Plan(NotFound)`, `Plan(WrongStatus)`, or `Plan(Invalid)`
    /// for an empty `by`.
    pub fn cancel_manual_plan(
        &self,
        id: &str,
        by: &str,
        reason: Option<&str>,
    ) -> Result<(), ManualError> {
        require_actor(by)?;
        with_tx(&self.conn, || {
            let row = planned_row(&self.conn, id, "cancel")?;
            close(&self.conn, id, PlanStatus::Cancelled, by)?;
            let diff = json!({
                "from_status": PlanStatus::Planned.as_str(),
                "to_status": PlanStatus::Cancelled.as_str(),
                "reason": reason,
            });
            seal_and_audit(&self.conn, id, by, "cancel", &diff, &row)
        })
    }
}

impl Reader {
    /// The plans `filter` lets through, the soonest due first. Every value
    /// is bound.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn manual_plans(&self, filter: &PlanFilter) -> Result<Vec<Value>, StoreError> {
        let mut sql = String::from("SELECT * FROM manual_exercise_plans WHERE TRUE");
        let mut params = Vec::new();
        if let Some(status) = &filter.status {
            sql.push_str(" AND status = ?");
            params.push(SqlParam::Text(status.clone()));
        }
        if let Some(target) = &filter.target_system {
            sql.push_str(" AND target_system = ?");
            params.push(SqlParam::Text(target.clone()));
        }
        if let Some(before) = filter.due_before_ns {
            sql.push_str(" AND due_at_ns < ?");
            params.push(SqlParam::Int(before));
        }
        sql.push_str(" ORDER BY due_at_ns ASC, id ASC");
        self.query_json_rows_bound(&sql, &params)
    }

    /// One plan with its audit trail, oldest row first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn manual_plan_detail(&self, id: &str) -> Result<Option<PlanDetail>, StoreError> {
        let rows =
            self.query_json_rows_bound("SELECT * FROM manual_exercise_plans WHERE id = ?", &[id])?;
        let Some(plan) = rows.into_iter().next() else {
            return Ok(None);
        };
        let audit = self.query_json_rows_bound(
            "SELECT * FROM manual_plan_audit WHERE plan_id = ? ORDER BY changed_at_ns ASC, id ASC",
            &[id],
        )?;
        Ok(Some(PlanDetail { plan, audit }))
    }
}
