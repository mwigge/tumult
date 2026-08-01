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
mod tests;
