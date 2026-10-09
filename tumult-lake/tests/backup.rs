#![cfg(feature = "duckdb")]
use tumult_lake::{backup, Store};

#[test]
fn complete_backup_restores_credentials_provenance_and_arbitrary_new_tables() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.duckdb");
    let store = Store::open(&source).unwrap();
    let writer = store.writer().unwrap();
    writer
        .execute(
            "CREATE TABLE future_feature (id VARCHAR, value VARCHAR)",
            [],
        )
        .unwrap();
    writer
        .execute("INSERT INTO future_feature VALUES ('one', 'secret')", [])
        .unwrap();
    writer
        .execute(
            "INSERT INTO users VALUES ('user', 'alice', 'hash-secret', 'admin', false, false, 1)",
            [],
        )
        .unwrap();
    writer
        .execute(
            "INSERT INTO sessions VALUES ('session-hash', 'user', 1, 2)",
            [],
        )
        .unwrap();
    writer
        .execute(
            "CREATE VIEW future_view AS SELECT value FROM future_feature",
            [],
        )
        .unwrap();
    drop(writer);
    let bundle = dir.path().join("backup");
    backup::create(&source, &bundle).unwrap();
    let destination = dir.path().join("restored.duckdb");
    backup::restore(&bundle, &destination).unwrap();
    let reader = Store::at(&destination).read_only().unwrap();
    assert_eq!(
        reader
            .query_json_rows("SELECT value FROM future_feature")
            .unwrap()[0]["value"],
        "secret"
    );
    assert_eq!(
        reader
            .query_json_rows("SELECT value FROM future_view")
            .unwrap()[0]["value"],
        "secret"
    );
    assert_eq!(
        reader
            .query_json_rows("SELECT password_hash FROM users WHERE id = 'user'")
            .unwrap()[0]["password_hash"],
        "hash-secret"
    );
    assert_eq!(
        reader
            .query_json_rows("SELECT id_hash FROM sessions WHERE user_id = 'user'")
            .unwrap()[0]["id_hash"],
        "session-hash"
    );
    let tables = reader
        .query_json_rows(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'main'",
        )
        .unwrap();
    for required in [
        "users",
        "tokens",
        "sessions",
        "runs",
        "run_audit",
        "evidence_attachments",
        "import_batches",
        "approval_requests",
        "run_execution_pins",
    ] {
        assert!(
            tables.iter().any(|row| row["table_name"] == required),
            "missing {required}"
        );
    }
    assert!(
        backup::restore(&bundle, &destination).is_err(),
        "never replace a live/existing store"
    );
    assert!(
        backup::create(&source, &bundle).is_err(),
        "never replace an existing backup"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&bundle).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn restore_rejects_tampered_database() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.duckdb");
    Store::open(&source).unwrap();
    let bundle = dir.path().join("backup");
    backup::create(&source, &bundle).unwrap();
    std::fs::write(bundle.join("store.duckdb"), b"tampered").unwrap();
    let target = dir.path().join("restored.duckdb");
    assert!(backup::restore(&bundle, &target).is_err());
    assert!(!target.exists());
}

#[test]
fn backup_rejects_missing_schema_version_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.duckdb");
    let store = Store::open(&source).unwrap();
    let writer = store.writer().unwrap();
    writer
        .execute("DELETE FROM schema_meta WHERE key = 'version'", [])
        .unwrap();
    drop(writer);
    assert!(backup::create(&source, &dir.path().join("backup")).is_err());
}

#[test]
fn restore_accepts_relative_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.duckdb");
    Store::open(&source).unwrap();
    let bundle = dir.path().join("backup");
    backup::create(&source, &bundle).unwrap();
    // Isolate current_dir in a child; changing it in a threaded test races.
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "relative_restore_child", "--nocapture"])
        .env("TUMULT_TEST_RESTORE_BUNDLE", &bundle)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(dir.path().join("restored.duckdb").is_file());
}

#[test]
fn relative_restore_child() {
    let Some(bundle) = std::env::var_os("TUMULT_TEST_RESTORE_BUNDLE") else {
        return;
    };
    backup::restore(
        std::path::Path::new(&bundle),
        std::path::Path::new("restored.duckdb"),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn restore_rejects_a_dangling_wal_path() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.duckdb");
    Store::open(&source).unwrap();
    let bundle = dir.path().join("backup");
    backup::create(&source, &bundle).unwrap();
    let destination = dir.path().join("restored.duckdb");
    std::os::unix::fs::symlink(
        dir.path().join("unrelated"),
        dir.path().join("restored.duckdb.wal"),
    )
    .unwrap();
    assert!(backup::restore(&bundle, &destination).is_err());
    assert!(!destination.exists());
}
