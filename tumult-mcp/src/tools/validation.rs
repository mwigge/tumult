//! Input validation and path-safety helpers for MCP tools.

use std::path::{Path, PathBuf};

use crate::error::ToolError;

/// Keywords that introduce a write or schema/configuration change. Rejected
/// as standalone tokens anywhere in the query, since DuckDB allows DML/DDL
/// after a leading `WITH` CTE (e.g. `WITH x AS (SELECT 1) INSERT INTO ...`).
const FORBIDDEN_SQL_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "ATTACH", "DETACH", "COPY", "EXPORT",
    "IMPORT", "INSTALL", "LOAD", "PRAGMA", "SET", "CALL", "VACUUM", "TRUNCATE",
];

/// Validate that a SQL query is read-only (SELECT or WITH only).
///
/// Prevents SQL injection by rejecting any query that does not start
/// with SELECT or WITH (e.g., DROP, INSERT, UPDATE, DELETE, CREATE), any
/// query containing more than one statement (stacked statements via `;`),
/// and any query containing a write/DDL keyword as a standalone token (to
/// catch DML smuggled in after a `WITH` CTE).
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if the query does not start with
/// `SELECT` or `WITH`, contains more than one statement, or contains a
/// forbidden keyword.
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
        if FORBIDDEN_SQL_KEYWORDS.contains(&token) {
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
}
