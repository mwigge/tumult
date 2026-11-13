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

/// Inserts and selects name the columns from the row struct's fields, so a
/// field the table does not declare (or the reverse) fails at run time
/// against a real server. This pins the two lists to each other.
#[test]
fn row_fields_match_the_columns_of_their_tables() {
    use clickhouse::Row;

    assert_eq!(ExperimentRow::COLUMN_NAMES, ddl_columns(EXPERIMENTS_DDL));
    assert_eq!(ActivityRow::COLUMN_NAMES, ddl_columns(ACTIVITY_RESULTS_DDL));
    assert_eq!(ActivityRow::COLUMN_NAMES.len(), 9);
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
