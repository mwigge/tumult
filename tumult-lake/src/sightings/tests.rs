use super::*;

const S: i64 = SECOND_NS;
/// Where these tests begin to count: the earliest time a span may bear
/// and still be a sight. The helpers below take and give times from
/// here, so a test reads in seconds from nought.
const T0: i64 = EARLIEST_SIGHT_NS;
/// A wall clock far past every span these tests store.
const NOW: i64 = 1_000_000_000 * S;

fn span(service: &str, version: Option<&str>, env: Option<&str>, ts_ns: i64) -> SpanRow {
    let ts_ns = T0.saturating_add(ts_ns);
    SpanRow {
        ts_ns,
        trace_id: format!("t-{service}-{ts_ns}"),
        span_id: format!("s-{ts_ns}"),
        span_name: "GET /".into(),
        span_kind: "SERVER".into(),
        status_code: "OK".into(),
        service_name: service.into(),
        service_version: version.map(str::to_string),
        target_environment: env.map(str::to_string),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// A `checkout` span in demo.
fn demo_span(version: &str, at_s: i64) -> SpanRow {
    span("checkout", Some(version), Some("demo"), at_s * S)
}

/// A span inside a request: it has a parent and is no server span.
fn inner(mut row: SpanRow, n: i64) -> SpanRow {
    row.span_kind = "INTERNAL".into();
    row.parent_span_id = Some("parent".into());
    row.span_id = format!("{}-inner-{n}", row.span_id);
    row
}

fn lake() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("lake.duckdb")).unwrap();
    (dir, store)
}

fn sightings(store: &Store, service: &str) -> Vec<VersionSighting> {
    let mut sightings = store
        .read_only()
        .unwrap()
        .version_sightings(service)
        .unwrap();
    for sighting in &mut sightings {
        sighting.first_seen_ns -= T0;
        sighting.last_seen_ns -= T0;
        sighting.last_deployed_ns -= T0;
    }
    sightings
}

type Seen = (Option<String>, String, i64, i64);

fn seen(store: &Store, service: &str) -> Vec<Seen> {
    sightings(store, service)
        .into_iter()
        .map(|s| (s.environment, s.version, s.first_seen_ns, s.last_seen_ns))
        .collect()
}

fn demo(version: &str, first: i64, last: i64) -> Seen {
    (Some("demo".into()), version.into(), first * S, last * S)
}

/// (version, latest deployment in seconds, deployments) of `checkout`.
fn deployments(store: &Store) -> Vec<(String, i64, u32)> {
    sightings(store, "checkout")
        .into_iter()
        .map(|s| (s.version, s.last_deployed_ns / S, s.deployments))
        .collect()
}

fn share(version: &str, requests: u64) -> VersionShare {
    VersionShare {
        version: version.into(),
        requests,
    }
}

/// How many rows the rollup table itself holds.
fn rollup_rows(store: &Store) -> i64 {
    store
        .writer()
        .unwrap()
        .conn
        .query_row(
            "SELECT count(*) FROM service_version_sightings",
            params![],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn inserting_spans_keeps_first_and_last_sight_per_environment() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            span("checkout", Some("1.0.0"), Some("demo"), 20 * S),
            span("checkout", Some("1.0.0"), Some("demo"), 10 * S),
            span("checkout", Some("1.0.0"), Some("prod"), 15 * S),
            span("checkout", Some("1.1.0"), None, 30 * S),
            span("checkout", None, Some("demo"), 99 * S),
            span("ledger", Some("3.0.0"), Some("demo"), 5 * S),
        ])
        .unwrap();
    // A later batch widens what is there, in both directions.
    writer
        .insert_spans(&[
            span("checkout", Some("1.0.0"), Some("demo"), 40 * S),
            span("checkout", Some("1.0.0"), Some("demo"), 4 * S),
            span("checkout", Some("1.1.0"), None, 31 * S),
        ])
        .unwrap();
    drop(writer);
    assert_eq!(
        seen(&store, "checkout"),
        vec![
            demo("1.0.0", 4, 40),
            (Some("prod".into()), "1.0.0".into(), 15 * S, 15 * S),
            (None, "1.1.0".into(), 30 * S, 31 * S),
        ]
    );
    assert_eq!(seen(&store, "ledger").len(), 1);
    assert!(seen(&store, "nobody").is_empty());
    // One deployment each, begun at the first sight, late spans included.
    assert!(sightings(&store, "checkout")
        .iter()
        .all(|s| s.deployments == 1 && s.last_deployed_ns == s.first_seen_ns && !s.redeployed()));
}

#[test]
fn a_version_that_returns_after_another_spoke_is_deployed_again() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[demo_span("1.4.0", 0), demo_span("1.4.0", 100)])
        .unwrap();
    writer
        .insert_spans(&[demo_span("1.5.0", 200), demo_span("1.5.0", 2000)])
        .unwrap();
    // The rollback: 1.4.0 speaks again, long after it went quiet.
    writer
        .insert_spans(&[demo_span("1.4.0", 3000), demo_span("1.4.0", 3100)])
        .unwrap();
    drop(writer);

    assert_eq!(
        deployments(&store),
        vec![
            ("1.4.0".to_string(), 3000, 2),
            ("1.5.0".to_string(), 200, 1)
        ]
    );
    let rolled_back = &sightings(&store, "checkout")[0];
    assert!(rolled_back.redeployed());
    assert_eq!(rolled_back.first_seen_ns, 0);
    assert_eq!(rolled_back.last_seen_ns, 3100 * S);
}

#[test]
fn one_batch_that_spans_a_rollback_still_shows_the_redeploy() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            demo_span("1.4.0", 3000),
            demo_span("1.5.0", 2000),
            demo_span("1.4.0", 0),
            demo_span("1.5.0", 200),
            demo_span("1.4.0", 100),
        ])
        .unwrap();
    drop(writer);
    assert_eq!(
        deployments(&store),
        vec![
            ("1.4.0".to_string(), 3000, 2),
            ("1.5.0".to_string(), 200, 1)
        ]
    );
}

#[test]
fn silence_alone_or_two_versions_side_by_side_is_no_redeploy() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    // A quiet night: nobody else spoke.
    writer.insert_spans(&[demo_span("1.4.0", 0)]).unwrap();
    writer.insert_spans(&[demo_span("1.4.0", 50_000)]).unwrap();
    // A canary: both speak, neither is silent past the gap.
    for at in [50_100, 50_400, 50_700, 51_000] {
        writer
            .insert_spans(&[demo_span("1.5.0", at), demo_span("1.4.0", at + 50)])
            .unwrap();
    }
    // What spoke in another environment meanwhile is another history.
    writer
        .insert_spans(&[span("checkout", Some("1.3.0"), Some("prod"), 60_000 * S)])
        .unwrap();
    writer.insert_spans(&[demo_span("1.5.0", 70_000)]).unwrap();
    // Spans that arrive late, from before the newer version was first
    // seen, are the old deployment still.
    let ledger = |version: &str, at: i64| span("ledger", Some(version), Some("demo"), at * S);
    writer.insert_spans(&[ledger("1.0.0", 0)]).unwrap();
    writer.insert_spans(&[ledger("2.0.0", 5000)]).unwrap();
    writer.insert_spans(&[ledger("1.0.0", 2000)]).unwrap();
    drop(writer);

    for service in ["checkout", "ledger"] {
        let all = sightings(&store, service);
        assert!(all.iter().all(|s| s.deployments == 1), "{all:?}");
        assert!(all.iter().all(|s| s.last_deployed_ns == s.first_seen_ns));
    }
}

#[test]
fn a_span_with_an_early_timestamp_moves_a_deployment_start_only_so_far() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[demo_span("1.4.0", 10_000)]).unwrap();
    writer.insert_spans(&[demo_span("1.5.0", 20_000)]).unwrap();
    // A span delivered late, from five minutes before the first one seen.
    writer.insert_spans(&[demo_span("1.5.0", 19_700)]).unwrap();
    // A clock gone wrong: 1.5.0 at a time before 1.4.0 was ever deployed.
    writer.insert_spans(&[demo_span("1.5.0", 0)]).unwrap();
    writer.insert_spans(&[demo_span("1.5.0", 20_100)]).unwrap();
    drop(writer);

    assert_eq!(
        deployments(&store),
        [
            ("1.4.0".to_string(), 10_000, 1),
            ("1.5.0".to_string(), 19_700, 1)
        ]
    );
    assert_eq!(
        seen(&store, "checkout"),
        [demo("1.4.0", 10_000, 10_000), demo("1.5.0", 19_700, 20_100)]
    );
}

#[test]
fn the_rollup_holds_one_row_for_each_service_environment_and_version() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    for round in 0..4 {
        writer
            .insert_spans(&[
                demo_span("1.4.0", round * 10),
                demo_span("1.5.0", round * 10 + 1),
                span("checkout", Some("1.4.0"), None, round * S),
                span("checkout", Some("1.4.0"), Some("prod"), round * S),
                span("ledger", Some("1.4.0"), Some("demo"), round * S),
            ])
            .unwrap();
    }
    writer.rebuild_version_sightings().unwrap();
    writer.insert_spans(&[demo_span("1.4.0", 99)]).unwrap();
    drop(writer);
    assert_eq!(rollup_rows(&store), 5);
}

#[test]
fn a_version_from_a_span_is_stored_within_the_bounds_and_flagged() {
    let (_dir, store) = lake();
    let hostile = format!("1.0\n<img src=x>\u{1b}[31m{}", "9".repeat(4000));
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            span("checkout", Some(&hostile), Some("demo"), 10 * S),
            span("checkout", Some("1.5.0"), Some("demo"), 20 * S),
        ])
        .unwrap();
    let stored: Vec<String> = {
        let mut stmt = writer
            .conn
            .prepare("SELECT service_version FROM spans ORDER BY ts_ns")
            .unwrap();
        let rows = stmt.query_map(params![], |row| row.get(0)).unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    };
    drop(writer);

    assert_eq!(stored[0].len(), MAX_VERSION_LEN);
    assert!(stored[0].starts_with("1.0<img src=x>[31m999"), "{stored:?}");
    assert_eq!(stored[1], "1.5.0");
    let all = sightings(&store, "checkout");
    assert_eq!(all[0].version, stored[0]);
    assert!(all[0].sanitized);
    assert!(!all[1].sanitized);
}

/// As a build before the rollup knew deployments left the lake: spans,
/// and no record of a backfill.
fn as_an_older_build_left_it(db: &Path) {
    duckdb::Connection::open(db)
        .unwrap()
        .execute_batch(
            "DROP TABLE IF EXISTS service_version_sightings;
             DROP TABLE IF EXISTS sightings_backfill;",
        )
        .unwrap();
}

/// Open the lake and let its backfill end. Opening gives a backfill
/// [`BACKFILL_OPEN_BUDGET`] of wall clock and leaves the rest to later
/// turns, so a busy machine can open the lake with the last step undone.
fn open_and_let_the_backfill_end(db: &Path) -> Store {
    let store = Store::open(db).unwrap();
    assert!(store
        .writer()
        .unwrap()
        .continue_version_backfill(Duration::from_secs(600))
        .unwrap());
    store
}

#[test]
fn a_lake_written_before_the_rollup_is_backfilled_once_on_open() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    {
        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                demo_span("1.0.0", 10),
                demo_span("1.0.0", 50),
                demo_span("1.1.0", 60),
                demo_span("1.1.0", 5000),
                demo_span("1.0.0", 9000),
            ])
            .unwrap();
    }
    as_an_older_build_left_it(&db);

    let store = open_and_let_the_backfill_end(&db);
    assert_eq!(
        seen(&store, "checkout"),
        vec![demo("1.0.0", 10, 9000), demo("1.1.0", 60, 5000)]
    );
    // The history is read with its deployments.
    assert_eq!(
        deployments(&store),
        vec![("1.0.0".to_string(), 9000, 2), ("1.1.0".to_string(), 60, 1)]
    );
    assert!(!store
        .read_only()
        .unwrap()
        .version_backfill_running()
        .unwrap());

    // It ran once: a rollup emptied afterwards is not filled again by
    // opening the lake.
    store
        .writer()
        .unwrap()
        .conn
        .execute_batch("DELETE FROM service_version_sightings")
        .unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    assert!(seen(&store, "checkout").is_empty());
}

#[test]
fn a_backfill_reads_a_day_at_a_step_and_says_while_it_is_running() {
    const DAY: i64 = 86_400;
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    {
        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[
                demo_span("1.0.0", 100),
                demo_span("1.1.0", 3 * DAY),
                demo_span("1.2.0", 6 * DAY),
            ])
            .unwrap();
    }
    as_an_older_build_left_it(&db);
    let conn = duckdb::Connection::open(&db).unwrap();
    for ddl in crate::schema::CREATE_TABLES {
        conn.execute_batch(ddl).unwrap();
    }
    let versions = |conn: &duckdb::Connection| -> Vec<String> {
        held_of(conn, "checkout")
            .unwrap()
            .into_keys()
            .map(|(_, version)| version)
            .collect()
    };

    // One step reads one day and leaves the rest.
    in_tx(&conn, || backfill_restart(&conn)).unwrap();
    assert!(!in_tx(&conn, || backfill_step(&conn, None, NOW)).unwrap());
    assert_eq!(versions(&conn), ["1.0.0"]);
    assert!(matches!(
        backfill_state(&conn).unwrap(),
        Backfill::Running(cursor) if cursor == T0 + (100 + DAY) * S
    ));

    // A batch stored meanwhile: what the backfill has passed is folded in
    // at once, what it has yet to reach is left to it, and the batch
    // moves it a step on.
    in_tx(&conn, || {
        note_sightings(
            &conn,
            &[demo_span("0.9.0", 50), demo_span("2.0.0", 9 * DAY)],
            NOW,
        )
        .map_err(|e| duckdb::Error::InvalidParameterName(e.to_string()))
    })
    .unwrap();
    assert_eq!(versions(&conn), ["0.9.0", "1.0.0", "1.1.0"]);
    let before_opening = backfill_state(&conn).unwrap();
    drop(conn);

    // Opening the lake carries it on, and a later turn takes what the
    // budget of the opening left. The spans of the batch above were
    // never stored, so 2.0.0 is not there to find.
    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    assert_ne!(backfill_state(&writer.conn).unwrap(), before_opening);
    assert!(writer
        .continue_version_backfill(Duration::from_secs(600))
        .unwrap());
    drop(writer);
    let reader = store.read_only().unwrap();
    assert!(!reader.version_backfill_running().unwrap());
    let all: Vec<String> = reader
        .version_sightings("checkout")
        .unwrap()
        .into_iter()
        .map(|s| s.version)
        .collect();
    assert_eq!(all, ["0.9.0", "1.0.0", "1.1.0", "1.2.0"]);
}

/// A day dense with spans must not hold the writer for the whole day:
/// the budget is looked at after every hour of it.
#[test]
fn a_backfill_out_of_budget_stops_inside_a_day() {
    const HOUR: i64 = 3600;
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    {
        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[demo_span("1.0.0", 100), demo_span("1.0.1", 100 + 2 * HOUR)])
            .unwrap();
        writer
            .conn
            .execute_batch(
                "DELETE FROM service_version_sightings;
                 UPDATE sightings_backfill SET done = false, cursor_ns = NULL",
            )
            .unwrap();
        // No budget: one hour is read, and kept.
        assert!(!writer.continue_version_backfill(Duration::ZERO).unwrap());
        assert_eq!(seen(&store, "checkout"), vec![demo("1.0.0", 100, 100)]);
        assert!(matches!(
            backfill_state(&writer.conn).unwrap(),
            Backfill::Running(cursor) if cursor == T0 + (100 + HOUR) * S
        ));
        // The next turn goes on from there.
        assert!(writer
            .continue_version_backfill(Duration::from_secs(30))
            .unwrap());
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(
        seen(&store, "checkout"),
        vec![
            demo("1.0.0", 100, 100),
            demo("1.0.1", 100 + 2 * HOUR, 100 + 2 * HOUR)
        ]
    );
}

#[test]
fn a_reader_says_when_the_backfill_is_still_running() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[demo_span("1.0.0", 100)]).unwrap();
    assert!(!store
        .read_only()
        .unwrap()
        .version_backfill_running()
        .unwrap());
    writer
        .conn
        .execute_batch("UPDATE sightings_backfill SET done = false, cursor_ns = 0")
        .unwrap();
    assert!(store
        .read_only()
        .unwrap()
        .version_backfill_running()
        .unwrap());
    assert!(writer
        .continue_version_backfill(Duration::from_secs(5))
        .unwrap());
    assert!(!store
        .read_only()
        .unwrap()
        .version_backfill_running()
        .unwrap());
}

#[test]
fn a_rebuild_reads_the_rollup_back_from_the_spans() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[demo_span("1.0.0", 10), demo_span("1.1.0", 60)])
        .unwrap();
    // Spans that arrived by a bulk import, past `insert_spans`.
    writer
        .conn
        .execute_batch(
            "INSERT INTO spans (ts_ns, trace_id, span_id, span_name, span_kind, \
             duration_ns, status_code, status_message, service_name, service_version, \
             target_environment) VALUES \
             (946684870000000000, 't', 's', 'GET /', 'SERVER', 0, 'OK', '', 'checkout', \
             '1.2.0', 'demo')",
        )
        .unwrap();
    assert_eq!(seen(&store, "checkout").len(), 2);
    writer.rebuild_version_sightings().unwrap();
    assert_eq!(
        seen(&store, "checkout"),
        vec![
            demo("1.0.0", 10, 10),
            demo("1.1.0", 60, 60),
            demo("1.2.0", 70, 70)
        ]
    );
    assert!(!store
        .read_only()
        .unwrap()
        .version_backfill_running()
        .unwrap());
}

#[test]
fn the_versions_that_served_a_window_are_counted_in_its_environment() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    let mut rows = vec![
        // Before the window: 0.9.0 was live.
        demo_span("0.9.0", 50),
        // A busy other environment inside the window.
        span("checkout", Some("2.0.0"), Some("prod"), 110 * S),
        span("checkout", Some("2.0.0"), Some("prod"), 111 * S),
        span("checkout", Some("2.0.0"), Some("prod"), 112 * S),
        span("checkout", Some("2.0.0"), Some("prod"), 113 * S),
        // Untagged spans are an environment of their own.
        span("checkout", Some("3.0.0"), None, 120 * S),
    ];
    rows.extend((0..3).map(|i| demo_span("1.0.0", 100 + i)));
    rows.extend((0..2).map(|i| demo_span("1.1.0", 150 + i)));
    writer.insert_spans(&rows).unwrap();
    drop(writer);
    let reader = store.read_only().unwrap();
    let observed = |env: Option<&str>, from: i64, to: i64| {
        reader
            .observed_versions("checkout", env, None, T0 + from * S, T0 + to * S)
            .unwrap()
    };
    // Most served first.
    assert_eq!(
        observed(Some("demo"), 100, 200),
        vec![share("1.0.0", 3), share("1.1.0", 2)]
    );
    assert_eq!(observed(Some("prod"), 100, 200), vec![share("2.0.0", 4)]);
    assert_eq!(observed(None, 100, 200), vec![share("3.0.0", 1)]);
    // The bounds are inclusive; nothing inside is nothing observed,
    // whatever spoke before.
    assert_eq!(observed(Some("demo"), 151, 151), vec![share("1.1.0", 1)]);
    assert!(observed(Some("demo"), 60, 90).is_empty());
    assert!(observed(Some("staging"), 0, 500).is_empty());
}

#[test]
fn a_window_is_counted_in_requests_so_a_chatty_build_weighs_no_more() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    // 1.0.0 served three requests; 1.1.0 served one, and says a great
    // deal about it.
    let mut rows: Vec<SpanRow> = (0..3).map(|i| demo_span("1.0.0", 100 + i)).collect();
    rows.push(demo_span("1.1.0", 150));
    rows.extend((0..50).map(|n| inner(demo_span("1.1.0", 150), n)));
    // What a service receives off a queue is a request; so is a span
    // with no parent, whatever its kind.
    let mut consumed = inner(demo_span("1.1.0", 160), 0);
    consumed.span_kind = "Consumer".into();
    rows.push(consumed);
    let mut begun = inner(demo_span("1.1.0", 170), 0);
    begun.parent_span_id = None;
    rows.push(begun);
    writer.insert_spans(&rows).unwrap();
    drop(writer);

    let observed = store
        .read_only()
        .unwrap()
        .observed_versions("checkout", Some("demo"), None, T0, T0 + 500 * S)
        .unwrap();
    assert_eq!(observed, vec![share("1.0.0", 3), share("1.1.0", 3)]);
}

#[test]
fn a_namespace_narrows_a_window_to_the_spans_that_carry_it() {
    let (dir, store) = lake();
    let writer = store.writer().unwrap();
    let within = |namespace: &str, version: &str, at: i64| {
        let mut row = demo_span(version, at);
        row.resource_attrs = vec![("service.namespace".into(), namespace.into())];
        row
    };
    writer
        .insert_spans(&[
            within("shop", "1.0.0", 100),
            within("shop", "1.0.0", 101),
            within("bank", "7.0.0", 102),
            demo_span("0.1.0", 103),
        ])
        .unwrap();
    drop(writer);

    let reader = store.read_only().unwrap();
    let observed = |namespace: Option<&str>| {
        reader
            .observed_versions("checkout", Some("demo"), namespace, T0, T0 + 500 * S)
            .unwrap()
    };
    assert_eq!(observed(Some("shop")), vec![share("1.0.0", 2)]);
    assert_eq!(observed(Some("bank")), vec![share("7.0.0", 1)]);
    assert!(observed(Some("nobody")).is_empty());
    assert_eq!(observed(None).len(), 3);

    // A runner that names its target's namespace is answered within it.
    let versions = StoredVersions::at(&dir.path().join("lake.duckdb"));
    let window = RunWindow {
        system: "checkout".into(),
        environment: Some("demo".into()),
        namespace: Some("bank".into()),
        started_at_ns: T0,
        ended_at_ns: T0 + 500 * S,
    };
    assert_eq!(versions.resolve(&window), Ok(vec![share("7.0.0", 1)]));
}

#[test]
fn a_runner_is_told_the_versions_that_served_its_window_in_its_environment() {
    let (dir, store) = lake();
    let writer = store.writer().unwrap();
    let mut rows = vec![
        demo_span("1.0.0", 50),
        span("checkout", Some("3.0.0"), None, 120 * S),
    ];
    rows.extend((0..3).map(|i| demo_span("1.1.0", 100 + i)));
    writer.insert_spans(&rows).unwrap();
    drop(writer);
    let versions = StoredVersions::at(&dir.path().join("lake.duckdb"));
    let window = |environment: Option<&str>, from: i64, to: i64| RunWindow {
        system: "checkout".into(),
        environment: environment.map(str::to_string),
        namespace: None,
        started_at_ns: T0 + from * S,
        ended_at_ns: T0 + to * S,
    };
    let demo = versions.resolve(&window(Some("demo"), 90, 130));
    assert_eq!(demo, Ok(vec![share("1.1.0", 3)]));
    let untagged = versions.resolve(&window(None, 90, 130));
    assert_eq!(untagged, Ok(vec![share("3.0.0", 1)]));
    assert_eq!(versions.resolve(&window(Some("prod"), 90, 130)), Ok(vec![]));
    assert_eq!(versions.resolve(&window(Some("demo"), 60, 70)), Ok(vec![]));
}

#[test]
fn a_store_that_cannot_be_read_is_an_error_not_an_absent_version() {
    let dir = tempfile::TempDir::new().unwrap();
    let versions = StoredVersions::at(&dir.path().join("missing.duckdb"));
    let window = RunWindow {
        system: "checkout".into(),
        environment: None,
        namespace: None,
        started_at_ns: 0,
        ended_at_ns: S,
    };
    let error = versions.resolve(&window).unwrap_err();
    assert!(error.contains("missing.duckdb"), "{error}");
}

/// 1.0.0, then 2.0.0 whose first span bears a time a day ahead of the
/// clock, twenty minutes of 2.0.0, then 3.0.0: 24 spans.
fn with_a_span_from_tomorrow(writer: &Writer) {
    writer.insert_spans(&[demo_span("1.0.0", 10_000)]).unwrap();
    writer
        .insert_spans(&[demo_span("2.0.0", 20_000 + 86_400)])
        .unwrap();
    for minute in 0..=20 {
        writer
            .insert_spans(&[demo_span("2.0.0", 20_000 + minute * 60)])
            .unwrap();
    }
    writer.insert_spans(&[demo_span("3.0.0", 21_300)]).unwrap();
}

fn spans_stored(writer: &Writer) -> i64 {
    writer
        .conn
        .query_row("SELECT count(*) FROM spans", params![], |row| row.get(0))
        .unwrap()
}

/// The history [`with_a_span_from_tomorrow`] should leave: the span from
/// tomorrow nowhere in it, and 3.0.0 the latest.
fn without_the_span_from_tomorrow() -> [Seen; 3] {
    [
        demo("1.0.0", 10_000, 10_000),
        demo("2.0.0", 20_000, 21_200),
        demo("3.0.0", 21_300, 21_300),
    ]
}

#[test]
fn a_span_stamped_ahead_of_the_clock_is_stored_and_left_out_of_the_rollup() {
    let (_dir, store) = lake();
    let mut writer = store.writer().unwrap();
    // 3.0.0's span is 50 s ahead of this clock: within the skew allowed.
    writer.clock = || T0 + 21_250 * S;
    with_a_span_from_tomorrow(&writer);
    assert_eq!(spans_stored(&writer), 24);
    drop(writer);

    assert_eq!(seen(&store, "checkout"), without_the_span_from_tomorrow());
    assert!(deployments(&store).iter().all(|(_, _, times)| *times == 1));
}

#[test]
fn a_rebuild_leaves_out_a_span_stamped_ahead_of_the_clock() {
    let (_dir, store) = lake();
    let mut writer = store.writer().unwrap();
    writer.clock = || T0 + 21_250 * S;
    with_a_span_from_tomorrow(&writer);
    writer.rebuild_version_sightings().unwrap();
    assert_eq!(spans_stored(&writer), 24);
    drop(writer);

    assert_eq!(seen(&store, "checkout"), without_the_span_from_tomorrow());
}

#[test]
fn a_warning_passes_once_in_a_while_and_says_how_many_were_held_back() {
    let throttle = Throttle::new();
    let start = Instant::now();
    assert_eq!(throttle.pass(start), Some(0));
    assert_eq!(throttle.pass(start + Duration::from_secs(1)), None);
    assert_eq!(throttle.pass(start + WARN_EVERY / 2), None);
    assert_eq!(throttle.pass(start + WARN_EVERY), Some(2));
    assert_eq!(throttle.pass(start + WARN_EVERY * 2), Some(0));
}

#[test]
fn the_spans_ahead_of_the_clock_are_named_by_version_and_environment() {
    let rows = [
        demo_span("1.0.0", 10),
        span("checkout", None, Some("demo"), 500),
        demo_span("2.0.0", 400),
        demo_span("2.0.0", 900),
    ];
    let ahead = ahead_in(&rows, T0 + 100 * S).unwrap();
    assert_eq!(
        (ahead.service.as_str(), ahead.environment.as_deref()),
        ("checkout", Some("demo"))
    );
    assert_eq!((ahead.version.as_str(), ahead.spans), ("2.0.0", 2));
    assert_eq!(ahead.span_ts_ns, T0 + 400 * S);
    assert!(ahead_in(&rows, T0 + 900 * S).is_none());
}

/// 1.0.0 at 10 000 s, 2.0.0 at 20 000 s, then thirty single spans of
/// 2.0.0, each 500 s before the last and each in a batch of its own.
fn with_late_spans_one_after_another(writer: &Writer) {
    writer.insert_spans(&[demo_span("1.0.0", 10_000)]).unwrap();
    writer.insert_spans(&[demo_span("2.0.0", 20_000)]).unwrap();
    for step in 1..=30 {
        writer
            .insert_spans(&[demo_span("2.0.0", 20_000 - step * 500)])
            .unwrap();
    }
}

#[test]
fn late_spans_one_after_another_do_not_walk_a_first_sight_back() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    with_late_spans_one_after_another(&writer);
    drop(writer);

    // Only the first of them lies within the limit of where 2.0.0 began.
    assert_eq!(
        seen(&store, "checkout"),
        [demo("1.0.0", 10_000, 10_000), demo("2.0.0", 19_500, 20_000)]
    );
    assert_eq!(
        deployments(&store),
        [
            ("1.0.0".to_string(), 10_000, 1),
            ("2.0.0".to_string(), 19_500, 1)
        ]
    );
}

#[test]
fn a_row_of_an_older_build_is_held_to_the_first_sight_it_has() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[demo_span("1.0.0", 10_000)]).unwrap();
    writer.insert_spans(&[demo_span("2.0.0", 20_000)]).unwrap();
    // As a build before the rollup kept where a version began left it.
    writer
        .conn
        .execute_batch("UPDATE service_version_sightings SET first_anchor_ns = NULL")
        .unwrap();
    for step in 1..=30 {
        writer
            .insert_spans(&[demo_span("2.0.0", 20_000 - step * 500)])
            .unwrap();
    }
    drop(writer);

    assert_eq!(
        seen(&store, "checkout"),
        [demo("1.0.0", 10_000, 10_000), demo("2.0.0", 19_500, 20_000)]
    );
}

/// A row of `checkout` 2.0.0 in demo as a build before the clock bound
/// left it: every sight at `at_ns`, and with `anchored` the anchor too.
fn seed_row(writer: &Writer, at_ns: i64, anchored: bool) {
    let at_ns = T0 + at_ns;
    writer
        .conn
        .execute(
            "INSERT INTO service_version_sightings
                (service_name, environment, service_version, first_seen_ns, last_seen_ns,
                 last_deployed_ns, deployments, sanitized, first_anchor_ns)
             VALUES ('checkout', 'demo', '2.0.0', ?, ?, ?, 1, false, ?)",
            params![at_ns, at_ns, at_ns, anchored.then_some(at_ns)],
        )
        .unwrap();
}

/// (first sight, last sight, latest deployment, anchor) of 2.0.0 as the
/// rollup table itself holds them, in seconds.
fn stored_row(writer: &Writer) -> Vec<(i64, i64, i64, Option<i64>)> {
    let mut stmt = writer
        .conn
        .prepare(
            "SELECT first_seen_ns, last_seen_ns, last_deployed_ns, first_anchor_ns
             FROM service_version_sightings WHERE service_version = '2.0.0'",
        )
        .unwrap();
    let rows = stmt
        .query_map(params![], |row| {
            Ok((
                (row.get::<_, i64>(0)? - T0) / S,
                (row.get::<_, i64>(1)? - T0) / S,
                (row.get::<_, i64>(2)? - T0) / S,
                row.get::<_, Option<i64>>(3)?.map(|ns| (ns - T0) / S),
            ))
        })
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_row_held_ahead_of_the_clock_is_anchored_anew_by_the_next_honest_span() {
    const YEAR: i64 = 365 * 86_400;
    for anchored in [false, true] {
        let (_dir, store) = lake();
        let mut writer = store.writer().unwrap();
        writer.clock = || T0 + 21_250 * S;
        writer.insert_spans(&[demo_span("1.0.0", 10_000)]).unwrap();
        seed_row(&writer, (21_250 + YEAR) * S, anchored);
        // A batch that says nothing of 2.0.0 leaves its row alone.
        writer.insert_spans(&[demo_span("1.0.0", 10_060)]).unwrap();
        assert_eq!(stored_row(&writer).len(), 1, "anchored: {anchored}");
        writer
            .insert_spans(&[demo_span("2.0.0", 20_000), demo_span("2.0.0", 20_060)])
            .unwrap();
        // Nothing of the year ahead is left in the row, the anchor
        // included: it was written over, not merged with.
        assert_eq!(
            stored_row(&writer),
            [(20_000, 20_060, 20_000, Some(20_000))],
            "anchored: {anchored}"
        );
        // And from there the version is held like any other.
        writer.insert_spans(&[demo_span("2.0.0", 19_500)]).unwrap();
        writer.insert_spans(&[demo_span("2.0.0", 19_000)]).unwrap();
        drop(writer);
        assert_eq!(
            seen(&store, "checkout"),
            [demo("1.0.0", 10_000, 10_060), demo("2.0.0", 19_500, 20_060)],
            "anchored: {anchored}"
        );
        assert_eq!(
            deployments(&store),
            [
                ("1.0.0".to_string(), 10_000, 1),
                ("2.0.0".to_string(), 19_500, 1)
            ],
            "anchored: {anchored}"
        );
    }
}

#[test]
fn a_row_held_within_the_skew_of_the_clock_keeps_its_anchor() {
    let (_dir, store) = lake();
    let mut writer = store.writer().unwrap();
    writer.clock = || T0 + 21_250 * S;
    // Exactly at the horizon: an honest clock a little ahead.
    seed_row(&writer, (21_250 + 300) * S, true);
    writer.insert_spans(&[demo_span("2.0.0", 20_000)]).unwrap();
    drop(writer);
    assert_eq!(seen(&store, "checkout"), [demo("2.0.0", 21_550, 21_550)]);
}

#[test]
fn a_backfill_ends_past_a_span_stamped_at_the_end_of_time() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            demo_span("1.0.0", 10_000),
            span("checkout", Some("9.9.9"), Some("demo"), i64::MAX - T0),
        ])
        .unwrap();
    assert_eq!(spans_stored(&writer), 2);
    in_tx(&writer.conn, || backfill_restart(&writer.conn)).unwrap();
    // The day that holds the last instant there is cannot end after it:
    // the step reads it all the same and moves the cursor there.
    let steps = (0..4)
        .position(|_| in_tx(&writer.conn, || backfill_step(&writer.conn, None, NOW)).unwrap());
    assert_eq!(steps, Some(2), "two days of spans, then nothing left");
    assert_eq!(
        backfill_state(&writer.conn).unwrap(),
        Backfill::Done,
        "the backfill is over"
    );
    drop(writer);
    assert_eq!(seen(&store, "checkout"), [demo("1.0.0", 10_000, 10_000)]);
}

/// 2.0.0 stamped at the epoch and a nanosecond before the floor, then
/// 1.0.0 at the floor itself and, long after it, 2.0.0 for real with one
/// more span from before the epoch: 5 spans.
fn with_spans_from_before_the_floor(writer: &Writer) {
    let at = |ts_ns: i64| SpanRow {
        ts_ns,
        span_id: format!("old-{ts_ns}"),
        ..demo_span("2.0.0", 0)
    };
    writer
        .insert_spans(&[at(0), at(EARLIEST_SIGHT_NS - 1)])
        .unwrap();
    writer.insert_spans(&[demo_span("1.0.0", 0)]).unwrap();
    writer
        .insert_spans(&[demo_span("2.0.0", 20_000), at(-5 * S)])
        .unwrap();
}

/// The history [`with_spans_from_before_the_floor`] should leave: 1.0.0
/// first, at the floor, and 2.0.0 where it really began.
fn without_the_spans_from_before_the_floor() -> [Seen; 2] {
    [demo("1.0.0", 0, 0), demo("2.0.0", 20_000, 20_000)]
}

#[test]
fn a_span_stamped_before_the_floor_is_stored_and_left_out_of_the_rollup() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    with_spans_from_before_the_floor(&writer);
    assert_eq!(spans_stored(&writer), 5);
    drop(writer);

    assert_eq!(
        seen(&store, "checkout"),
        without_the_spans_from_before_the_floor()
    );
    assert!(deployments(&store).iter().all(|(_, _, times)| *times == 1));
}

#[test]
fn a_rebuild_leaves_out_a_span_stamped_before_the_floor() {
    let (_dir, store) = lake();
    let writer = store.writer().unwrap();
    with_spans_from_before_the_floor(&writer);
    writer.rebuild_version_sightings().unwrap();
    assert_eq!(spans_stored(&writer), 5);
    drop(writer);

    assert_eq!(
        seen(&store, "checkout"),
        without_the_spans_from_before_the_floor()
    );
}

#[test]
fn the_spans_from_before_the_floor_are_named_by_version_and_environment() {
    let rows = [
        demo_span("1.0.0", 10),
        span("checkout", None, Some("demo"), -500 * S),
        demo_span("2.0.0", -400),
        demo_span("2.0.0", -1),
    ];
    let old = before_the_floor_in(&rows).unwrap();
    assert_eq!(
        (old.service.as_str(), old.environment.as_deref()),
        ("checkout", Some("demo"))
    );
    assert_eq!((old.version.as_str(), old.spans), ("2.0.0", 2));
    assert_eq!(old.span_ts_ns, T0 - 400 * S);
    assert!(before_the_floor_in(&rows[..1]).is_none());
}
