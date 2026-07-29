//! Render sample R1/R3/R2 PDFs from a seeded fixture store into
//! `/tmp/krk/out/` — the visual-iteration loop for the report pipeline.
//!
//! Run: `cargo run -p kronika-docs --example render_samples`

use std::time::{SystemTime, UNIX_EPOCH};

use kronika_docs::builders;
use kronika_docs::typst_pdf::render_pdf;
use kronika_store::{LogRow, SpanRow, Store};

const NS: i64 = 1_000_000_000;
const DAY: i64 = 86_400 * NS;

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

fn span(ts: i64, name: &str) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: "t".into(),
        span_id: "s".into(),
        parent_span_id: None,
        span_name: name.into(),
        span_kind: "Internal".into(),
        duration_ns: NS,
        status_code: "Unset".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: None,
        experiment_id: None,
        experiment_name: None,
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: None,
        target_technology: None,
        target_environment: None,
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
}

/// One experiment run: root span + started/completed logs.
fn run(
    spans: &mut Vec<SpanRow>,
    logs: &mut Vec<LogRow>,
    id: &str,
    name: &str,
    ts: i64,
    outcome: &str,
) {
    let mut root = span(ts, "resilience.experiment");
    root.trace_id = format!("trace-{id}");
    root.span_id = format!("span-{id}-root");
    root.experiment_id = Some(id.into());
    root.experiment_name = Some(name.into());
    spans.push(root);
    for (body, status, off) in [
        ("experiment.started", None, 0),
        ("experiment.completed", Some(outcome), 5 * NS),
    ] {
        logs.push(LogRow {
            ts_ns: ts + off,
            severity_text: "INFO".into(),
            body: body.into(),
            trace_id: Some(format!("trace-{id}")),
            span_id: None,
            service_name: "tumult".into(),
            log_attrs: [
                Some(("experiment_id".into(), id.into())),
                status.map(|s| ("status".into(), s.into())),
            ]
            .into_iter()
            .flatten()
            .collect(),
            resource_attrs: vec![],
        });
    }
}

fn main() {
    let dir = std::path::Path::new("/tmp/krk/out");
    std::fs::create_dir_all(dir).unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("fixture.duckdb");
    let now = now_ns();

    let mut spans = Vec::new();
    let mut logs = Vec::new();

    // Recent window (last 7d): 8 experiments — 6 green, 1 deviated, 1 failed.
    let recent: &[(&str, &str, &str, i64)] = &[
        (
            "e1",
            "config corruption — invalid config rejected",
            "Deviated",
            2 * DAY,
        ),
        (
            "e2",
            "message-queue dependency loss — restart fails",
            "Failed",
            DAY,
        ),
        (
            "e3",
            "api-worker freeze — heartbeat recovers after SIGSTOP injection",
            "Completed",
            2 * DAY,
        ),
        (
            "e4",
            "batch worker pool — all jobs complete without strays",
            "Completed",
            3 * DAY,
        ),
        (
            "e5",
            "disk pressure — usage detected and reclaimed",
            "Completed",
            4 * DAY,
        ),
        (
            "e6",
            "host cpu burn — shell stays responsive",
            "Completed",
            5 * DAY,
        ),
        (
            "e7",
            "service partition — endpoint loss detected and healed",
            "Completed",
            6 * DAY,
        ),
        (
            "e8",
            "worker-process kill — disruption detected and removed",
            "Completed",
            DAY,
        ),
    ];
    for (id, name, outcome, age) in recent {
        run(&mut spans, &mut logs, id, name, now - age, outcome);
    }
    // The deviated one later ran green → counts as "fixed".
    run(
        &mut spans,
        &mut logs,
        "e1b",
        "config corruption — invalid config rejected",
        now - DAY,
        "Completed",
    );
    // Stale: last pass 45d ago; never-run-in-window but on record.
    run(
        &mut spans,
        &mut logs,
        "e9",
        "legacy batch sweep — quarterly job",
        now - 45 * DAY,
        "Completed",
    );

    // History for the trend: monthly runs over 6 months, weaker in the past.
    let history: &[(&str, &str, i64, &str)] = &[
        (
            "e3",
            "api-worker freeze — heartbeat recovers after SIGSTOP injection",
            30 * DAY,
            "Completed",
        ),
        (
            "e4",
            "batch worker pool — all jobs complete without strays",
            30 * DAY,
            "Completed",
        ),
        (
            "e1",
            "config corruption — invalid config rejected",
            30 * DAY,
            "Deviated",
        ),
        (
            "e3",
            "api-worker freeze — heartbeat recovers after SIGSTOP injection",
            60 * DAY,
            "Completed",
        ),
        (
            "e2",
            "message-queue dependency loss — restart fails",
            60 * DAY,
            "Failed",
        ),
        (
            "e5",
            "disk pressure — usage detected and reclaimed",
            90 * DAY,
            "Completed",
        ),
        (
            "e1",
            "config corruption — invalid config rejected",
            120 * DAY,
            "Failed",
        ),
        (
            "e6",
            "host cpu burn — shell stays responsive",
            150 * DAY,
            "Deviated",
        ),
        (
            "e7",
            "service partition — endpoint loss detected and healed",
            180 * DAY,
            "Completed",
        ),
    ];
    for (i, (_id, name, age, outcome)) in history.iter().enumerate() {
        run(
            &mut spans,
            &mut logs,
            &format!("h{i}"),
            name,
            now - age,
            outcome,
        );
    }

    // Game-day fixture: e1's deviated run gets a full span tree + findings.
    let gd = "e1";
    let base = now - 2 * DAY;
    let children: &[(&str, i64, &str, i64)] = &[
        ("resilience.hypothesis.before", 0, "Ok", 300_000_000),
        ("resilience.probe", 100_000_000, "Ok", 300_000_000),
        ("resilience.action", 150_000_000, "Error", 900_000),
        ("resilience.probe", 200_000_000, "Ok", 350_000_000),
        (
            "resilience.hypothesis.after",
            800_000_000,
            "Ok",
            300_000_000,
        ),
        ("resilience.rollback", 900_000_000, "Ok", 120_000_000),
    ];
    for (i, (name, off, status, dur)) in children.iter().enumerate() {
        let mut s = span(base + off, name);
        s.trace_id = format!("trace-{gd}");
        s.span_id = format!("span-{gd}-{i}");
        s.parent_span_id = Some(format!("span-{gd}-root"));
        s.status_code = (*status).into();
        s.duration_ns = *dur;
        if *name == "resilience.action" {
            s.fault_type = Some("corruption".into());
            s.fault_subtype = Some("config-rewrite".into());
        }
        spans.push(s);
    }
    // Root span enrichment for the game-day header.
    let root = spans
        .iter_mut()
        .find(|s| s.experiment_id.as_deref() == Some(gd))
        .unwrap();
    root.fault_type = Some("corruption".into());
    root.fault_subtype = Some("config-rewrite".into());
    root.fault_severity = Some("high".into());
    root.blast_radius = Some("single-host".into());
    root.hypothesis_met = Some(false);
    root.recovery_time_s = Some(42.0);
    root.span_attrs = vec![
        ("tumult.scenario".into(), "config-corruption".into()),
        ("tumult.mode".into(), "attack-then-rollback".into()),
    ];
    logs.push(LogRow {
        ts_ns: base + 250_000_000,
        severity_text: "WARN".into(),
        body: "probe latency above steady-state band (p95 812ms > 500ms)".into(),
        trace_id: Some(format!("trace-{gd}")),
        span_id: None,
        service_name: "tumult".into(),
        log_attrs: vec![("experiment_id".into(), gd.into())],
        resource_attrs: vec![],
    });
    logs.push(LogRow {
        ts_ns: base + 700_000_000,
        severity_text: "ERROR".into(),
        body: "service rejected config: unknown key 'cache.stratgy'".into(),
        trace_id: Some(format!("trace-{gd}")),
        span_id: None,
        service_name: "app".into(),
        log_attrs: vec![],
        resource_attrs: vec![],
    });

    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    writer.insert_spans(&spans).unwrap();
    writer.insert_logs(&logs).unwrap();
    drop(writer);
    drop(store);

    let store = Store::at(&db);
    let reader = store.read_only().unwrap();

    let period = 7 * DAY;
    let docs = [
        (
            "r1",
            builders::build_executive(&reader, now, period, now).unwrap(),
        ),
        (
            "r3",
            builders::build_game_day(&reader, gd, now).unwrap().unwrap(),
        ),
        (
            "r2",
            builders::build_evidence_pack(&reader, "dora", Some(14 * DAY), now).unwrap(),
        ),
    ];
    for (tag, doc) in &docs {
        let pdf = render_pdf(doc).unwrap();
        let path = dir.join(format!("{tag}.pdf"));
        std::fs::write(&path, &pdf).unwrap();
        std::fs::write(
            dir.join(format!("{tag}.html")),
            kronika_docs::html::render(doc),
        )
        .unwrap();
        println!(
            "{}: {} ({} bytes)",
            path.display(),
            doc.meta.doc_id,
            pdf.len()
        );
    }
}
