use super::rows::{ActivityRow, ExperimentRow};
use super::*;

/// Column names declared by a `CREATE TABLE` statement, in order.
fn ddl_columns(ddl: &str) -> Vec<&str> {
    let open = ddl.find('(').expect("column list opens");
    let close = ddl.find(") ENGINE").expect("column list closes");
    ddl[open + 1..close]
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

/// Number of columns a `CREATE TABLE` statement declares, from the commas
/// that separate them.
fn declared_column_count(ddl: &str) -> usize {
    let open = ddl.find('(').expect("column list opens");
    let close = ddl.find(") ENGINE").expect("column list closes");
    ddl[open..close].matches(',').count() + 1
}

/// Inserts and selects name the columns from the row struct's fields, so a
/// field the table does not declare (or the reverse) fails at run time
/// against a real server. This pins the two lists to each other.
#[test]
fn row_fields_match_the_columns_of_their_tables() {
    use clickhouse::Row;

    assert_eq!(ExperimentRow::COLUMN_NAMES, ddl_columns(EXPERIMENTS_DDL));
    assert_eq!(ActivityRow::COLUMN_NAMES, ddl_columns(ACTIVITY_RESULTS_DDL));
    // Counted a second way, so a parser that drops lines cannot agree with
    // itself.
    assert_eq!(
        ExperimentRow::COLUMN_NAMES.len(),
        declared_column_count(EXPERIMENTS_DDL)
    );
    assert_eq!(
        ActivityRow::COLUMN_NAMES.len(),
        declared_column_count(ACTIVITY_RESULTS_DDL)
    );
}

/// The retention-period arithmetic overflow used to be an `expect` panic in
/// library code; it must now surface as a checked `AnalyticsError::Internal`.
/// The overflow check runs before any network I/O, so an unreachable store
/// suffices.
#[test]
fn purge_retention_overflow_is_checked_error_not_panic() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let store = ClickHouseStore {
            client: Client::default().with_url("http://127.0.0.1:1"),
            database: "test".into(),
            query_timeout: std::time::Duration::from_millis(100),
        };
        let err = store
            .purge_older_than_days_async(u32::MAX)
            .await
            .expect_err("u32::MAX days must overflow i64 nanoseconds");
        assert!(
            matches!(
                err,
                tumult_lake::error::AnalyticsError::Internal(ref msg)
                    if msg.contains("overflows")
            ),
            "expected Internal overflow error, got {err:?}"
        );
    });
}

/// Verifies that the synchronous `AnalyticsBackend` wrapper methods use
/// `block_in_place` rather than a bare `block_on`, which would panic when
/// called from inside an already-running multi-threaded Tokio context.
///
/// The test spawns a real multi-thread runtime and then, from within an
/// async task, calls the synchronous trait methods via a `ClickHouseStore`
/// configured with an unreachable URL.  The calls are expected to return
/// an `Err` (connection refused / timeout), but must NOT panic.
#[test]
fn analytics_backend_sync_methods_do_not_panic_inside_tokio_task() {
    use tumult_lake::backend::AnalyticsBackend as _;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        // Spawn a task — this is the "inside a Tokio task" scenario that
        // previously triggered a panic from bare Handle::current().block_on().
        tokio::task::spawn(async {
            // Build a store pointing at an unreachable URL — no real
            // ClickHouse needed; we only care that the call doesn't panic.
            let store = ClickHouseStore {
                client: Client::default().with_url("http://127.0.0.1:1"),
                database: "test".into(),
                query_timeout: std::time::Duration::from_millis(100),
            };

            // Each of these must return the connection error, not panic.
            assert!(matches!(
                store.experiment_count(),
                Err(AnalyticsError::ClickHouse(_))
            ));
            assert!(matches!(store.stats(), Err(AnalyticsError::ClickHouse(_))));
            assert!(matches!(
                store.schema_version(),
                Err(AnalyticsError::ClickHouse(_))
            ));
        })
        .await
        .expect("task should not panic");
    });
}

#[derive(serde::Serialize, clickhouse::Row)]
struct Count {
    count: u64,
}

/// A store whose queries are answered by `mock`.
fn mocked_store(mock: &clickhouse::test::Mock) -> ClickHouseStore {
    ClickHouseStore {
        client: Client::default().with_mock(mock),
        database: "tumult".into(),
        query_timeout: std::time::Duration::from_secs(5),
    }
}

#[tokio::test]
async fn stats_returns_the_server_counts_and_records_them_as_gauges() {
    use clickhouse::test::handlers::provide;
    use tumult_test_utils::MetricCapture;

    let mock = clickhouse::test::Mock::new();
    mock.add(provide([Count { count: 3 }]));
    mock.add(provide([Count { count: 7 }]));
    let store = mocked_store(&mock);
    let capture = MetricCapture::new();

    let stats = store
        .stats_recorded_on(&capture.meter("tumult-clickhouse"))
        .await
        .expect("both count queries are answered");

    assert_eq!((stats.experiment_count, stats.activity_count), (3, 7));
    let gauge = |name: &str| -> Vec<f64> { capture.points(name).iter().map(|p| p.value).collect() };
    assert_eq!(gauge("clickhouse.store.experiments"), [3.0]);
    assert_eq!(gauge("clickhouse.store.activities"), [7.0]);
}

/// The defaults are what callers get when they do not tune the retry: three
/// attempts, waiting 2 s, 4 s and 8 s after the failures. The database name
/// is rejected before any request is made, so every attempt fails at once
/// and the paused clock measures the backoff alone.
#[tokio::test(start_paused = true)]
async fn default_retry_makes_three_attempts_with_doubling_backoff() {
    use tumult_test_utils::capture_spans_async;

    let config = ClickHouseConfig {
        database: "not a valid name".into(),
        ..ClickHouseConfig::default()
    };
    let started = tokio::time::Instant::now();

    let (result, spans) = capture_spans_async(ClickHouseStore::connect_with_retry(
        &config,
        &RetryConfig::default(),
    ))
    .await;

    assert!(matches!(result.err(), Some(AnalyticsError::Io(_))));
    let attempts = spans
        .iter()
        .filter(|span| span.name == "clickhouse.connect")
        .count();
    assert_eq!(attempts, 3);
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(14));
}

#[tokio::test(start_paused = true)]
async fn retry_stops_at_the_configured_attempts_and_waits_the_listed_backoff() {
    use tumult_test_utils::capture_spans_async;

    let config = ClickHouseConfig {
        database: "not a valid name".into(),
        ..ClickHouseConfig::default()
    };
    let retry = RetryConfig {
        max_attempts: 2,
        backoff_durations: vec![std::time::Duration::from_millis(300)],
    };
    let started = tokio::time::Instant::now();

    let (result, spans) =
        capture_spans_async(ClickHouseStore::connect_with_retry(&config, &retry)).await;

    assert!(matches!(result.err(), Some(AnalyticsError::Io(_))));
    assert_eq!(spans.len(), 2);
    // 300 ms after the first failure; the second has no listed backoff and
    // falls back to 8 s.
    assert_eq!(started.elapsed(), std::time::Duration::from_millis(8_300));
}
