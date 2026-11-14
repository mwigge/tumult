//! A lake written by a newer build is refused by an older one.
//!
//! A table gains columns with a schema version, and the inserts of this
//! crate name no columns for most tables. An older binary that opened a
//! newer lake would therefore fail on every write, long after it started.
//! It refuses at open instead, before it changes anything.

use tumult_lake::duckdb_store::AnalyticsStore;
use tumult_lake::{AnalyticsError, Store, StoreError, CURRENT_SCHEMA_VERSION};

const NEWER: i64 = CURRENT_SCHEMA_VERSION + 1;

/// A lake at `version`, as a build that knows that version would leave it.
fn lake_at(version: i64) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("kronika.duckdb");
    let store = Store::open(&path).unwrap();
    let writer = store.writer().unwrap();
    writer
        .execute(
            "UPDATE schema_meta SET value = ? WHERE key = 'version'",
            [version],
        )
        .unwrap();
    writer
        .execute("ALTER TABLE logs ADD COLUMN from_the_future VARCHAR", [])
        .unwrap();
    drop(writer);
    (dir, path)
}

fn stored_version(path: &std::path::Path) -> i64 {
    Store::at(path)
        .read_only()
        .unwrap()
        .query_json_rows("SELECT value FROM schema_meta WHERE key = 'version'")
        .unwrap()[0]["value"]
        .as_i64()
        .unwrap()
}

#[test]
fn a_lake_newer_than_the_build_is_refused_at_open() {
    let (_dir, path) = lake_at(NEWER);
    let refused = Store::open(&path).err().expect("the open is refused");
    assert!(
        matches!(
            refused,
            StoreError::SchemaTooNew { stored, supported }
                if stored == NEWER && supported == CURRENT_SCHEMA_VERSION
        ),
        "{refused:?}"
    );
    let message = refused.to_string();
    assert!(
        message.starts_with(&format!(
            "lake schema v{NEWER} is newer than this build supports (v{CURRENT_SCHEMA_VERSION})"
        )),
        "{message}"
    );
}

/// The refusal comes before the migration: the version the newer build
/// wrote is still there, so that build can open the lake again.
#[test]
fn a_refused_open_changes_nothing() {
    let (_dir, path) = lake_at(NEWER + 5);
    assert!(Store::open(&path).is_err());
    assert!(Store::open(&path).is_err(), "and it is refused every time");
    assert_eq!(stored_version(&path), NEWER + 5);
}

/// The analytics store migrates the same lake through the same code.
#[test]
fn the_analytics_store_refuses_it_too() {
    let (_dir, path) = lake_at(NEWER);
    let refused = AnalyticsStore::open(&path).err().expect("refused");
    assert!(
        matches!(
            refused,
            AnalyticsError::SchemaTooNew { stored, supported }
                if stored == NEWER && supported == CURRENT_SCHEMA_VERSION
        ),
        "{refused:?}"
    );
    assert!(
        refused.to_string().starts_with("lake schema v"),
        "{refused}"
    );
}

/// Reading is not migrating: a read-only open of a newer lake is still
/// possible, which is what `tumult store restore` and the readiness probe
/// do to learn the version.
#[test]
fn a_newer_lake_can_still_be_read() {
    let (_dir, path) = lake_at(NEWER);
    assert_eq!(stored_version(&path), NEWER);
    assert_eq!(
        AnalyticsStore::open_read_only(&path)
            .unwrap()
            .schema_version()
            .unwrap(),
        NEWER
    );
}

#[test]
fn a_lake_of_this_build_or_an_older_one_opens() {
    let (_dir, current) = lake_at(CURRENT_SCHEMA_VERSION);
    Store::open(&current).unwrap();
    assert_eq!(stored_version(&current), CURRENT_SCHEMA_VERSION);
    let dir = tempfile::TempDir::new().unwrap();
    Store::open(&dir.path().join("fresh.duckdb")).unwrap();
}

/// A writer is also opened without a migration: the ingest task rebuilds
/// its connection that way after a fatal error. It is refused like the
/// open, so a daemon cannot come to write a lake that a newer build took
/// over in the meantime.
#[test]
fn a_writer_on_a_newer_lake_is_refused_without_a_migration() {
    let (_dir, path) = lake_at(NEWER);
    let refused = Store::at(&path).writer().err().expect("refused");
    assert!(
        matches!(
            refused,
            StoreError::SchemaTooNew { stored, supported }
                if stored == NEWER && supported == CURRENT_SCHEMA_VERSION
        ),
        "{refused:?}"
    );
    assert_eq!(stored_version(&path), NEWER);
}

/// A file that holds no lake yet has no version: the writer that creates
/// it is not refused.
#[test]
fn a_writer_on_a_new_file_is_not_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let opened = Store::at(&dir.path().join("fresh.duckdb")).writer();
    assert!(opened.is_ok(), "{:?}", opened.err());
}
