//! Autopilot tool surface: one pass of the decision loop, the approval
//! queue, status/log readback and the Parquet archive export.
//!
//! Sequencing contract everywhere: the decision (and any human response)
//! is persisted BEFORE the experiment runs. A crash mid-loop leaves the
//! truthful partial record.

#![allow(clippy::missing_errors_doc)]

mod engine;

#[cfg(test)]
mod tests;

use std::{fmt::Write as _, path::Path};

use tumult_autopilot::{LoadedPolicy, Verdict};

use crate::error::ToolError;
use crate::tools::StructuredReport;

use engine::{assemble_candidates, persist_decision, regate_decision};

fn now_ns() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    )
    .unwrap_or(i64::MAX)
}

fn open_store(store_path: &str) -> Result<tumult_lake::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "analytics store not found at {store_path}"
        )));
    }
    tumult_lake::AnalyticsStore::open(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// Open the store read-only for the status/log view: Viewer-gated readback
/// must never take the store's write lock (same pattern as the chaosgraph
/// tools).
fn open_store_ro(store_path: &str) -> Result<tumult_lake::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "analytics store not found at {store_path}"
        )));
    }
    tumult_lake::AnalyticsStore::open_read_only(path).map_err(|e| ToolError::Store(e.to_string()))
}

fn load_policy(policy_path: &str) -> Result<LoadedPolicy, ToolError> {
    let text = std::fs::read_to_string(policy_path)
        .map_err(|e| ToolError::NotFound(format!("policy {policy_path}: {e}")))?;
    LoadedPolicy::parse(&text).map_err(|e| match e {
        // TOML parse errors embed a snippet of the offending file content;
        // a policy file may sit next to sensitive material, so never relay
        // that snippet to the caller — line/column only.
        tumult_autopilot::PolicyError::Parse(_) => {
            let location = toml::from_str::<toml::Value>(&text)
                .err()
                .and_then(|te| te.span())
                .map(|span| line_col_of(&text, span.start));
            location.map_or_else(
                || {
                    ToolError::InvalidInput(format!(
                        "policy {policy_path}: invalid TOML (unknown or malformed key)"
                    ))
                },
                |(line, col)| {
                    ToolError::InvalidInput(format!(
                        "policy {policy_path}: invalid TOML at line {line}, column {col}"
                    ))
                },
            )
        }
        // Range/shape validation errors carry no file content — safe to relay.
        other => ToolError::InvalidInput(other.to_string()),
    })
}

/// 1-based (line, column) of a byte offset in `text` — error positions are
/// safe to relay even when the offending content is not.
fn line_col_of(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Run the playbook experiment for a decision, appending lifecycle events
/// and the graph `enacted` edge. Returns the journal's terminal status.
fn run_playbook(
    store: &tumult_lake::AnalyticsStore,
    store_path: &str,
    decision_id: &str,
    playbook: &str,
    journal_dir: &Path,
) -> Result<String, ToolError> {
    store
        .append_autopilot_event(
            decision_id,
            now_ns(),
            "run_started",
            &serde_json::json!({ "experiment": playbook }),
        )
        .map_err(|e| ToolError::Store(e.to_string()))?;

    std::fs::create_dir_all(journal_dir)?;
    let journal_path = journal_dir.join(format!("{decision_id}.journal.toon"));
    let result = crate::tools::run_experiment(crate::tools::RunExperimentRequest {
        experiment_path: playbook,
        rollback_strategy: "on-deviation",
        journal_path: &journal_path,
        store_path,
        no_ingest: false,
        format: "toon",
        parent_context: None,
    });

    match result {
        Ok(report) => {
            let journal = report.structured.get("journal");
            let status = journal
                .and_then(|j| j.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let experiment_id = journal
                .and_then(|j| j.get("experiment_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // Post-run steady-state is the DiRT rule: rollback of the
            // injection is not recovery of the system.
            let clean = status.eq_ignore_ascii_case("completed");
            store
                .append_autopilot_event(
                    decision_id,
                    now_ns(),
                    if clean { "run_completed" } else { "run_failed" },
                    &serde_json::json!({ "status": status, "experiment_id": experiment_id }),
                )
                .map_err(|e| ToolError::Store(e.to_string()))?;
            if !experiment_id.is_empty() {
                store
                    .record_enacted_edge(decision_id, &experiment_id, now_ns())
                    .map_err(|e| ToolError::Store(e.to_string()))?;
            }
            Ok(status)
        }
        Err(err) => {
            store
                .append_autopilot_event(
                    decision_id,
                    now_ns(),
                    "run_failed",
                    &serde_json::json!({ "error": err.to_string() }),
                )
                .map_err(|e| ToolError::Store(e.to_string()))?;
            Err(err)
        }
    }
}

/// One pass of the loop: assemble, gate, persist every decision, enact the
/// `enact` verdicts (when `execute`).
///
/// `concurrent_experiments` is the server-wide enactment-ledger reading the
/// gate's `ambient.no_concurrent_experiment` rule vets: the caller holding
/// the enactment slot passes 0, every other pass observes 1 while an
/// enactment is in flight and its enact verdicts veto.
pub fn autopilot_once(
    store_path: &str,
    policy_path: &str,
    execute: bool,
    limit: Option<u32>,
    concurrent_experiments: u32,
) -> Result<StructuredReport, ToolError> {
    let policy = load_policy(policy_path)?;
    if !policy.policy.enabled {
        return Err(ToolError::InvalidInput(
            "autopilot is disabled in policy (autopilot.enabled = false)".into(),
        ));
    }
    let store = open_store(store_path)?;
    let now = now_ns();
    let within_hours = within_business_hours_local();
    let limit = limit.unwrap_or(3).clamp(1, 10) as usize;

    let assembled = assemble_candidates(
        &store,
        &policy,
        now,
        within_hours,
        limit,
        concurrent_experiments,
    )?;
    let mut lines = Vec::new();
    let mut decisions = Vec::new();
    let mut enacted = 0u32;

    for item in &assembled {
        // Audit BEFORE act — always, for every verdict.
        persist_decision(&store, &policy, item, now)?;
        let c = &item.candidate;
        let (verdict, detail) = match &item.decision.verdict {
            Verdict::Enact => ("enact", String::new()),
            Verdict::Downgrade { reasons } => ("downgrade", reasons.join("; ")),
            Verdict::Propose { reasons } => ("propose", reasons.join("; ")),
            Verdict::Veto { rule } => ("veto", rule.clone()),
        };
        lines.push(format!(
            "[{verdict}] {} {}::{} for {} (score {:.2}){}",
            c.service_id,
            c.plugin,
            c.action,
            c.article_id,
            c.score,
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        ));

        let mut run_status = None;
        if matches!(item.decision.verdict, Verdict::Enact) && execute {
            let playbook = c.playbook_experiment.clone().ok_or_else(|| {
                ToolError::Execution("enact verdict without playbook (gate bug)".into())
            })?;
            let journal_dir = Path::new(store_path)
                .parent()
                .unwrap_or(Path::new("."))
                .join("autopilot-journals");
            let status = run_playbook(&store, store_path, &c.id, &playbook, &journal_dir)?;
            lines.push(format!("        ran {playbook} -> {status}"));
            enacted += 1;
            run_status = Some(status);
        }
        decisions.push(serde_json::json!({
            "id": c.id, "verdict": verdict, "service": c.service_id,
            "article": c.article_id, "plugin": c.plugin, "action": c.action,
            "score": c.score, "detail": detail, "run_status": run_status,
        }));
    }

    let text = if lines.is_empty() {
        "autopilot pass: no candidates (nothing stale, broken or untested with a playbook)".into()
    } else {
        format!(
            "autopilot pass: {} decision(s), {} enacted (policy {})\n{}",
            decisions.len(),
            enacted,
            policy.policy_hash(),
            lines.join("\n")
        )
    };
    let mut structured = serde_json::Map::new();
    structured.insert("decisions".into(), serde_json::json!(decisions));
    structured.insert("enacted".into(), serde_json::json!(enacted));
    structured.insert(
        "policy_hash".into(),
        serde_json::json!(policy.policy_hash()),
    );
    structured.insert("executed".into(), serde_json::json!(execute));
    Ok(StructuredReport { text, structured })
}

/// Approve or deny a proposed/downgraded decision. Approval runs the
/// playbook; both outcomes are appended as human events (the veto feedback
/// the autonomy ladder consumes).
///
/// Approval is never stale: before the playbook runs, the full gate is
/// re-evaluated against CURRENT state (policy supplied via `policy_path`,
/// hash-checked against the decision record; enrollment, concurrency,
/// cooldown, guard telemetry pre-flight and every other rule re-checked).
/// A veto or downgrade at re-gate refuses execution, records the refusal in
/// the audit trail, and reports why.
pub fn autopilot_respond(
    store_path: &str,
    decision_id: &str,
    approve: bool,
    reason: Option<&str>,
    policy_path: Option<&str>,
    concurrent_experiments: u32,
) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;
    let Some(status) = tumult_query::autopilot_decision(&store, decision_id)
        .map_err(|e| ToolError::Store(e.to_string()))?
    else {
        return Err(ToolError::NotFound(format!("decision {decision_id}")));
    };
    if !matches!(status.record.verdict.as_str(), "propose" | "downgrade") {
        return Err(ToolError::InvalidInput(format!(
            "decision {decision_id} has verdict '{}' — only propose/downgrade take a human response",
            status.record.verdict
        )));
    }
    if matches!(
        status.last_event.as_deref(),
        Some("human_approved" | "human_denied")
    ) {
        return Err(ToolError::InvalidInput(format!(
            "decision {decision_id} already resolved ({})",
            status.last_event.unwrap_or_default()
        )));
    }

    // Approval needs the policy to re-gate against — validate the argument
    // before any event is appended so a usage error never burns the one
    // response a decision takes.
    let policy = if approve {
        let policy_path = policy_path.ok_or_else(|| {
            ToolError::InvalidInput(
                "policy_path is required when approve=true — the gate re-evaluates current \
                 state before the playbook runs"
                    .into(),
            )
        })?;
        Some(load_policy(policy_path)?)
    } else {
        None
    };

    let kind = if approve {
        "human_approved"
    } else {
        "human_denied"
    };
    store
        .append_autopilot_event(
            decision_id,
            now_ns(),
            kind,
            &serde_json::json!({ "reason": reason }),
        )
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut text = format!("{kind}: {decision_id}");
    if let Some(policy) = policy {
        // The re-gate: policy identity first — a decision gated under one
        // policy version may not execute under another.
        if policy.policy_hash() != status.record.policy_hash {
            let detail = format!(
                "policy changed since the decision was recorded (decision hash {}, current hash \
                 {}) — re-run the autopilot pass and approve the fresh decision",
                status.record.policy_hash,
                policy.policy_hash()
            );
            record_regate_outcome(&store, decision_id, "re_gate_refused", &detail, None)?;
            return Err(ToolError::Execution(format!("approval refused: {detail}")));
        }
        let decision = regate_decision(
            &store,
            &policy,
            &status.record,
            now_ns(),
            within_business_hours_local(),
            concurrent_experiments,
        )?;
        if !matches!(decision.verdict, Verdict::Enact) {
            let detail = regate_refusal_detail(&decision);
            record_regate_outcome(
                &store,
                decision_id,
                "re_gate_refused",
                &detail,
                Some(&decision),
            )?;
            return Err(ToolError::Execution(format!(
                "approval refused by gate re-evaluation: {detail}"
            )));
        }
        record_regate_outcome(
            &store,
            decision_id,
            "re_gate_passed",
            "enact",
            Some(&decision),
        )?;

        let playbook = status
            .record
            .playbook
            .clone()
            .ok_or_else(|| ToolError::InvalidInput("decision has no playbook to run".into()))?;
        let journal_dir = Path::new(store_path)
            .parent()
            .unwrap_or(Path::new("."))
            .join("autopilot-journals");
        let run = run_playbook(&store, store_path, decision_id, &playbook, &journal_dir)?;
        let _ = write!(text, "\nran {playbook} -> {run}");
    }

    let mut structured = serde_json::Map::new();
    structured.insert("decision_id".into(), serde_json::json!(decision_id));
    structured.insert("action".into(), serde_json::json!(kind));
    Ok(StructuredReport { text, structured })
}

/// One-line summary of why a re-gated decision must not run.
fn regate_refusal_detail(decision: &tumult_autopilot::GateDecision) -> String {
    match &decision.verdict {
        Verdict::Veto { rule } => format!("gate veto on rule '{rule}'"),
        Verdict::Downgrade { reasons } | Verdict::Propose { reasons } => {
            format!("gate no longer enacts: {}", reasons.join("; "))
        }
        Verdict::Enact => "enact".to_string(),
    }
}

/// Persist the re-gate outcome (pass or refusal) so the audit trail shows
/// not just the human response but the gate's answer at execution time.
fn record_regate_outcome(
    store: &tumult_lake::AnalyticsStore,
    decision_id: &str,
    kind: &str,
    detail: &str,
    decision: Option<&tumult_autopilot::GateDecision>,
) -> Result<(), ToolError> {
    let rules = decision.map_or_else(
        || serde_json::json!([]),
        |d| serde_json::json!(d.rules_evaluated),
    );
    store
        .append_autopilot_event(
            decision_id,
            now_ns(),
            kind,
            &serde_json::json!({ "detail": detail, "rules_evaluated": rules }),
        )
        .map_err(|e| ToolError::Store(e.to_string()))
}

/// Record an external change event (deploy, config change) against a
/// service. The next autopilot pass treats the service's evidence as
/// invalidated (change-triggered revalidation, not just time-triggered).
pub fn autopilot_notify_change(
    store_path: &str,
    service: &str,
    source: &str,
    detail: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;
    store
        .record_change_event(service, now_ns(), source, detail)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let mut structured = serde_json::Map::new();
    structured.insert("service".into(), serde_json::json!(service));
    structured.insert("source".into(), serde_json::json!(source));
    Ok(StructuredReport {
        text: format!("change event recorded: {service} (source: {source})"),
        structured,
    })
}

/// Status/log: decisions with their latest lifecycle event. Opens the
/// store read-only — this is the one Viewer-gated autopilot tool.
pub fn autopilot_status(
    store_path: &str,
    verdict: Option<&str>,
    limit: Option<u32>,
) -> Result<StructuredReport, ToolError> {
    let store = open_store_ro(store_path)?;
    let rows = tumult_query::autopilot_decisions(&store, verdict, u64::from(limit.unwrap_or(20)))
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let mut lines = Vec::new();
    let mut items = Vec::new();
    for row in &rows {
        let r = &row.record;
        lines.push(format!(
            "{}  [{}] {} {}::{} for {} — {}{}",
            &r.id[..8.min(r.id.len())],
            r.verdict,
            r.service_id,
            r.plugin,
            r.action,
            r.article_id,
            r.trigger,
            row.last_event
                .as_deref()
                .map(|e| format!(" · {e}"))
                .unwrap_or_default()
        ));
        items.push(serde_json::json!({
            "id": r.id, "verdict": r.verdict, "service": r.service_id,
            "article": r.article_id, "trigger": r.trigger,
            "policy_hash": r.policy_hash, "last_event": row.last_event,
            "decided_at_ns": r.decided_at_ns,
        }));
    }
    let text = if lines.is_empty() {
        "no autopilot decisions recorded".to_string()
    } else {
        lines.join("\n")
    };
    let mut structured = serde_json::Map::new();
    structured.insert("decisions".into(), serde_json::json!(items));
    structured.insert("count".into(), serde_json::json!(rows.len()));
    Ok(StructuredReport { text, structured })
}

/// Export the decision tables to Parquet under `dir`.
pub fn autopilot_export(store_path: &str, dir: &str) -> Result<StructuredReport, ToolError> {
    let store = open_store(store_path)?;
    store
        .export_autopilot_parquet(Path::new(dir))
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let mut structured = serde_json::Map::new();
    structured.insert("dir".into(), serde_json::json!(dir));
    Ok(StructuredReport {
        text: format!("exported autopilot_decisions.parquet + autopilot_events.parquet to {dir}"),
        structured,
    })
}

/// Local business-hours check (Mon-Fri 09-17 local time). The gate only
/// consults this when the policy demands it.
fn within_business_hours_local() -> bool {
    use chrono::{Datelike, Local, Timelike};
    let now = Local::now();
    let weekday_ok = !matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
    weekday_ok && (9..17).contains(&now.hour())
}
