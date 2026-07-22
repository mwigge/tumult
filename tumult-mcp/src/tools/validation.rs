//! Input validation and path-safety helpers for MCP tools.

use std::path::{Path, PathBuf};

use crate::error::ToolError;

/// Keywords that introduce a write or schema/configuration change. Rejected
/// as standalone tokens anywhere in the query, since `DuckDB` allows DML/DDL
/// after a leading `WITH` CTE (e.g. `WITH x AS (SELECT 1) INSERT INTO ...`).
const FORBIDDEN_SQL_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "ATTACH", "DETACH", "COPY", "EXPORT",
    "IMPORT", "INSTALL", "LOAD", "PRAGMA", "SET", "CALL", "VACUUM", "TRUNCATE",
];

/// `DuckDB` table functions and extension entry points that reach the host
/// filesystem, the network, or another database. A plain `SELECT` stays
/// read-only against the store, but `SELECT * FROM read_text('/etc/passwd')`
/// reads arbitrary host files and `INSTALL httpfs` loads remote extensions —
/// so these tokens are rejected exactly like the DML/DDL keywords above.
const FORBIDDEN_SQL_FUNCTIONS: &[&str] = &[
    "READ_TEXT",
    "READ_CSV",
    "READ_PARQUET",
    "READ_JSON",
    "READ_BLOB",
    "GLOB",
    "SQLITE_SCAN",
    "PARQUET_SCAN",
    "CSV_SCAN",
    "JSON_SCAN",
    "HTTPFS",
    "EXPORT_DATABASE",
    "IMPORT_DATABASE",
];

/// Validate that a SQL query is read-only (SELECT or WITH only).
///
/// Prevents SQL injection by rejecting any query that does not start
/// with SELECT or WITH (e.g., DROP, INSERT, UPDATE, DELETE, CREATE), any
/// query containing more than one statement (stacked statements via `;`),
/// any query containing a write/DDL keyword as a standalone token (to
/// catch DML smuggled in after a `WITH` CTE), and any query naming a
/// filesystem/extension table function (e.g. `read_text`, `glob`,
/// `parquet_scan`, `httpfs`) that would escape the store onto the host.
///
/// The token scan is deliberately quote-insensitive: a forbidden token
/// inside a string literal also rejects the query. That trades a rare
/// false positive (a literal naming such a function) for never missing a
/// smuggled call — the safe direction for a viewer-facing query tool.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if the query does not start with
/// `SELECT` or `WITH`, contains more than one statement, or contains a
/// forbidden keyword or table function.
pub fn validate_select_only(query: &str) -> Result<(), ToolError> {
    let trimmed = query.trim();
    let normalized = trimmed.to_uppercase();
    if !(normalized.starts_with("SELECT") || normalized.starts_with("WITH")) {
        return Err(ToolError::InvalidInput(format!(
            "only SELECT/WITH queries are allowed, got: {}",
            normalized.split_whitespace().next().unwrap_or("(empty)")
        )));
    }

    let without_trailing_semicolons =
        trimmed.trim_end_matches(|c: char| c.is_whitespace() || c == ';');
    if without_trailing_semicolons.contains(';') {
        return Err(ToolError::InvalidInput(
            "only a single statement is allowed (no `;`-separated statements)".into(),
        ));
    }

    for token in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if FORBIDDEN_SQL_KEYWORDS.contains(&token) || FORBIDDEN_SQL_FUNCTIONS.contains(&token) {
            return Err(ToolError::InvalidInput(format!(
                "query contains a forbidden keyword: {token}"
            )));
        }
    }

    Ok(())
}

/// Validate that an action or probe name contains only safe characters.
///
/// Allowed characters: ASCII alphanumerics, hyphens (`-`), underscores (`_`),
/// and dots (`.`).  This whitelist prevents SQL injection when the name is
/// interpolated into a query string (e.g., in the `coverage` tool).
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if the name is empty or contains any
/// character outside the allowed set.
pub fn validate_action_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidInput(
            "action name must not be empty".into(),
        ));
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        Ok(())
    } else {
        Err(ToolError::InvalidInput(format!(
            "action name contains invalid characters: {name:?}"
        )))
    }
}

/// Resolve a user-supplied path safely within a base directory.
///
/// Joins `base` with `user_path`, canonicalizes the result, and verifies
/// the resolved path is still within `base`. This prevents directory
/// traversal attacks (e.g., `../../etc/passwd`).
///
/// # Errors
///
/// Returns [`ToolError::Path`] if the path cannot be canonicalized or if the
/// resolved path escapes the base directory.
pub fn safe_resolve_path(base: &Path, user_path: &str) -> Result<PathBuf, ToolError> {
    let candidate = base.join(user_path);
    let resolved = candidate
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("path resolution error: {e}")))?;
    let base_canonical = base
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("base path resolution error: {e}")))?;
    if resolved.starts_with(&base_canonical) {
        Ok(resolved)
    } else {
        Err(ToolError::Path(format!(
            "path traversal detected: resolved path {} is outside base {}",
            resolved.display(),
            base_canonical.display()
        )))
    }
}

/// Resolve a user-supplied *output* path safely within a base directory.
///
/// Unlike [`safe_resolve_path`], the leaf component is allowed to not exist
/// yet (the caller intends to create it). The parent directory is
/// canonicalized and verified to be within `base`, and the leaf name is
/// validated to be a plain file name (no `..`, no separators, no null
/// bytes). If the path already exists — including as a dangling symlink —
/// it is resolved via [`safe_resolve_path`] so symlink escapes are caught
/// and the caller can report its own already-exists error.
///
/// # Errors
///
/// Returns [`ToolError::Path`] if the path contains a null byte, has no
/// valid file name, its parent cannot be canonicalized, or the resolved
/// parent escapes the base directory.
pub fn safe_resolve_output_path(base: &Path, user_path: &str) -> Result<PathBuf, ToolError> {
    if user_path.contains('\0') {
        return Err(ToolError::Path("path contains a null byte".into()));
    }
    let candidate = base.join(user_path);

    // Existing entries (including dangling symlinks) go through the strict
    // resolver: symlink escapes are rejected there, and callers that require
    // a fresh file surface their own already-exists error on the result.
    if candidate.symlink_metadata().is_ok() {
        return safe_resolve_path(base, user_path);
    }

    // `file_name()` returns None for paths ending in `..` or a root,
    // rejecting traversal in the leaf component.
    let file_name = candidate.file_name().ok_or_else(|| {
        ToolError::Path(format!("output path has no valid file name: {user_path}"))
    })?;
    let parent = candidate.parent().ok_or_else(|| {
        ToolError::Path(format!("output path has no parent directory: {user_path}"))
    })?;
    let parent_canonical = parent
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("output directory resolution error: {e}")))?;
    let base_canonical = base
        .canonicalize()
        .map_err(|e| ToolError::Path(format!("base path resolution error: {e}")))?;
    if parent_canonical.starts_with(&base_canonical) {
        Ok(parent_canonical.join(file_name))
    } else {
        Err(ToolError::Path(format!(
            "path traversal detected: output directory {} is outside base {}",
            parent_canonical.display(),
            base_canonical.display()
        )))
    }
}

#[cfg(test)]
mod tests {
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
        assert!(validate_select_only(
            "WITH x AS (SELECT 1) ATTACH 'evil.db' AS evil SELECT * FROM x"
        )
        .is_err());
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
}
