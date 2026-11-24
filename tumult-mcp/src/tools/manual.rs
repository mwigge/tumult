//! The manual registry: list and read the records and the plans, and draft
//! a record. An agent may draft, a person signs: no tool here submits,
//! signs off, verifies or attaches evidence (segregation of duties). A
//! person does that in the daemon, on the page or with `tumult manual`.
//!
//! The tools read and write the lake file, as the other store tools do,
//! so they need the lake free of the daemon's write lock.
//! [`ManualFilter`] is also the filter of `tumult manual list`.

use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use tumult_lake::{ManualError, NewManualExperiment, PlanFilter, Reader, Store};

use crate::error::ToolError;
use crate::tools::StructuredReport;

/// Which records a list shows. A record must match every field that is
/// set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManualFilter {
    /// `draft`, `submitted`, `verified` or `rejected`.
    pub status: Option<String>,
    pub exercise_type: Option<String>,
    /// The target system.
    pub target: Option<String>,
    /// Executed at or after this time, in unix nanoseconds.
    pub from_ns: Option<i64>,
    /// Executed before this time, in unix nanoseconds.
    pub to_ns: Option<i64>,
}

impl ManualFilter {
    /// Whether `record`, a row of the list, matches.
    #[must_use]
    pub fn matches(&self, record: &Value) -> bool {
        let text = |field: &str, want: Option<&String>| {
            want.is_none_or(|want| record[field].as_str() == Some(want.as_str()))
        };
        let executed = record["executed_at_ns"].as_i64();
        let after = self
            .from_ns
            .is_none_or(|from| executed.is_some_and(|at| at >= from));
        let before = self
            .to_ns
            .is_none_or(|to| executed.is_some_and(|at| at < to));
        text("status", self.status.as_ref())
            && text("exercise_type", self.exercise_type.as_ref())
            && text("target_system", self.target.as_ref())
            && after
            && before
    }
}

/// The first instant of a window, in unix nanoseconds: a date
/// (`YYYY-MM-DD`, UTC) is the start of that day, an RFC 3339 time is
/// itself.
///
/// # Errors
///
/// Returns the reason when `text` is neither.
pub fn window_start(text: &str) -> Result<i64, String> {
    instant(text, 0)
}

/// The end of a window, not part of it, in unix nanoseconds: a date
/// (`YYYY-MM-DD`, UTC) is the start of the next day, so the window holds
/// the whole day; an RFC 3339 time is itself.
///
/// # Errors
///
/// Returns the reason when `text` is neither.
pub fn window_end(text: &str) -> Result<i64, String> {
    instant(text, 1)
}

/// `text` as unix nanoseconds; a date moves `days` days on.
fn instant(text: &str, days: u64) -> Result<i64, String> {
    let refused = || format!("'{text}' is not a date (YYYY-MM-DD) or an RFC 3339 time");
    let at = match NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        Ok(date) => date
            .checked_add_days(chrono::Days::new(days))
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .map(|midnight| midnight.and_utc())
            .ok_or_else(refused)?,
        Err(_) => DateTime::parse_from_rfc3339(text)
            .map_err(|_| refused())?
            .with_timezone(&Utc),
    };
    at.timestamp_nanos_opt().ok_or_else(refused)
}

/// `ns` as an RFC 3339 time in UTC, to the second.
fn time_of(ns: Option<i64>) -> String {
    ns.map(DateTime::from_timestamp_nanos).map_or_else(
        || "-".into(),
        |at| at.to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

/// A reader of the lake at `store_path`, which must be there.
fn reader(store_path: &str) -> Result<Reader, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "analytics store not found at {store_path}"
        )));
    }
    Store::at(path)
        .read_only()
        .map_err(|e| ToolError::Store(e.to_string()))
}

/// One page of `rows` as `{items, total, offset, limit}`.
fn page(rows: Vec<Value>, limit: usize, offset: usize) -> (Vec<Value>, Map<String, Value>) {
    let total = rows.len();
    let items: Vec<Value> = rows.into_iter().skip(offset).take(limit).collect();
    let mut structured = Map::new();
    structured.insert("items".into(), json!(items));
    structured.insert("total".into(), json!(total));
    structured.insert("offset".into(), json!(offset));
    structured.insert("limit".into(), json!(limit));
    (items, structured)
}

fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    row[name].as_str().unwrap_or("-")
}

/// The records of the lake at `store_path` that `filter` lets through,
/// newest entry first, one page of them.
///
/// # Errors
///
/// Returns an error when the lake is not there or cannot be read.
pub fn manual_list(
    store_path: &str,
    filter: &ManualFilter,
    limit: usize,
    offset: usize,
) -> Result<StructuredReport, ToolError> {
    let mut rows = reader(store_path)?
        .manual_experiments(filter.status.as_deref())
        .map_err(|e| ToolError::Store(e.to_string()))?;
    rows.retain(|row| filter.matches(row));
    let (items, structured) = page(rows, limit, offset);
    let mut text = format!("Manual records: {}\n", structured["total"]);
    for row in &items {
        let _ = writeln!(
            text,
            "  {}  {}  {}  {}  {}  {}",
            field(row, "id"),
            field(row, "status"),
            field(row, "exercise_type"),
            time_of(row["executed_at_ns"].as_i64()),
            field(row, "target_system"),
            field(row, "experiment_name"),
        );
    }
    Ok(StructuredReport { text, structured })
}

/// One record with its audit trail, attachments, steps, actions, links and
/// sign-offs.
///
/// # Errors
///
/// Returns `NotFound` for a record that is not there, and an error when
/// the lake cannot be read.
pub fn manual_get(store_path: &str, id: &str) -> Result<StructuredReport, ToolError> {
    let detail = reader(store_path)?
        .manual_experiment_detail(id)
        .map_err(|e| ToolError::Store(e.to_string()))?
        .ok_or_else(|| ToolError::NotFound(format!("manual record '{id}'")))?;
    let record = &detail.experiment;
    let text = format!(
        "{} ({id})\n  {}, {}, outcome {}\n  executed {}, target {}\n  \
         steps {}, actions {}, links {}, attachments {}, sign-offs {}\n",
        field(record, "experiment_name"),
        field(record, "exercise_type"),
        field(record, "status"),
        field(record, "outcome_status"),
        time_of(record["executed_at_ns"].as_i64()),
        field(record, "target_system"),
        detail.steps.len(),
        detail.actions.len(),
        detail.links.len(),
        detail.attachments.len(),
        detail.signoffs.len(),
    );
    let Value::Object(structured) =
        serde_json::to_value(&detail).map_err(|e| ToolError::Parse(e.to_string()))?
    else {
        return Err(ToolError::Parse("a record is not an object".into()));
    };
    Ok(StructuredReport { text, structured })
}

/// A refused draft as a tool error: what the caller sent is wrong, or
/// the lake failed.
fn draft_error(err: ManualError) -> ToolError {
    match err {
        ManualError::Store(e) => ToolError::Store(e.to_string()),
        ManualError::NotFound(_) | ManualError::Plan(tumult_lake::PlanError::NotFound(_)) => {
            ToolError::NotFound(err.to_string())
        }
        other => ToolError::InvalidInput(other.to_string()),
    }
}

/// Write `draft` into the lake at `store_path` as a `draft` record. A
/// person submits it and other people sign it off, in the daemon.
///
/// # Errors
///
/// Returns an error when the lake is not there, refuses the record (an
/// empty field, a plan the record cannot fulfil) or cannot be written.
pub fn manual_create_draft(
    store_path: &str,
    draft: &NewManualExperiment,
) -> Result<StructuredReport, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "analytics store not found at {store_path}"
        )));
    }
    let writer = Store::open(path)
        .and_then(|store| store.writer())
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let id = writer.create_manual_draft(draft).map_err(draft_error)?;
    let mut structured = Map::new();
    structured.insert("id".into(), json!(id));
    structured.insert("status".into(), json!("draft"));
    let text = format!(
        "Drafted manual record {id}. A person submits it, and other people sign it off, in the daemon.\n"
    );
    Ok(StructuredReport { text, structured })
}

/// The plans of the lake at `store_path` that `filter` lets through, the
/// soonest due first, one page of them.
///
/// # Errors
///
/// Returns an error when the lake is not there or cannot be read.
pub fn manual_plans_list(
    store_path: &str,
    filter: &PlanFilter,
    limit: usize,
    offset: usize,
) -> Result<StructuredReport, ToolError> {
    let rows = reader(store_path)?
        .manual_plans(filter)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let (items, structured) = page(rows, limit, offset);
    let mut text = format!("Plans: {}\n", structured["total"]);
    for row in &items {
        let _ = writeln!(
            text,
            "  {}  {}  {}  due {}  {}  {}",
            field(row, "id"),
            field(row, "status"),
            field(row, "exercise_type"),
            time_of(row["due_at_ns"].as_i64()),
            field(row, "target_system"),
            field(row, "name"),
        );
    }
    Ok(StructuredReport { text, structured })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ns: i64) -> Value {
        json!({"executed_at_ns": ns, "status": "draft", "exercise_type": "restore",
               "target_system": "ledger"})
    }

    #[test]
    fn a_date_window_holds_its_whole_last_day() {
        let start = window_start("2026-11-06").unwrap();
        let end = window_end("2026-11-06").unwrap();
        assert_eq!(start, 1_793_923_200_000_000_000);
        assert_eq!(end - start, 86_400_000_000_000);
        assert_eq!(
            window_end("2026-11-18T11:06:40Z").unwrap(),
            1_795_000_000_000_000_000
        );
        assert_eq!(
            window_start("2026-11-18T12:06:40+01:00").unwrap(),
            1_795_000_000_000_000_000
        );
        let refused = window_start("next week").unwrap_err();
        assert!(refused.contains("'next week'"), "{refused}");
        assert!(window_end("2026-13-01").is_err());
    }

    #[test]
    fn a_record_matches_when_every_set_field_matches() {
        let record = at(1_000);
        assert!(ManualFilter::default().matches(&record));
        let only = |filter: ManualFilter| filter.matches(&record);
        assert!(only(ManualFilter {
            status: Some("draft".into()),
            exercise_type: Some("restore".into()),
            target: Some("ledger".into()),
            from_ns: Some(1_000),
            to_ns: Some(1_001),
        }));
        assert!(!only(ManualFilter {
            status: Some("verified".into()),
            ..ManualFilter::default()
        }));
        assert!(!only(ManualFilter {
            exercise_type: Some("failover".into()),
            ..ManualFilter::default()
        }));
        assert!(!only(ManualFilter {
            target: Some("payments-db".into()),
            ..ManualFilter::default()
        }));
        assert!(!only(ManualFilter {
            from_ns: Some(1_001),
            ..ManualFilter::default()
        }));
        assert!(!only(ManualFilter {
            to_ns: Some(1_000),
            ..ManualFilter::default()
        }));
        let undated = json!({"status": "draft"});
        assert!(!ManualFilter {
            from_ns: Some(0),
            ..ManualFilter::default()
        }
        .matches(&undated));
    }
}
