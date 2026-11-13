//! Function-register storage (schema v20): legal entities, functions, the
//! telemetry-to-function mapping and the daily per-requirement evidence
//! status.
//!
//! The register is declared in `org.yaml` and synced here on daemon start.
//! The sync is the only writer of the register tables and preserves one
//! invariant: a `register_code` (`F<n>`), once assigned to a
//! (function, legal entity, licensed activity) combination, is never
//! changed and never handed to another combination — a function removed
//! from the declaration is kept as an inactive row so its number stays
//! taken.

use std::collections::BTreeMap;

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
use crate::store::{query_json_rows, with_tx};
use crate::{Reader, Writer};

/// One legal entity served by the deployment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IctLegalEntity {
    /// Legal Entity Identifier (ISO 17442).
    pub lei: String,
    pub name: String,
    pub country: Option<String>,
    pub entity_type: Option<String>,
    pub competent_authority: Option<String>,
    /// `full` or `simplified`.
    pub profile: String,
    /// Entity flags (`microenterprise`, `tlpt_designated`, …).
    pub flags: Vec<String>,
    /// Evidence feeds connected for this entity (`incident`, `slo`, …).
    pub feeds: Vec<String>,
    /// Who signed off the requirement registry for this entity.
    pub registry_signed_by: Option<String>,
    /// When the registry was signed off (`YYYY-MM-DD`).
    pub registry_signed_at: Option<String>,
}

/// One test the entity plans to run for a function, with its interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTest {
    /// Discipline (`dr`, `chaos`, …).
    pub kind: String,
    /// Planned interval between executions, in days.
    pub interval_days: u32,
}

/// One registered function under one legal entity and licensed activity.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IctFunction {
    /// The function's `ict.entity.id`.
    pub function_id: String,
    /// `F<n>`; ignored on sync input, assigned by the store.
    pub register_code: String,
    pub lei: String,
    pub name: String,
    pub licensed_activity: String,
    /// `critical`, `important` or `other`.
    pub tier: String,
    pub reasons: Option<String>,
    /// Date of the last criticality assessment (`YYYY-MM-DD`).
    pub last_assessed: Option<String>,
    pub rto_s: Option<f64>,
    pub rpo_s: Option<f64>,
    pub impact: Option<String>,
    /// Org-tree path of the unit that owns the function.
    pub org_unit: Option<String>,
    /// Requirement ids the entity has declared a plan or policy for.
    pub declared: Vec<String>,
    pub test_plan: Vec<PlannedTest>,
    /// `false` once the function left the declaration.
    pub active: bool,
}

/// One telemetry-to-function mapping row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IctFunctionService {
    pub function_id: String,
    /// `entity` (an `ict.entity.id`) or `service` (a `service.name` glob).
    pub kind: String,
    pub pattern: String,
}

/// The whole register as declared (sync input) or stored (read output).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IctRegister {
    pub legal_entities: Vec<IctLegalEntity>,
    pub functions: Vec<IctFunction>,
    pub services: Vec<IctFunctionService>,
}

/// One `requirement_status_daily` row.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementStatusRow {
    /// Days since the Unix epoch (UTC).
    pub as_of_day: i64,
    pub function_id: String,
    pub requirement_id: String,
    /// `na`, `unknown`, `no_events`, `none`, `designed`, `existing`,
    /// `operating`.
    pub status: String,
    /// The evidence value `v` behind the status, when one exists.
    pub value: Option<f64>,
    /// `snapshot`: computed on the day it describes. Only those rows are
    /// read back as score inputs.
    pub source: String,
}

/// Key of a register-code assignment.
type CodeKey = (String, String, String);

/// Codes already assigned, and the highest number used per legal entity.
type AssignedCodes = (BTreeMap<CodeKey, String>, BTreeMap<String, u64>);

fn code_key(function: &IctFunction) -> CodeKey {
    (
        function.function_id.clone(),
        function.lei.clone(),
        function.licensed_activity.clone(),
    )
}

fn code_number(code: &str) -> u64 {
    code.strip_prefix('F')
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn json_list(items: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(items).map_err(|e| StoreError::Internal(format!("ict list: {e}")))
}

fn plan_json(plan: &[PlannedTest]) -> String {
    let items: Vec<Value> = plan
        .iter()
        .map(|p| serde_json::json!({ "kind": p.kind, "interval_days": p.interval_days }))
        .collect();
    Value::Array(items).to_string()
}

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_text(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn list(row: &Value, key: &str) -> Result<Vec<String>, StoreError> {
    let raw = row.get(key).and_then(Value::as_str).unwrap_or("[]");
    serde_json::from_str(raw).map_err(|e| StoreError::Internal(format!("ict column {key}: {e}")))
}

fn plan_from(row: &Value) -> Result<Vec<PlannedTest>, StoreError> {
    let raw = row.get("test_plan").and_then(Value::as_str).unwrap_or("[]");
    let items: Vec<Value> = serde_json::from_str(raw)
        .map_err(|e| StoreError::Internal(format!("ict column test_plan: {e}")))?;
    Ok(items
        .iter()
        .map(|item| PlannedTest {
            kind: text(item, "kind"),
            interval_days: item
                .get("interval_days")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        })
        .collect())
}

fn legal_entity_from(row: &Value) -> Result<IctLegalEntity, StoreError> {
    Ok(IctLegalEntity {
        lei: text(row, "lei"),
        name: text(row, "name"),
        country: opt_text(row, "country"),
        entity_type: opt_text(row, "entity_type"),
        competent_authority: opt_text(row, "competent_authority"),
        profile: text(row, "profile"),
        flags: list(row, "flags")?,
        feeds: list(row, "feeds")?,
        registry_signed_by: opt_text(row, "registry_signed_by"),
        registry_signed_at: opt_text(row, "registry_signed_at"),
    })
}

fn function_from(row: &Value) -> Result<IctFunction, StoreError> {
    Ok(IctFunction {
        function_id: text(row, "function_id"),
        register_code: text(row, "register_code"),
        lei: text(row, "lei"),
        name: text(row, "name"),
        licensed_activity: text(row, "licensed_activity"),
        tier: text(row, "tier"),
        reasons: opt_text(row, "reasons"),
        last_assessed: opt_text(row, "last_assessed"),
        rto_s: row.get("rto_s").and_then(Value::as_f64),
        rpo_s: row.get("rpo_s").and_then(Value::as_f64),
        impact: opt_text(row, "impact"),
        org_unit: opt_text(row, "org_unit"),
        declared: list(row, "declared")?,
        test_plan: plan_from(row)?,
        active: row.get("active").and_then(Value::as_bool).unwrap_or(false),
    })
}

const REGISTER_FUNCTIONS_SQL: &str = "SELECT function_id, register_code, lei, name, \
     licensed_activity, tier, reasons, last_assessed, rto_s, rpo_s, impact, org_unit, declared, \
     test_plan, active FROM ict_functions \
     ORDER BY lei, CAST(substr(register_code, 2) AS BIGINT)";

fn read_register(conn: &duckdb::Connection) -> Result<IctRegister, StoreError> {
    let legal_entities = query_json_rows(
        conn,
        "SELECT lei, name, country, entity_type, competent_authority, profile, flags, feeds, \
         registry_signed_by, registry_signed_at FROM ict_legal_entities ORDER BY lei",
    )?
    .iter()
    .map(legal_entity_from)
    .collect::<Result<_, _>>()?;
    let functions = query_json_rows(conn, REGISTER_FUNCTIONS_SQL)?
        .iter()
        .map(function_from)
        .collect::<Result<_, _>>()?;
    let services = query_json_rows(
        conn,
        "SELECT function_id, kind, pattern FROM ict_function_services ORDER BY position",
    )?
    .iter()
    .map(|row| IctFunctionService {
        function_id: text(row, "function_id"),
        kind: text(row, "kind"),
        pattern: text(row, "pattern"),
    })
    .collect();
    Ok(IctRegister {
        legal_entities,
        functions,
        services,
    })
}

fn read_status(
    conn: &duckdb::Connection,
    from_day: i64,
) -> Result<Vec<RequirementStatusRow>, StoreError> {
    let sql = format!(
        "SELECT CAST(datediff('day', DATE '1970-01-01', as_of) AS BIGINT) AS as_of_day, \
         function_id, requirement_id, status, value, source FROM requirement_status_daily \
         WHERE as_of >= DATE '1970-01-01' + CAST({from_day} AS INTEGER) \
         ORDER BY as_of, function_id, requirement_id"
    );
    Ok(query_json_rows(conn, &sql)?
        .iter()
        .map(|row| RequirementStatusRow {
            as_of_day: row.get("as_of_day").and_then(Value::as_i64).unwrap_or(0),
            function_id: text(row, "function_id"),
            requirement_id: text(row, "requirement_id"),
            status: text(row, "status"),
            value: row.get("value").and_then(Value::as_f64),
            source: text(row, "source"),
        })
        .collect())
}

impl Writer {
    /// Replace the stored register with the declared one, keeping every
    /// `register_code` already assigned and numbering new combinations
    /// after the highest code of their legal entity.
    ///
    /// # Errors
    /// Returns an error if a statement fails (the transaction is rolled back).
    pub fn sync_ict_register(&self, register: &IctRegister, now_ns: i64) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            self.replace_legal_entities(&register.legal_entities, now_ns)?;
            self.sync_functions(&register.functions, now_ns)?;
            self.replace_function_services(&register.services)
        })
    }

    fn replace_legal_entities(
        &self,
        entities: &[IctLegalEntity],
        now_ns: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM ict_legal_entities", [])?;
        for e in entities {
            self.conn.execute(
                "INSERT INTO ict_legal_entities (lei, name, country, entity_type, \
                 competent_authority, profile, flags, feeds, registry_signed_by, \
                 registry_signed_at, updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    e.lei,
                    e.name,
                    e.country,
                    e.entity_type,
                    e.competent_authority,
                    e.profile,
                    json_list(&e.flags)?,
                    json_list(&e.feeds)?,
                    e.registry_signed_by,
                    e.registry_signed_at,
                    now_ns
                ],
            )?;
        }
        Ok(())
    }

    /// Codes already assigned, and the highest number used per legal entity.
    fn assigned_codes(&self) -> Result<AssignedCodes, StoreError> {
        let mut codes = BTreeMap::new();
        let mut highest: BTreeMap<String, u64> = BTreeMap::new();
        for function in read_register(&self.conn)?.functions {
            let top = highest.entry(function.lei.clone()).or_insert(0);
            *top = (*top).max(code_number(&function.register_code));
            codes.insert(code_key(&function), function.register_code);
        }
        Ok((codes, highest))
    }

    fn sync_functions(&self, functions: &[IctFunction], now_ns: i64) -> Result<(), StoreError> {
        let (codes, mut highest) = self.assigned_codes()?;
        self.conn.execute(
            "UPDATE ict_functions SET active = false, updated_at_ns = ? WHERE active",
            params![now_ns],
        )?;
        for (position, function) in functions.iter().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            if codes.contains_key(&code_key(function)) {
                self.update_function(function, position, now_ns)?;
                continue;
            }
            let top = highest.entry(function.lei.clone()).or_insert(0);
            *top += 1;
            self.insert_function(function, &format!("F{top}"), position, now_ns)?;
        }
        Ok(())
    }

    fn update_function(
        &self,
        f: &IctFunction,
        position: i64,
        now_ns: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE ict_functions SET name = ?, tier = ?, is_critical_or_important = ?, \
             reasons = ?, last_assessed = ?, rto_s = ?, rpo_s = ?, impact = ?, org_unit = ?, \
             declared = ?, test_plan = ?, active = true, position = ?, updated_at_ns = ? \
             WHERE function_id = ? AND lei = ? AND licensed_activity = ?",
            params![
                f.name,
                f.tier,
                is_critical_or_important(&f.tier),
                f.reasons,
                f.last_assessed,
                f.rto_s,
                f.rpo_s,
                f.impact,
                f.org_unit,
                json_list(&f.declared)?,
                plan_json(&f.test_plan),
                position,
                now_ns,
                f.function_id,
                f.lei,
                f.licensed_activity
            ],
        )?;
        Ok(())
    }

    fn insert_function(
        &self,
        f: &IctFunction,
        register_code: &str,
        position: i64,
        now_ns: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO ict_functions (function_id, register_code, lei, name, \
             licensed_activity, tier, is_critical_or_important, reasons, last_assessed, rto_s, \
             rpo_s, impact, org_unit, declared, test_plan, active, position, registered_at_ns, \
             updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, true, ?, ?, ?)",
            params![
                f.function_id,
                register_code,
                f.lei,
                f.name,
                f.licensed_activity,
                f.tier,
                is_critical_or_important(&f.tier),
                f.reasons,
                f.last_assessed,
                f.rto_s,
                f.rpo_s,
                f.impact,
                f.org_unit,
                json_list(&f.declared)?,
                plan_json(&f.test_plan),
                position,
                now_ns,
                now_ns
            ],
        )?;
        Ok(())
    }

    fn replace_function_services(&self, services: &[IctFunctionService]) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM ict_function_services", [])?;
        for (position, s) in services.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO ict_function_services (function_id, kind, pattern, position) \
                 VALUES (?, ?, ?, ?)",
                params![
                    s.function_id,
                    s.kind,
                    s.pattern,
                    i64::try_from(position).unwrap_or(i64::MAX)
                ],
            )?;
        }
        Ok(())
    }

    /// The stored register, read on the write connection.
    ///
    /// # Errors
    /// Returns an error if a table cannot be read or a list column is not
    /// valid JSON.
    pub fn ict_register(&self) -> Result<IctRegister, StoreError> {
        read_register(&self.conn)
    }

    /// Write status rows, replacing any row already stored for the same
    /// day, function and requirement.
    ///
    /// # Errors
    /// Returns an error if a statement fails (the transaction is rolled back).
    pub fn upsert_requirement_status(
        &self,
        rows: &[RequirementStatusRow],
        computed_at_ns: i64,
    ) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            for row in rows {
                self.conn.execute(
                    "DELETE FROM requirement_status_daily \
                     WHERE as_of = DATE '1970-01-01' + CAST(? AS INTEGER) \
                     AND function_id = ? AND requirement_id = ?",
                    params![row.as_of_day, row.function_id, row.requirement_id],
                )?;
                self.conn.execute(
                    "INSERT INTO requirement_status_daily (as_of, function_id, \
                     requirement_id, status, value, source, computed_at_ns) \
                     VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, ?, ?, ?, ?, ?)",
                    params![
                        row.as_of_day,
                        row.function_id,
                        row.requirement_id,
                        row.status,
                        row.value,
                        row.source,
                        computed_at_ns
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// Status rows on or after `from_day`, read on the write connection.
    ///
    /// # Errors
    /// Returns an error if the table cannot be read.
    pub fn requirement_status_since(
        &self,
        from_day: i64,
    ) -> Result<Vec<RequirementStatusRow>, StoreError> {
        read_status(&self.conn, from_day)
    }
}

impl Reader {
    /// The stored register.
    ///
    /// # Errors
    /// Returns an error if a table cannot be read or a list column is not
    /// valid JSON.
    pub fn ict_register(&self) -> Result<IctRegister, StoreError> {
        read_register(&self.conn)
    }

    /// Status rows on or after `from_day` (days since the Unix epoch),
    /// oldest first.
    ///
    /// # Errors
    /// Returns an error if the table cannot be read.
    pub fn requirement_status_since(
        &self,
        from_day: i64,
    ) -> Result<Vec<RequirementStatusRow>, StoreError> {
        read_status(&self.conn, from_day)
    }
}

/// Whether a tier name is critical or important.
#[must_use]
pub fn is_critical_or_important(tier: &str) -> bool {
    matches!(tier, "critical" | "important")
}
