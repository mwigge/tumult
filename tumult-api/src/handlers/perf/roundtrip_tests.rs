//! A lake rebuilt from its Parquet export alone gives the verdict it
//! gave before: the export carries everything the verdict is computed
//! from, the root span of every run and the version history of the
//! target among it.

use std::path::Path;

use tumult_core::types::{
    ExperimentStatus, Journal, LoadExecution, LoadResult, LoadSample, LoadTool, RequestCounts,
    TargetVersion, VersionSource,
};
use tumult_lake::{AnalyticsStore, SpanRow, Store};

use super::*;

const NOW: i64 = 1_800_000_000 * NS_PER_S;
const FILES: [&str; 6] = [
    "experiments.parquet",
    "activities.parquet",
    "load_results.parquet",
    "probe_samples.parquet",
    "run_roots.parquet",
    "version_sightings.parquet",
];

/// A request `checkout` answered as `version`.
fn request(version: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{version}-{ts}"),
        span_id: format!("span-{version}-{ts}"),
        span_name: "GET /cart".into(),
        span_kind: "Server".into(),
        duration_ns: 1_000_000,
        status_code: "Unset".into(),
        service_name: "checkout".into(),
        service_version: Some(version.into()),
        target_environment: Some("demo".into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

fn root(id: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 60 * NS_PER_S,
        status_code: "Unset".into(),
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        target_system: Some("checkout".into()),
        target_environment: Some("demo".into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

fn window(started: i64, i: u32, p95: f64, timeout_ms: f64) -> LoadSample {
    let jitter = f64::from((i * 7) % 5) - 2.0;
    // Every tenth window's p95 fell among the requests given up on.
    let censored = timeout_ms < 1000.0 && i.is_multiple_of(10);
    LoadSample {
        ts_ns: started + i64::from(i) * NS_PER_S,
        duration_s: 1.0,
        vus: 10,
        throughput_rps: 100.0,
        latency_p50_ms: p95 / 2.0,
        latency_p95_ms: if censored { timeout_ms } else { p95 + jitter },
        latency_p99_ms: p95 * 1.5,
        error_rate: 0.0,
        requests: 100,
        counts: None,
        request_timeout_ms: Some(timeout_ms),
    }
}

/// A run that declared `version`, with what became of its requests.
fn run(id: &str, version: &str, started: i64, p95: f64, ok: u64, timeout_ms: f64) -> Journal {
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, ExperimentStatus::Completed);
    journal.started_at_ns = started;
    journal.ended_at_ns = started + 60 * NS_PER_S;
    journal.target_version = Some(TargetVersion {
        version: version.into(),
        version_source: VersionSource::Declared,
        shares: Vec::new(),
    });
    journal.load_result = Some(LoadResult {
        tool: LoadTool::Oha,
        started_at_ns: started,
        ended_at_ns: started + 60 * NS_PER_S,
        duration_s: 60.0,
        vus: 10,
        throughput_rps: 100.0,
        latency_p50_ms: p95 / 2.0,
        latency_p95_ms: p95,
        latency_p99_ms: p95 * 1.5,
        error_rate: 0.0,
        total_requests: 4000,
        thresholds_met: true,
        samples: (0..40)
            .map(|i| window(started, i, p95, timeout_ms))
            .collect(),
        execution: LoadExecution {
            requests: Some(RequestCounts {
                sent: 4000,
                ok,
                timed_out: 4000 - ok,
                ..RequestCounts::default()
            }),
            ..LoadExecution::default()
        },
    });
    journal
}

/// Three runs of 1.0.0 that answered everything in 100 ms, then three of
/// 1.1.0 that answered six requests in ten, in 20 ms.
fn evidence() -> (Vec<SpanRow>, Vec<Journal>) {
    let sides = [
        ("old", "1.0.0", NOW - 2500 * NS_PER_S, 100.0, 4000, 2000.0),
        ("new", "1.1.0", NOW - 1000 * NS_PER_S, 20.0, 2400, 500.0),
    ];
    let mut spans = Vec::new();
    let mut journals = Vec::new();
    for (prefix, version, first, p95, ok, timeout_ms) in sides {
        for i in 0..3 {
            let id = format!("{prefix}-{i}");
            let started = first + i * 100 * NS_PER_S;
            spans.push(root(&id, started));
            spans.push(request(version, started + NS_PER_S));
            journals.push(run(&id, version, started, p95, ok, timeout_ms));
        }
    }
    (spans, journals)
}

fn write(lake: &Path, spans: &[SpanRow], journals: &[Journal]) {
    let writer = Store::open(lake).unwrap().writer().unwrap();
    writer.insert_spans(spans).unwrap();
    for journal in journals {
        writer.ingest_journal(journal, None).unwrap();
    }
}

fn export(lake: &Path, dir: &Path) {
    let [experiments, activities, loads, samples, roots, sightings] =
        FILES.map(|file| dir.join(file));
    let store = AnalyticsStore::open(lake).unwrap();
    store.export_tables(&experiments, &activities).unwrap();
    store.export_load_tables(&loads, &samples).unwrap();
    store.export_verdict_tables(&roots, &sightings).unwrap();
}

fn import(lake: &Path, dir: &Path) {
    let [experiments, activities, loads, samples, roots, sightings] =
        FILES.map(|file| dir.join(file));
    let store = AnalyticsStore::open(lake).unwrap();
    store.import_tables(&experiments, &activities).unwrap();
    store.import_load_tables(&loads, &samples).unwrap();
    store.import_verdict_tables(&roots, &sightings).unwrap();
}

fn history(lake: &Path) -> Vec<tumult_lake::VersionSighting> {
    let reader = Store::at(lake).read_only().unwrap();
    reader.version_sightings("checkout").unwrap()
}

fn verdict(lake: &Path, probe: &str) -> Value {
    let req = Request {
        target: "checkout".into(),
        probe: probe.into(),
        opts: CompareOptions::default(),
        env: None,
        scopes: Vec::new(),
        baseline: None,
        candidate: None,
        range: "30d".into(),
        since_ns: NOW - 30 * 86_400 * NS_PER_S,
        as_of_ns: NOW,
        include_partial: false,
    };
    let reader = Store::at(lake).read_only().unwrap();
    let Ok(answer) = decide(&reader, &req).unwrap() else {
        panic!("no answer for {probe}");
    };
    answer.json(false, false)
}

#[test]
fn the_verdict_is_identical_after_an_export_and_an_import() {
    let dir = tempfile::TempDir::new().unwrap();
    let (first, second) = (dir.path().join("a.duckdb"), dir.path().join("b.duckdb"));
    let (spans, journals) = evidence();
    write(&first, &spans, &journals);
    export(&first, dir.path());
    // The second lake has nothing but what the export carries; importing
    // it a second time changes nothing.
    import(&second, dir.path());
    import(&second, dir.path());
    assert_eq!(history(&first).len(), 2);
    assert_eq!(history(&second), history(&first));

    for probe in [DEFAULT_PROBE, "load.throughput_rps"] {
        assert_eq!(verdict(&second, probe), verdict(&first, probe), "{probe}");
    }
    // And it is a verdict that rests on what the new columns carry: the
    // recorded versions, the request counts, the request timeout.
    let answer = verdict(&second, DEFAULT_PROBE);
    assert_eq!(answer["verdict"], "regressed", "{answer}");
    assert_eq!(answer["reason"], "failure_share", "{answer}");
    assert_eq!(answer["candidate"]["version"], "1.1.0", "{answer}");
    assert_eq!(answer["candidate"]["attribution"]["declared"], 3);
    assert_eq!(answer["candidate"]["requests"]["failed"], 4800);
    assert_eq!(answer["candidate"]["windows"]["censored"], 12);
}
