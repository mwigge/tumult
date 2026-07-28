//! SQL guardrails for LLM-generated queries.
//!
//! The pipeline every generated query must pass:
//!
//! 1. **Single statement** — a `;` anywhere is rejected.
//! 2. **Read-only shape** — must start with `SELECT` or `WITH`.
//! 3. **No comments** — `--` and `/* */` are rejected (they hide intent).
//! 4. **Allow-listed tables** — every identifier following `FROM`/`JOIN`
//!    must be in the caller's allow-list.
//! 5. [`inject_limit`] — append `LIMIT n` unless the query already has one.
//!
//! The tokenizer is a deliberately simple word splitter with string-literal
//! awareness: **best-effort, not a SQL parser**. Defense in depth comes from
//! also running on a read-only connection with a statement timeout (see
//! `docs/adr/0002-ai-layer.md`); this module is the first gate, not the wall.

/// Why a generated query was rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SqlGuardError {
    #[error("empty SQL")]
    Empty,
    #[error("query must start with SELECT or WITH")]
    NotASelect,
    #[error("multiple statements are not allowed (';' found)")]
    MultipleStatements,
    #[error("SQL comments are not allowed ('--' or '/* */' found)")]
    CommentFound,
    #[error("table {0:?} is not in the allow-list")]
    TableNotAllowed(String),
}

/// Split SQL into word tokens, tracking single-quoted string literals.
/// String-literal *contents* are dropped (returned as the token `?str?`) so
/// that punctuation inside literals cannot trip the structural checks.
fn tokenize(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\'' {
                // '' inside a string is an escaped quote — stay in the string.
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_string = false;
                    tokens.push("?str?".to_string());
                }
            }
            continue;
        }
        match c {
            '\'' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                in_string = true;
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' => current.push(c),
            _ => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                if !c.is_whitespace() {
                    tokens.push(c.to_string());
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Scan for comment markers and semicolons outside string literals.
fn scan_raw(sql: &str) -> Result<(), SqlGuardError> {
    let mut in_string = false;
    let mut prev = '\0';
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_string = false;
                }
            }
        } else {
            match c {
                '\'' => in_string = true,
                ';' => return Err(SqlGuardError::MultipleStatements),
                '-' if prev == '-' => return Err(SqlGuardError::CommentFound),
                '*' if prev == '/' => return Err(SqlGuardError::CommentFound),
                '/' if prev == '*' => return Err(SqlGuardError::CommentFound),
                _ => {}
            }
        }
        prev = c;
    }
    Ok(())
}

/// Validate LLM-generated SQL against the guardrail pipeline (steps 1–4).
///
/// # Errors
/// Returns the first [`SqlGuardError`] the query trips.
pub fn validate_generated_sql(sql: &str, allowed_tables: &[&str]) -> Result<(), SqlGuardError> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err(SqlGuardError::Empty);
    }
    scan_raw(trimmed)?;

    let tokens = tokenize(trimmed);
    let first = tokens
        .first()
        .map_or("", String::as_str)
        .to_ascii_uppercase();
    if first != "SELECT" && first != "WITH" {
        return Err(SqlGuardError::NotASelect);
    }

    let allowed: Vec<String> = allowed_tables
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    // CTE names (`WITH x AS (…)` / `, y AS (…)`) are not tables; collect
    // them so references to the CTE pass the allow-list. Best-effort: any
    // identifier directly before `AS (` is treated as a CTE name.
    let cte_names: Vec<String> = tokens
        .windows(3)
        .filter(|w| w[1].eq_ignore_ascii_case("AS") && w[2] == "(")
        .map(|w| w[0].to_ascii_lowercase())
        .collect();
    let mut expect_table = false;
    for token in &tokens {
        let upper = token.to_ascii_uppercase();
        if expect_table {
            if token == "(" {
                // Subquery: its own FROM/JOIN will be checked as we continue.
                expect_table = false;
                continue;
            }
            expect_table = false;
            let lower = token.to_ascii_lowercase();
            if !allowed.contains(&lower) && !cte_names.contains(&lower) {
                return Err(SqlGuardError::TableNotAllowed(token.clone()));
            }
        } else if upper == "FROM" || upper == "JOIN" {
            expect_table = true;
        }
    }
    Ok(())
}

/// Append `LIMIT n` unless the query already carries one.
#[must_use]
pub fn inject_limit(sql: &str, n: u64) -> String {
    let has_limit = tokenize(sql)
        .iter()
        .any(|t| t.eq_ignore_ascii_case("LIMIT"));
    if has_limit {
        sql.to_string()
    } else {
        format!("{} LIMIT {n}", sql.trim_end_matches(';').trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: &[&str] = &["spans", "logs", "metric_sums", "experiment_runs"];

    #[test]
    fn accepts_simple_select() {
        assert!(validate_generated_sql("SELECT count(*) FROM spans", ALLOWED).is_ok());
        assert!(
            validate_generated_sql("WITH x AS (SELECT * FROM spans) SELECT * FROM x", ALLOWED)
                .is_ok()
        );
    }

    #[test]
    fn rejects_non_select() {
        assert_eq!(
            validate_generated_sql("DROP TABLE spans", ALLOWED),
            Err(SqlGuardError::NotASelect)
        );
        assert_eq!(
            validate_generated_sql("DELETE FROM spans", ALLOWED),
            Err(SqlGuardError::NotASelect)
        );
    }

    #[test]
    fn rejects_multiple_statements() {
        assert_eq!(
            validate_generated_sql("SELECT 1 FROM spans; DROP TABLE spans", ALLOWED),
            Err(SqlGuardError::MultipleStatements)
        );
    }

    #[test]
    fn rejects_comments() {
        assert_eq!(
            validate_generated_sql("SELECT * FROM spans -- oops", ALLOWED),
            Err(SqlGuardError::CommentFound)
        );
        assert_eq!(
            validate_generated_sql("SELECT * FROM spans /* hidden */", ALLOWED),
            Err(SqlGuardError::CommentFound)
        );
    }

    #[test]
    fn rejects_non_allowlisted_tables() {
        assert_eq!(
            validate_generated_sql("SELECT * FROM pg_catalog.pg_tables", ALLOWED),
            Err(SqlGuardError::TableNotAllowed(
                "pg_catalog.pg_tables".into()
            ))
        );
        assert_eq!(
            validate_generated_sql("SELECT * FROM spans JOIN secrets ON true", ALLOWED),
            Err(SqlGuardError::TableNotAllowed("secrets".into()))
        );
    }

    #[test]
    fn punctuation_inside_string_literals_is_safe() {
        assert!(validate_generated_sql(
            "SELECT * FROM spans WHERE outcome_status = 'it''s fine; -- no'",
            ALLOWED
        )
        .is_ok());
    }

    #[test]
    fn inject_limit_appends_once() {
        assert_eq!(
            inject_limit("SELECT * FROM spans", 100),
            "SELECT * FROM spans LIMIT 100"
        );
        let already = "SELECT * FROM spans LIMIT 5";
        assert_eq!(inject_limit(already, 100), already);
    }
}
