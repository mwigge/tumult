//! SQL guardrails for LLM-generated queries.
//!
//! The pipeline every generated query must pass:
//!
//! 1. **Single statement** — a `;` anywhere is rejected.
//! 2. **Read-only shape** — must start with `SELECT` or `WITH`.
//! 3. **No comments** — `--` and `/* */` are rejected (they hide intent).
//! 4. **Allow-listed tables** — every identifier following `FROM`/`JOIN`
//!    must be in the caller's allow-list.
//! 5. **Allow-listed functions** — every function call (identifier directly
//!    followed by `(`) must be on a fixed allow-list of safe aggregate and
//!    scalar functions. This is what stops `read_text`, `read_csv`, `glob`
//!    and friends from exfiltrating server files through a plain SELECT.
//! 6. [`inject_limit`] — append `LIMIT n` unless the query already has one.
//!
//! [`scope_tables`] rewrites a validated query so every base-table reference
//! is wrapped in a filtering subquery — the mechanism behind per-user
//! environment scoping on `/api/ask`.
//!
//! The tokenizer is a deliberately simple word splitter with string-literal
//! awareness: **best-effort, not a SQL parser**. Defense in depth comes from
//! also running on a read-only connection with a statement timeout (see
//! `docs/adr/0002-ai-layer.md`); this module is the first gate, not the wall.

// Imported from kronika (kronika-ai). Pedantic lints are scoped to
// tumult-native code; this module predates the pedantic gate.
#![allow(clippy::pedantic)]

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
    #[error("function {0:?} is not in the allow-list")]
    FunctionNotAllowed(String),
    #[error("table {0:?} cannot be filtered to the principal's environment scopes")]
    TableNotScopable(String),
}

/// Scalar and aggregate functions generated SQL may call. Everything else —
/// `read_text`, `read_blob`, `glob`, `read_csv`, `read_parquet`, `httpfs`
/// helpers, … — fails validation, closing the file-read exfiltration path
/// that table allow-listing alone leaves open. Extend only after checking
/// the function cannot read the file system, the network, or configuration.
const ALLOWED_FUNCTIONS: &[&str] = &[
    // Aggregates.
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "median",
    "quantile",
    "stddev",
    "stddev_samp",
    "stddev_pop",
    "var_samp",
    "var_pop",
    "variance",
    "any_value",
    "arg_max",
    "arg_min",
    "first",
    "last",
    "list",
    "string_agg",
    // Window functions.
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    // Numeric / null handling.
    "abs",
    "ceil",
    "ceiling",
    "floor",
    "round",
    "sign",
    "greatest",
    "least",
    "coalesce",
    "nullif",
    "cast",
    "try_cast",
    // Text.
    "lower",
    "upper",
    "length",
    "substring",
    "substr",
    "trim",
    "ltrim",
    "rtrim",
    "replace",
    "concat",
    "starts_with",
    "ends_with",
    "contains",
    // Timestamps (epoch-ns arithmetic is the store's convention).
    "date_trunc",
    "date_part",
    "date_diff",
    "strftime",
    "epoch",
    "epoch_ms",
    "now",
    "current_date",
    "current_timestamp",
];

/// Keywords that legitimately precede a parenthesised clause and therefore
/// are not function calls (`FROM (…)`, `IN (…)`, `FILTER (WHERE …)`, …).
const PAREN_KEYWORDS: &[&str] = &[
    "select", "from", "join", "as", "in", "not", "exists", "over", "filter", "values", "where",
    "and", "or", "on", "using", "case", "when", "by", "lateral",
];

/// Keywords that may follow a table reference; anything else word-shaped is
/// treated as an alias (`FROM spans s`).
const RESERVED_AFTER_TABLE: &[&str] = &[
    "WHERE",
    "GROUP",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "HAVING",
    "QUALIFY",
    "WINDOW",
    "ON",
    "USING",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "CROSS",
    "FULL",
    "NATURAL",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "AS",
    "LATERAL",
    "FOR",
];

/// True for tokens that can start a function name (letter or underscore —
/// numbers, `?str?` literal placeholders and punctuation are excluded).
fn is_identifier_start(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
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
    // Step 5: every function call must be on the allow-list — an identifier
    // directly followed by `(` is a call unless it is a keyword that
    // legitimately precedes a parenthesised clause (`FROM (`, `IN (`, …).
    for (i, token) in tokens.iter().enumerate() {
        if !is_identifier_start(token) || tokens.get(i + 1).map(String::as_str) != Some("(") {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if !PAREN_KEYWORDS.contains(&lower.as_str()) && !ALLOWED_FUNCTIONS.contains(&lower.as_str())
        {
            return Err(SqlGuardError::FunctionNotAllowed(token.clone()));
        }
    }
    Ok(())
}

/// A token with its byte span in the source SQL, for splicing rewrites.
struct PosTok {
    text: String,
    start: usize,
    end: usize,
    word: bool,
}

/// [`tokenize`], but keeping byte spans. String literals become non-word
/// `?str?` placeholders (their contents stay invisible to the rewriter).
fn tokenize_positions(sql: &str) -> Vec<PosTok> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start = 0usize;
    let mut in_string = false;
    let mut string_start = 0usize;
    let mut chars = sql.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if in_string {
            if c == '\'' {
                if chars.peek().map(|&(_, p)| p) == Some('\'') {
                    chars.next();
                } else {
                    in_string = false;
                    tokens.push(PosTok {
                        text: "?str?".to_string(),
                        start: string_start,
                        end: i + 1,
                        word: false,
                    });
                }
            }
            continue;
        }
        match c {
            '\'' => {
                if !current.is_empty() {
                    tokens.push(PosTok {
                        text: std::mem::take(&mut current),
                        start: current_start,
                        end: i,
                        word: true,
                    });
                }
                in_string = true;
                string_start = i;
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' => {
                if current.is_empty() {
                    current_start = i;
                }
                current.push(c);
            }
            _ => {
                if !current.is_empty() {
                    tokens.push(PosTok {
                        text: std::mem::take(&mut current),
                        start: current_start,
                        end: i,
                        word: true,
                    });
                }
                if !c.is_whitespace() {
                    tokens.push(PosTok {
                        text: c.to_string(),
                        start: i,
                        end: i + c.len_utf8(),
                        word: false,
                    });
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(PosTok {
            text: current,
            start: current_start,
            end: sql.len(),
            word: true,
        });
    }
    tokens
}

/// Rewrite every base-table reference (`FROM t` / `JOIN t`) in a validated
/// query into a filtering subquery `(SELECT * FROM t WHERE <predicate>)`, so
/// the caller can confine results to rows matching per-table predicates —
/// the per-user environment scoping on `/api/ask`.
///
/// CTE references and subqueries pass through untouched (a CTE's inner
/// `FROM` is rewritten on its own). An explicit alias is kept
/// (`FROM spans s` → `FROM (…) s`); otherwise the subquery is aliased to the
/// bare table name so qualified column references keep resolving. Any
/// referenced base table without a predicate is rejected — callers must
/// either supply a predicate for every allow-listed table or fail closed.
///
/// # Errors
/// Returns [`SqlGuardError::TableNotScopable`] for the first referenced
/// table that has no predicate.
pub fn scope_tables(sql: &str, predicates: &[(&str, &str)]) -> Result<String, SqlGuardError> {
    let tokens = tokenize_positions(sql);
    // Same best-effort CTE rule as the validator: `name AS (`.
    let cte_names: Vec<String> = tokens
        .windows(3)
        .filter(|w| w[0].word && w[1].text.eq_ignore_ascii_case("AS") && w[2].text == "(")
        .map(|w| w[0].text.to_ascii_lowercase())
        .collect();
    let preds: Vec<(String, &str)> = predicates
        .iter()
        .map(|(t, p)| (t.to_ascii_lowercase(), *p))
        .collect();

    let mut out = String::with_capacity(sql.len() + 64 * predicates.len().max(1));
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < tokens.len() {
        let token = &tokens[i];
        let upper = token.text.to_ascii_uppercase();
        if token.word && (upper == "FROM" || upper == "JOIN") {
            if let Some(next) = tokens.get(i + 1) {
                if next.word {
                    let lower = next.text.to_ascii_lowercase();
                    if !cte_names.contains(&lower) {
                        let Some((_, pred)) = preds.iter().find(|(name, _)| *name == lower) else {
                            return Err(SqlGuardError::TableNotScopable(next.text.clone()));
                        };
                        // `FROM t AS s` / `FROM t s`: the alias stays, so the
                        // subquery must not add one of its own.
                        let has_alias = match tokens.get(i + 2) {
                            Some(a) if a.word && a.text.eq_ignore_ascii_case("AS") => true,
                            Some(a) if a.word => !RESERVED_AFTER_TABLE
                                .contains(&a.text.to_ascii_uppercase().as_str()),
                            _ => false,
                        };
                        let alias = next.text.rsplit('.').next().unwrap_or(&next.text);
                        out.push_str(&sql[cursor..next.start]);
                        out.push_str("(SELECT * FROM ");
                        out.push_str(&next.text);
                        out.push_str(" WHERE ");
                        out.push_str(pred);
                        out.push(')');
                        if !has_alias {
                            out.push_str(" AS ");
                            out.push_str(alias);
                        }
                        cursor = next.end;
                        i += 2;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&sql[cursor..]);
    Ok(out)
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

    #[test]
    fn rejects_file_reading_functions() {
        // The exfiltration vector from the security report: a plain SELECT
        // over an allow-listed table calling a file-reading function.
        assert_eq!(
            validate_generated_sql("SELECT read_text('/proc/self/environ') FROM spans", ALLOWED),
            Err(SqlGuardError::FunctionNotAllowed("read_text".into()))
        );
        for f in [
            "read_blob",
            "read_csv",
            "read_parquet",
            "glob",
            "parquet_scan",
            "csv_sniffer",
        ] {
            let sql = format!("SELECT {f}('/etc/passwd') FROM spans");
            assert!(
                matches!(
                    validate_generated_sql(&sql, ALLOWED),
                    Err(SqlGuardError::FunctionNotAllowed(_))
                ),
                "{f} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_allowlisted_functions() {
        assert!(validate_generated_sql(
            "SELECT COUNT(*), AVG(duration_ns), date_trunc('day', now()) FROM spans \
             GROUP BY 3",
            ALLOWED
        )
        .is_ok());
        assert!(validate_generated_sql(
            "SELECT SUM(CAST(value AS DOUBLE)) FILTER (WHERE outcome_status = 'success') \
             / NULLIF(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums",
            ALLOWED
        )
        .is_ok());
    }

    #[test]
    fn keywords_before_parens_are_not_function_calls() {
        assert!(validate_generated_sql(
            "SELECT count(*) FROM spans WHERE span_name IN ('a', 'b') \
             AND EXISTS (SELECT 1 FROM logs)",
            ALLOWED
        )
        .is_ok());
        assert!(validate_generated_sql(
            "SELECT count(*) OVER (PARTITION BY service_name) FROM spans",
            ALLOWED
        )
        .is_ok());
    }

    const SCOPE: &[(&str, &str)] = &[("spans", "target_environment IN ('dev')")];

    #[test]
    fn scope_tables_wraps_plain_references() {
        let scoped = scope_tables("SELECT count(*) FROM spans", SCOPE).unwrap();
        assert_eq!(
            scoped,
            "SELECT count(*) FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) \
             AS spans"
        );
    }

    #[test]
    fn scope_tables_keeps_explicit_aliases() {
        let scoped = scope_tables(
            "SELECT s.experiment_id FROM spans s ORDER BY s.ts_ns DESC",
            SCOPE,
        )
        .unwrap();
        assert_eq!(
            scoped,
            "SELECT s.experiment_id FROM (SELECT * FROM spans WHERE \
             target_environment IN ('dev')) s ORDER BY s.ts_ns DESC"
        );
        let scoped = scope_tables("SELECT * FROM spans AS s", SCOPE).unwrap();
        assert_eq!(
            scoped,
            "SELECT * FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) AS s"
        );
    }

    #[test]
    fn scope_tables_rewrites_inside_ctes_and_subqueries() {
        let scoped = scope_tables(
            "WITH x AS (SELECT * FROM spans) SELECT count(*) FROM x",
            SCOPE,
        )
        .unwrap();
        assert_eq!(
            scoped,
            "WITH x AS (SELECT * FROM (SELECT * FROM spans WHERE \
             target_environment IN ('dev')) AS spans) SELECT count(*) FROM x"
        );
        let scoped = scope_tables(
            "SELECT * FROM (SELECT experiment_id FROM spans) WHERE 1 = 1",
            SCOPE,
        )
        .unwrap();
        assert_eq!(
            scoped,
            "SELECT * FROM (SELECT experiment_id FROM (SELECT * FROM spans WHERE \
             target_environment IN ('dev')) AS spans) WHERE 1 = 1"
        );
    }

    #[test]
    fn scope_tables_rewrites_joins() {
        let preds: &[(&str, &str)] =
            &[("spans", "target_environment IN ('dev')"), ("logs", "true")];
        let scoped = scope_tables("SELECT * FROM spans s JOIN logs l ON true", preds).unwrap();
        assert_eq!(
            scoped,
            "SELECT * FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) s \
             JOIN (SELECT * FROM logs WHERE true) l ON true"
        );
    }

    #[test]
    fn scope_tables_rejects_tables_without_a_predicate() {
        assert_eq!(
            scope_tables("SELECT * FROM spans JOIN logs ON true", SCOPE),
            Err(SqlGuardError::TableNotScopable("logs".into()))
        );
    }

    #[test]
    fn scope_tables_ignores_keywords_inside_string_literals() {
        let scoped = scope_tables(
            "SELECT * FROM spans WHERE body = 'from logs join secrets'",
            SCOPE,
        )
        .unwrap();
        assert_eq!(
            scoped,
            "SELECT * FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) AS spans \
             WHERE body = 'from logs join secrets'"
        );
    }
}
