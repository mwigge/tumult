//! What entity events cost the single writer when the table is as full as
//! the limits let a sender make it. Prints the time of one-event exports,
//! each in a transaction of its own as the daemon stores them.
//!
//! ```text
//! cargo run -p tumult-lake --example entity_writer_cost -- <directory>
//! ```
//!
//! The directory receives a lake file and must not hold one. The figures
//! in `docs/guides/entity-events.md` (section 6) come from this program.

#![allow(clippy::print_stdout)] // an example: the measurements are its output

use std::time::{Duration, Instant};

use serde_json::json;
use tumult_lake::{
    EntityEventKind, EntityEventRow, EntityLimits, Store, WireForm, Writer,
    ENTITIES_PER_ENVIRONMENT,
};

/// 2100-01-01: ahead of the clock, so every entity stored here is live.
const RECEIVED: i64 = 4_102_444_800_000_000_000;
/// 2026-01-01: long ago, so an entity last heard of here has expired.
const LONG_AGO: i64 = 1_767_225_600_000_000_000;
/// One-event exports timed per case.
const EXPORTS: usize = 200;
/// Events per export while the table is filled.
const FILL_BATCH: usize = 5_000;
/// Environments one key is planted in.
const PLANTED: usize = 100_000;

fn host(id: &str, environment: &str, ts_ns: i64) -> EntityEventRow {
    EntityEventRow {
        kind: EntityEventKind::State,
        ts_ns,
        received_at_ns: ts_ns,
        entity_type: "host".into(),
        entity_id: vec![("host.id".into(), id.into())],
        description: json!({"host.name": id, "os.type": "linux"}),
        relationships: json!([]),
        interval_s: Some(60),
        delete_reason: None,
        wire_form: WireForm::Spec,
        observer: "token:default".into(),
        schema_url: None,
        environment: environment.into(),
    }
}

fn fill(writer: &Writer, limits: &EntityLimits, events: &[EntityEventRow]) {
    for batch in events.chunks(FILL_BATCH) {
        writer
            .apply_entity_events(batch, limits)
            .expect("the fill is stored");
    }
}

/// Store each event as an export of its own; the times, shortest first.
fn timed(writer: &Writer, limits: &EntityLimits, events: &[EntityEventRow]) -> Vec<Duration> {
    let mut times: Vec<Duration> = events
        .iter()
        .map(|event| {
            let started = Instant::now();
            writer
                .apply_entity_events(std::slice::from_ref(event), limits)
                .expect("the export is stored");
            started.elapsed()
        })
        .collect();
    times.sort_unstable();
    times
}

fn report(case: &str, times: &[Duration]) {
    let median = times[times.len() / 2];
    let slowest = times[times.len() - 1];
    println!(
        "{case}: median {:.2} ms, slowest {:.2} ms over {} exports",
        median.as_secs_f64() * 1e3,
        slowest.as_secs_f64() * 1e3,
        times.len()
    );
}

fn unknown(environment: &str, tag: &str) -> Vec<EntityEventRow> {
    (0..EXPORTS)
        .map(|n| host(&format!("{tag}-{n}"), environment, RECEIVED + 1))
        .collect()
}

fn main() {
    let directory = std::env::args().nth(1).expect("a directory for the lake");
    let store =
        Store::open(&std::path::Path::new(&directory).join("cost.duckdb")).expect("the lake opens");
    let writer = store.writer().expect("the writer opens");
    let limits = EntityLimits::default();

    report(
        "unknown id, empty table (opened)",
        &timed(&writer, &limits, &unknown("small", "first")),
    );
    report(
        "heartbeat of a known entity, 200 entities in the table",
        &timed(&writer, &limits, &unknown("small", "first")),
    );

    let full: Vec<EntityEventRow> = (0..ENTITIES_PER_ENVIRONMENT)
        .map(|n| host(&format!("h-{n}"), "prod", RECEIVED))
        .collect();
    let started = Instant::now();
    fill(&writer, &limits, &full);
    println!(
        "filled prod with {ENTITIES_PER_ENVIRONMENT} entities in {:.1} s",
        started.elapsed().as_secs_f64()
    );

    let heartbeats: Vec<EntityEventRow> = (0..EXPORTS)
        .map(|n| host(&format!("h-{n}"), "prod", RECEIVED + 1))
        .collect();
    report(
        "heartbeat of a known entity",
        &timed(&writer, &limits, &heartbeats),
    );
    report(
        "unknown id, environment at its limit (refused)",
        &timed(&writer, &limits, &unknown("prod", "refused")),
    );
    report(
        "unknown id, environment with room (opened)",
        &timed(&writer, &limits, &unknown("dev", "opened")),
    );

    let planted: Vec<EntityEventRow> = (0..PLANTED)
        .map(|n| host("everywhere", &format!("env-{n}"), RECEIVED))
        .collect();
    fill(&writer, &limits, &planted);
    let one: Vec<EntityEventRow> = (1..=EXPORTS)
        .map(|n| {
            host(
                "everywhere",
                "env-7",
                RECEIVED + i64::try_from(n).unwrap_or(0),
            )
        })
        .collect();
    report(
        &format!("heartbeat of a key that is in {PLANTED} environments"),
        &timed(&writer, &limits, &one),
    );
    report(
        "unknown id, environment at its limit, 150 000 entities in the lake",
        &timed(&writer, &limits, &unknown("prod", "refused-later")),
    );

    let stale: Vec<EntityEventRow> = (0..ENTITIES_PER_ENVIRONMENT)
        .map(|n| host(&format!("s-{n}"), "stale", LONG_AGO))
        .collect();
    fill(&writer, &limits, &stale);
    report(
        "unknown id, environment at its limit with expired entities (the slowest evicts 500)",
        &timed(&writer, &limits, &unknown("stale", "evicting")),
    );

    let started = Instant::now();
    let rows = writer.entity_version_count().expect("the rows are counted");
    println!(
        "count of {rows} rows: {:.2} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    let started = Instant::now();
    let pruned = writer
        .prune_entities(0, 5_000)
        .expect("the retention look runs");
    println!(
        "retention look that removes {pruned} rows: {:.2} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    let started = Instant::now();
    let pruned = writer
        .prune_entities(LONG_AGO + 1, 5_000)
        .expect("the retention look runs");
    println!(
        "retention look that removes {pruned} rows: {:.2} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    let started = Instant::now();
    let purged = writer.purge_entities("prod", None).expect("the purge runs");
    println!(
        "purge of an environment with {} entities: {:.2} ms",
        purged.entities,
        started.elapsed().as_secs_f64() * 1e3
    );
}
