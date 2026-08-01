use super::*;
use tempfile::TempDir;

// ── validate_select_only ─────────────────────────────────

#[test]
fn validate_select_only_allows_select() {
    assert!(validate_select_only("SELECT * FROM experiments").is_ok());
}

#[test]
fn validate_select_only_allows_with() {
    assert!(validate_select_only("WITH cte AS (SELECT 1) SELECT * FROM cte").is_ok());
}

#[test]
fn validate_select_only_allows_lowercase() {
    assert!(validate_select_only("select count(*) from experiments").is_ok());
}

#[test]
fn validate_select_only_allows_whitespace_prefix() {
    assert!(validate_select_only("  SELECT 1").is_ok());
}

#[test]
fn validate_select_only_rejects_drop() {
    let result = validate_select_only("DROP TABLE experiments");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("only SELECT/WITH"));
}

#[test]
fn validate_select_only_rejects_insert() {
    assert!(validate_select_only("INSERT INTO experiments VALUES (1)").is_err());
}

#[test]
fn validate_select_only_rejects_update() {
    assert!(validate_select_only("UPDATE experiments SET x=1").is_err());
}

#[test]
fn validate_select_only_rejects_delete() {
    assert!(validate_select_only("DELETE FROM experiments").is_err());
}

#[test]
fn validate_select_only_rejects_create() {
    assert!(validate_select_only("CREATE TABLE foo (id int)").is_err());
}

#[test]
fn validate_select_only_rejects_empty() {
    assert!(validate_select_only("").is_err());
}

#[test]
fn validate_select_only_rejects_stacked_statement() {
    let result = validate_select_only("SELECT 1; DROP TABLE experiments");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("single statement"));
}

#[test]
fn validate_select_only_allows_single_trailing_semicolon() {
    assert!(validate_select_only("SELECT * FROM experiments;").is_ok());
    assert!(validate_select_only("SELECT * FROM experiments ; ").is_ok());
}

#[test]
fn validate_select_only_rejects_dml_after_cte() {
    let result =
        validate_select_only("WITH x AS (SELECT 1) INSERT INTO experiments SELECT * FROM x");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("forbidden keyword"));
}

#[test]
fn validate_select_only_rejects_pragma_and_attach() {
    assert!(validate_select_only("SELECT 1; PRAGMA database_list").is_err());
    assert!(
        validate_select_only("WITH x AS (SELECT 1) ATTACH 'evil.db' AS evil SELECT * FROM x")
            .is_err()
    );
}

#[test]
fn validate_select_only_allows_identifiers_containing_keyword_substrings() {
    assert!(validate_select_only("SELECT alter_ego, settings FROM experiments").is_ok());
}

// ── filesystem / extension table functions ───────────────

#[test]
fn validate_select_only_rejects_read_text_of_host_file() {
    let result = validate_select_only("select * from read_text('/etc/passwd')");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("forbidden keyword"));
}

#[test]
fn validate_select_only_rejects_filesystem_table_functions() {
    for query in [
        "SELECT * FROM read_csv('data.csv')",
        "SELECT * FROM read_parquet('s3://bucket/x.parquet')",
        "SELECT * FROM read_json('/var/log/app.json')",
        "SELECT * FROM read_blob('/etc/shadow')",
        "SELECT * FROM glob('/home/*/.ssh/*')",
        "SELECT * FROM sqlite_scan('/tmp/other.db', 'users')",
        "SELECT * FROM parquet_scan('x.parquet')",
        "SELECT * FROM csv_scan('x.csv')",
        "SELECT * FROM json_scan('x.json')",
        "SELECT * FROM httpfs('https://evil.example/x')",
        "SELECT export_database('/tmp/out')",
        "SELECT import_database('/tmp/in')",
    ] {
        assert!(validate_select_only(query).is_err(), "must reject: {query}");
    }
}

#[test]
fn validate_select_only_rejects_extension_management_in_select() {
    // INSTALL/LOAD/ATTACH/COPY are keyword-class tokens; they must be
    // caught even when wrapped in an otherwise plain SELECT.
    assert!(validate_select_only("SELECT * FROM (INSTALL httpfs)").is_err());
    assert!(validate_select_only("WITH x AS (LOAD httpfs) SELECT 1").is_err());
    assert!(validate_select_only("SELECT * FROM t ATTACH 'evil.db'").is_err());
    assert!(validate_select_only("SELECT 1 FROM t COPY TO '/tmp/out.csv'").is_err());
}

#[test]
fn validate_select_only_allows_plain_select_and_function_name_substrings() {
    assert!(validate_select_only("select title from experiments").is_ok());
    // Identifiers that merely *contain* a forbidden token stay allowed:
    // the scan is word-boundary aware (tokens split on non-ident chars).
    assert!(validate_select_only("SELECT globe_count, loader FROM experiments").is_ok());
}

// ── validate_action_name ─────────────────────────────────

#[test]
fn validate_action_name_allows_simple_name() {
    assert!(validate_action_name("kill-process").is_ok());
}

#[test]
fn validate_action_name_allows_underscores_and_dots() {
    assert!(validate_action_name("cpu_stress.v2").is_ok());
}

#[test]
fn validate_action_name_rejects_single_quote() {
    assert!(validate_action_name("name' OR '1'='1").is_err());
}

#[test]
fn validate_action_name_rejects_semicolon() {
    assert!(validate_action_name("name; DROP TABLE activity_results --").is_err());
}

#[test]
fn validate_action_name_rejects_empty() {
    assert!(validate_action_name("").is_err());
}

// ── safe_resolve_path ────────────────────────────────────

#[test]
fn safe_resolve_path_allows_file_within_base() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("test.toon");
    std::fs::write(&file, "content").unwrap();
    let result = safe_resolve_path(dir.path(), "test.toon");
    assert!(result.is_ok());
}

#[test]
fn safe_resolve_path_allows_subdirectory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let file = sub.join("test.toon");
    std::fs::write(&file, "content").unwrap();
    let result = safe_resolve_path(dir.path(), "sub/test.toon");
    assert!(result.is_ok());
}

#[test]
fn safe_resolve_path_rejects_traversal() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_path(dir.path(), "../../etc/passwd");
    // Either path resolution error (file doesn't exist) or traversal detected
    assert!(result.is_err());
}

#[test]
fn safe_resolve_path_rejects_absolute_escape() {
    let dir = TempDir::new().unwrap();
    // An absolute path that's outside the base
    let result = safe_resolve_path(dir.path(), "/etc/hosts");
    assert!(result.is_err());
}

// ── safe_resolve_output_path ─────────────────────────────

#[test]
fn safe_resolve_output_path_allows_new_file_in_base() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "new.toon").unwrap();
    assert_eq!(result, dir.path().canonicalize().unwrap().join("new.toon"));
}

#[test]
fn safe_resolve_output_path_allows_new_file_in_subdirectory() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let result = safe_resolve_output_path(dir.path(), "sub/new.toon").unwrap();
    assert!(result.starts_with(dir.path().canonicalize().unwrap()));
    assert!(result.ends_with("sub/new.toon"));
}

#[test]
fn safe_resolve_output_path_resolves_existing_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("existing.toon"), "x").unwrap();
    // Existing files resolve successfully; the caller decides whether
    // overwriting is an error (create_experiment reports AlreadyExists).
    let result = safe_resolve_output_path(dir.path(), "existing.toon");
    assert!(result.is_ok());
}

#[test]
fn safe_resolve_output_path_rejects_traversal() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "../evil.toon");
    assert!(result.is_err(), "parent traversal must be rejected");
    let result = safe_resolve_output_path(dir.path(), "../../etc/cron.d/evil");
    assert!(result.is_err(), "deep traversal must be rejected");
}

#[test]
fn safe_resolve_output_path_rejects_dotdot_leaf() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "sub/..");
    assert!(result.is_err(), "`..` leaf must be rejected");
}

#[test]
fn safe_resolve_output_path_rejects_absolute_escape() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "/etc/evil.toon");
    assert!(
        result.is_err(),
        "absolute path outside base must be rejected"
    );
}

#[test]
fn safe_resolve_output_path_rejects_null_byte() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "evil\0.toon");
    assert!(result.is_err(), "null byte must be rejected");
}

#[test]
fn safe_resolve_output_path_rejects_nonexistent_parent() {
    let dir = TempDir::new().unwrap();
    let result = safe_resolve_output_path(dir.path(), "no-such-dir/new.toon");
    assert!(result.is_err(), "missing parent directory must be an error");
}

#[cfg(unix)]
#[test]
fn safe_resolve_output_path_rejects_symlink_escape() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    // A symlinked directory pointing outside the base must not be usable
    // as an output parent.
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
    let result = safe_resolve_output_path(dir.path(), "link/new.toon");
    assert!(result.is_err(), "symlinked parent escape must be rejected");
}

#[cfg(unix)]
#[test]
fn safe_resolve_output_path_rejects_dangling_symlink_leaf() {
    let dir = TempDir::new().unwrap();
    // A dangling symlink at the leaf would redirect the write outside the
    // base even though the target does not exist yet.
    std::os::unix::fs::symlink("/nonexistent/target", dir.path().join("evil.toon")).unwrap();
    let result = safe_resolve_output_path(dir.path(), "evil.toon");
    assert!(result.is_err(), "dangling symlink leaf must be rejected");
}
