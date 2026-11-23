//! Plain text of the answers of the daemon for `tumult manual`.

use std::fmt::Write as _;

use chrono::{DateTime, SecondsFormat};
use serde_json::Value;

fn text<'a>(row: &'a Value, field: &str) -> Option<&'a str> {
    row[field].as_str().filter(|s| !s.is_empty())
}

fn or_dash<'a>(row: &'a Value, field: &str) -> &'a str {
    text(row, field).unwrap_or("-")
}

/// `ns` as an RFC 3339 time in UTC, to the second.
fn time(ns: Option<i64>) -> String {
    ns.map(DateTime::from_timestamp_nanos).map_or_else(
        || "-".into(),
        |at| at.to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

/// `ns` as a date in UTC.
fn date(ns: Option<i64>) -> String {
    ns.map(DateTime::from_timestamp_nanos)
        .map_or_else(|| "-".into(), |at| at.format("%Y-%m-%d").to_string())
}

/// `count` and `noun`, the noun in the plural unless the count is one.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The target system of a record or a plan, with its environment.
fn target(row: &Value) -> String {
    match (text(row, "target_system"), text(row, "target_environment")) {
        (Some(system), Some(env)) => format!("{system} in {env}"),
        (Some(system), None) => system.to_owned(),
        (None, Some(env)) => format!("- in {env}"),
        (None, None) => "-".to_owned(),
    }
}

/// Seconds as `300 s`, or `not set`.
fn seconds(row: &Value, field: &str) -> String {
    row[field]
        .as_f64()
        .map_or_else(|| "not set".into(), |s| format!("{s} s"))
}

/// The list of records: a count, then one line per record.
pub fn records(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "no manual records\n".into();
    }
    let mut out = format!("{}\n", counted(rows.len(), "record"));
    for row in rows {
        let _ = writeln!(
            out,
            "{}  {}  {}  {}  {}  {}",
            or_dash(row, "id"),
            or_dash(row, "status"),
            or_dash(row, "exercise_type"),
            date(row["executed_at_ns"].as_i64()),
            target(row),
            or_dash(row, "experiment_name"),
        );
    }
    out
}

/// The facts of the record itself, under its name.
fn head(out: &mut String, record: &Value) {
    let _ = writeln!(
        out,
        "{} ({})\n  {}, {}, outcome {}\n  executed {}, {}\n  entered by {}",
        or_dash(record, "experiment_name"),
        or_dash(record, "id"),
        or_dash(record, "exercise_type"),
        or_dash(record, "status"),
        or_dash(record, "outcome_status"),
        time(record["executed_at_ns"].as_i64()),
        target(record),
        or_dash(record, "entered_by"),
    );
    let _ = writeln!(
        out,
        "  RTO planned {}, actual {}",
        seconds(record, "rto_target_s"),
        seconds(record, "recovery_time_s"),
    );
    let data = match record["data_verified"].as_bool() {
        Some(true) => "data verified",
        Some(false) => "data loss or corruption found",
        None => "data not checked",
    };
    let _ = writeln!(
        out,
        "  RPO planned {}, actual {}, {data}",
        seconds(record, "rpo_target_s"),
        seconds(record, "rpo_measured_s"),
    );
    if let Some(plan) = text(record, "plan_id") {
        let _ = writeln!(out, "  plan {plan}");
    }
    if let Some(severity) = text(record, "findings_severity") {
        let _ = writeln!(out, "  findings severity {severity}");
    }
    if let Some(tester) = text(record, "tester") {
        let _ = writeln!(
            out,
            "  tester {tester} ({})",
            or_dash(record, "tester_kind")
        );
    }
    tlpt(out, record);
}

/// The TLPT block of a record: the attestation and the test summary
/// report, when the record holds either.
fn tlpt(out: &mut String, record: &Value) {
    let attestation = text(record, "tlpt_attestation_ref");
    let summary = text(record, "tlpt_summary_ref");
    let attested = record["tlpt_attested_at_ns"].as_i64();
    if attestation.is_none() && summary.is_none() && attested.is_none() {
        return;
    }
    let _ = writeln!(
        out,
        "  TLPT attestation {} of {}, test summary report {}",
        attestation.unwrap_or("-"),
        date(attested),
        summary.unwrap_or("-"),
    );
}

/// A part of the record with a title and one line per item; nothing when
/// there are no items.
fn part(out: &mut String, title: &str, items: &Value, line: fn(&Value) -> String) {
    let Some(items) = items.as_array().filter(|items| !items.is_empty()) else {
        return;
    };
    let _ = writeln!(out, "{title}");
    for item in items {
        let _ = writeln!(out, "  {}", line(item));
    }
}

fn participant(p: &Value) -> String {
    match text(p, "role") {
        Some(role) => format!("{} ({role})", or_dash(p, "name")),
        None => or_dash(p, "name").to_owned(),
    }
}

fn step(s: &Value) -> String {
    let mut line = format!("{}. {}", s["ordinal"], or_dash(s, "title"));
    if let Some(actor) = text(s, "actor") {
        let _ = write!(line, " ({actor})");
    }
    if let Some(result) = text(s, "result") {
        let _ = write!(line, ": {result}");
    }
    line
}

fn action(a: &Value) -> String {
    let mut line = format!(
        "{}. {}, owner {}, {}",
        a["ordinal"],
        or_dash(a, "title"),
        or_dash(a, "owner"),
        or_dash(a, "status"),
    );
    if let Some(severity) = text(a, "severity") {
        let _ = write!(line, ", severity {severity}");
    }
    if let Some(due) = a["due_at_ns"].as_i64() {
        let _ = write!(line, ", due {}", date(Some(due)));
    }
    if let Some(ticket) = text(a, "ticket") {
        let _ = write!(line, ", ticket {ticket}");
    }
    if let Some(retest) = text(a, "retest_id") {
        let _ = write!(line, ", retested by {} {retest}", or_dash(a, "retest_kind"));
    }
    line
}

/// `what` and its label in brackets, when it has one.
fn labelled(what: String, row: &Value) -> String {
    match text(row, "label") {
        Some(label) => format!("{what} ({label})"),
        None => what,
    }
}

fn link(l: &Value) -> String {
    labelled(
        format!("{} {}", or_dash(l, "kind"), or_dash(l, "linked_id")),
        l,
    )
}

fn attachment(a: &Value) -> String {
    if a["kind"] == "file" {
        return format!(
            "file {}, {}, {} bytes, sha256 {}",
            or_dash(a, "label"),
            or_dash(a, "media_type"),
            a["size_bytes"],
            or_dash(a, "file_hash"),
        );
    }
    labelled(format!("{} {}", or_dash(a, "kind"), or_dash(a, "uri")), a)
}

fn signoff(s: &Value) -> String {
    let mut line = format!(
        "{} by {} at {}",
        or_dash(s, "decision"),
        or_dash(s, "signed_by"),
        time(s["signed_at_ns"].as_i64()),
    );
    if let Some(note) = text(s, "note") {
        let _ = write!(line, ": {note}");
    }
    line
}

/// One record as `GET /api/manual/experiments/{id}` gives it: the record,
/// its parts, and its sign-offs with how many are still needed.
pub fn record(detail: &Value) -> String {
    let mut out = String::new();
    head(&mut out, &detail["experiment"]);
    part(
        &mut out,
        "participants",
        &detail["experiment"]["participants"],
        participant,
    );
    part(&mut out, "steps", &detail["steps"], step);
    part(&mut out, "actions", &detail["actions"], action);
    part(&mut out, "links", &detail["links"], link);
    part(&mut out, "attachments", &detail["attachments"], attachment);
    let signoffs = detail["signoffs"].as_array().map_or(&[][..], Vec::as_slice);
    let approvals = signoffs
        .iter()
        .filter(|s| s["decision"] == "approved")
        .count();
    let _ = writeln!(
        out,
        "sign-offs: {approvals} of {}, {} still needed",
        detail["signoffs_required"], detail["signoffs_needed"],
    );
    for s in signoffs {
        let _ = writeln!(out, "  {}", signoff(s));
    }
    out
}

/// The answer to a sign-off of record `id`: where it stands now.
pub fn signed(id: &str, decision: &str, answer: &Value) -> String {
    let status = or_dash(answer, "status");
    let needed = answer["signoffs_needed"].as_u64().unwrap_or_default();
    if status == "submitted" && needed > 0 {
        let needed = usize::try_from(needed).unwrap_or(usize::MAX);
        return format!(
            "{decision} {id}: status {status}, {} still needed\n",
            counted(needed, "sign-off")
        );
    }
    format!("{decision} {id}: status {status}\n")
}

/// The verdict of the audit verify of record `id`: ok, or each problem on
/// a line of its own.
pub fn verdict(id: &str, verdict: &Value) -> String {
    let problems = verdict["problems"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if verdict["ok"] == true {
        return format!(
            "{id}: ok (seal form {}, {} legacy audit rows)\n",
            verdict["hash_version"], verdict["legacy_rows"]
        );
    }
    let mut out = format!("{id}: NOT ok, {}\n", counted(problems.len(), "problem"));
    for p in problems {
        let field = text(p, "field")
            .map(|f| format!(" {f}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {} {}{field}: {}",
            or_dash(p, "table"),
            or_dash(p, "row"),
            or_dash(p, "problem"),
        );
    }
    out
}

/// The list of plans: a count, then one line per plan.
pub fn plans(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "no plans\n".into();
    }
    let mut out = format!("{}\n", counted(rows.len(), "plan"));
    for row in rows {
        let _ = writeln!(
            out,
            "{}  {}  {}  due {}  {}  {}",
            or_dash(row, "id"),
            or_dash(row, "status"),
            or_dash(row, "exercise_type"),
            date(row["due_at_ns"].as_i64()),
            target(row),
            or_dash(row, "name"),
        );
    }
    out
}

/// One plan as `GET /api/manual/plans/{id}` gives it, with its audit trail.
pub fn plan(detail: &Value) -> String {
    let plan = &detail["plan"];
    let every = plan["interval_days"]
        .as_i64()
        .map_or_else(|| "once".into(), |days| format!("every {days} days"));
    let mut out = format!(
        "{} ({})\n  {}, {}, owner {}\n  due {}, {every}\n  {}\n  RTO planned {}, RPO planned {}\n",
        or_dash(plan, "name"),
        or_dash(plan, "id"),
        or_dash(plan, "exercise_type"),
        or_dash(plan, "status"),
        or_dash(plan, "owner"),
        time(plan["due_at_ns"].as_i64()),
        target(plan),
        seconds(plan, "rto_target_s"),
        seconds(plan, "rpo_target_s"),
    );
    if let Some(scope) = text(plan, "scope") {
        let _ = writeln!(out, "  scope: {scope}");
    }
    part(&mut out, "audit", &detail["audit"], |row| {
        format!(
            "{} {} by {}",
            time(row["changed_at_ns"].as_i64()),
            or_dash(row, "action"),
            or_dash(row, "changed_by"),
        )
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_count_is_in_the_plural_unless_it_is_one() {
        assert_eq!(counted(0, "plan"), "0 plans");
        assert_eq!(counted(1, "plan"), "1 plan");
        assert_eq!(counted(2, "sign-off"), "2 sign-offs");
    }

    #[test]
    fn a_target_names_its_environment_when_it_has_one() {
        assert_eq!(
            target(&json!({"target_system": "ledger", "target_environment": "staging"})),
            "ledger in staging"
        );
        assert_eq!(target(&json!({"target_system": "ledger"})), "ledger");
        assert_eq!(
            target(&json!({"target_environment": "staging"})),
            "- in staging"
        );
        assert_eq!(target(&json!({})), "-");
    }

    #[test]
    fn a_sign_off_says_the_status_and_what_is_still_needed() {
        let one = json!({"status": "submitted", "signoffs_needed": 1});
        assert_eq!(
            signed("r1", "approved", &one),
            "approved r1: status submitted, 1 sign-off still needed\n"
        );
        let done = json!({"status": "verified", "signoffs_needed": 0});
        assert_eq!(
            signed("r1", "approved", &done),
            "approved r1: status verified\n"
        );
    }

    #[test]
    fn a_verdict_that_is_not_ok_lists_each_problem() {
        let found = json!({"ok": false, "problems": [
            {"table": "manual_experiments", "row": "r1", "field": "reviewed_by",
             "problem": "reviewed_by is 'mallory'"},
            {"table": "manual_signoffs", "row": "s1", "field": null, "problem": "broken link"}
        ]});
        assert_eq!(
            verdict("r1", &found),
            "r1: NOT ok, 2 problems\n  manual_experiments r1 reviewed_by: reviewed_by is \
             'mallory'\n  manual_signoffs s1: broken link\n"
        );
        let fine = json!({"ok": true, "hash_version": 2, "legacy_rows": 0, "problems": []});
        assert_eq!(
            verdict("r1", &fine),
            "r1: ok (seal form 2, 0 legacy audit rows)\n"
        );
    }

    #[test]
    fn a_tlpt_record_shows_its_severity_tester_attestation_and_retests() {
        let detail = json!({
            "experiment": {"id": "r2", "experiment_name": "Payments TLPT", "exercise_type": "tlpt",
                           "status": "draft", "outcome_status": "partial", "entered_by": "alice",
                           "executed_at_ns": 1_794_000_000_000_000_000_i64,
                           "findings_severity": "high", "tester": "Red Team Ltd",
                           "tester_kind": "external", "tlpt_attestation_ref": "ATT-1",
                           "tlpt_attested_at_ns": 1_795_000_000_000_000_000_i64,
                           "tlpt_summary_ref": "TSR-1"},
            "steps": [], "links": [], "attachments": [], "signoffs": [],
            "actions": [
                {"ordinal": 1, "title": "Remove stale VPN accounts", "owner": "carol",
                 "status": "open", "severity": "critical", "retest_kind": "run",
                 "retest_id": "exp-1"},
                {"ordinal": 2, "title": "Alert on VPN logins", "owner": "dave", "status": "open"}
            ],
            "signoffs_required": 1, "signoffs_needed": 1
        });
        let text = record(&detail);
        for line in [
            "  findings severity high\n",
            "  tester Red Team Ltd (external)\n",
            "  TLPT attestation ATT-1 of 2026-11-18, test summary report TSR-1\n",
            "  1. Remove stale VPN accounts, owner carol, open, severity critical, \
             retested by run exp-1\n",
            "  2. Alert on VPN logins, owner dave, open\n",
        ] {
            assert!(text.contains(line), "{line:?} not in:\n{text}");
        }
    }

    #[test]
    fn a_record_without_parts_shows_only_its_facts_and_sign_offs() {
        let detail = json!({
            "experiment": {"id": "r1", "experiment_name": "Restore", "exercise_type": "restore",
                           "status": "draft", "outcome_status": "partial", "entered_by": "alice",
                           "executed_at_ns": 1_794_000_000_000_000_000_i64,
                           "recovery_time_s": 90.5, "data_verified": false},
            "steps": [], "actions": [], "links": [], "attachments": [], "signoffs": [],
            "signoffs_required": 1, "signoffs_needed": 1
        });
        assert_eq!(
            record(&detail),
            "Restore (r1)\n  restore, draft, outcome partial\n  executed 2026-11-06T21:20:00Z, -\n  \
             entered by alice\n  RTO planned not set, actual 90.5 s\n  RPO planned not set, \
             actual not set, data loss or corruption found\nsign-offs: 0 of 1, 1 still needed\n"
        );
    }
}
