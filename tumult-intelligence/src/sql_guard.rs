//! SQL guardrails for LLM-generated queries.
//!
//! The pipeline every generated query must pass:
//!
//! 1. **Single statement** — a `;` anywhere is rejected.
//! 2. **Read-only shape** — must start with `SELECT` or `WITH`.
//! 3. **No comments** — `--` and `/* */` are rejected (they hide intent).
//!    So is SQL the tokenizer would read otherwise than DuckDB: a
//!    dollar-quoted string, an escape string (`E'…'`) and a word glued to
//!    a number (`1FROM`).
//! 4. **Allow-listed tables** — every table a FROM clause names, at any
//!    depth, must be in the caller's allow-list or a CTE in scope: the
//!    source after `FROM`, after `JOIN` and after each comma of a FROM
//!    list ([`table_refs`]). A FROM clause the guard cannot read fails
//!    closed.
//! 5. **Allow-listed functions** — every function call (a name directly
//!    followed by `(`) must be on a fixed allow-list of safe aggregate and
//!    scalar functions. This is what stops `read_text`, `read_csv`, `glob`
//!    and friends from exfiltrating server files through a plain SELECT.
//! 6. [`inject_limit`] — append `LIMIT n` unless the query already has one.
//!
//! [`scope_tables`] rewrites a validated query so every base-table reference
//! is wrapped in a filtering subquery — the mechanism behind per-user
//! environment scoping on `/api/ask`. It finds the tables with the same
//! walk as step 4.
//!
//! The tokenizer is a deliberately simple word splitter that knows string
//! literals and quoted identifiers: **best-effort, not a SQL parser**.
//! Defense in depth comes from also running on a read-only connection with
//! a statement timeout (see the AI-layer ADR); this module is the first
//! gate, not the wall.

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
    #[error("the guard cannot read {0:?}")]
    Unreadable(String),
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
    "select",
    "from",
    "join",
    "as",
    "in",
    "not",
    "exists",
    "over",
    "filter",
    "values",
    "where",
    "and",
    "or",
    "on",
    "using",
    "case",
    "when",
    "by",
    "lateral",
    "materialized",
];

/// True for tokens that can start a function name (letter or underscore —
/// numbers, `?str?` literal placeholders and punctuation are excluded).
fn is_identifier_start(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
}

/// True for a word that starts like a number but holds more than one:
/// DuckDB reads `1FROM` as the number `1` and the word `FROM`, where the
/// tokenizer sees one word.
fn glues_a_word_to_a_number(token: &str) -> bool {
    token.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && !token
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | 'e' | 'E'))
}

/// The token texts of [`tokenize_positions`], which owns the splitting
/// rule, so the validator checks the tokens that [`scope_tables`] rewrites.
fn tokenize(sql: &str) -> Vec<String> {
    tokenize_positions(sql)
        .into_iter()
        .map(|t| t.text)
        .collect()
}

/// Scan for comment markers and semicolons outside string literals and
/// quoted identifiers, and refuse the literals the tokenizer cannot read:
/// a dollar-quoted string (`$$…$$`) and an escape string (`E'…'`, which
/// `\'` does not end).
fn scan_raw(sql: &str) -> Result<(), SqlGuardError> {
    // The quote of the literal (`'`) or identifier (`"`, `` ` ``) being read.
    let mut quote: Option<char> = None;
    let mut prev = '\0';
    let mut before_prev = '\0';
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                if chars.peek() == Some(&q) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else {
            match c {
                '$' => return Err(SqlGuardError::Unreadable("$".into())),
                '\'' if matches!(prev, 'e' | 'E')
                    && !(before_prev.is_alphanumeric() || before_prev == '_') =>
                {
                    return Err(SqlGuardError::Unreadable(format!("{prev}'")));
                }
                '\'' | '"' | '`' => quote = Some(c),
                ';' => return Err(SqlGuardError::MultipleStatements),
                '-' if prev == '-' => return Err(SqlGuardError::CommentFound),
                '*' if prev == '/' => return Err(SqlGuardError::CommentFound),
                '/' if prev == '*' => return Err(SqlGuardError::CommentFound),
                _ => {}
            }
        }
        before_prev = prev;
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

    // DuckDB reads a number and then the word glued to it.
    if let Some(glued) = tokens.iter().find(|t| glues_a_word_to_a_number(t)) {
        return Err(SqlGuardError::Unreadable(glued.clone()));
    }
    let allowed: Vec<String> = allowed_tables
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    for table in table_refs(&tokens)? {
        let name = &tokens[table.index];
        if !table.cte && !allowed.contains(&name.to_ascii_lowercase()) {
            return Err(SqlGuardError::TableNotAllowed(name.clone()));
        }
    }
    // Step 5: every function call must be on the allow-list — a name
    // directly followed by `(` is a call unless it is a keyword that
    // legitimately precedes a parenthesised clause (`FROM (`, `IN (`, …).
    // A quoted name (`"read_text"(…)`) calls the function too.
    for (i, token) in tokens.iter().enumerate() {
        let named = is_identifier_start(token) || matches!(token.chars().next(), Some('"' | '`'));
        if !named || tokens.get(i + 1).map(String::as_str) != Some("(") {
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
}

/// Split SQL into tokens with their byte spans: words (letters, digits,
/// `_` and `.`), single punctuation characters, single-quoted string
/// literals and identifiers quoted with `"` or `` ` `` (a doubled quote
/// inside any of them is an escaped quote). A string literal becomes the
/// placeholder `?str?`, so punctuation inside it cannot trip the structural
/// checks and its contents stay invisible to the rewriter. A quoted
/// identifier keeps its text, quotes included: it is one name, and never a
/// keyword. An unterminated literal or identifier yields no token.
fn tokenize_positions(sql: &str) -> Vec<PosTok> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start = 0usize;
    // The quote of the literal or identifier being read, and where it began.
    let mut quoted: Option<(char, usize)> = None;
    let mut chars = sql.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if let Some((quote, start)) = quoted {
            if c == quote {
                if chars.peek().map(|&(_, p)| p) == Some(quote) {
                    chars.next();
                } else {
                    quoted = None;
                    let text = if quote == '\'' {
                        "?str?".to_string()
                    } else {
                        sql[start..=i].to_string()
                    };
                    tokens.push(PosTok {
                        text,
                        start,
                        end: i + 1,
                    });
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                if !current.is_empty() {
                    tokens.push(PosTok {
                        text: std::mem::take(&mut current),
                        start: current_start,
                        end: i,
                    });
                }
                quoted = Some((c, i));
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
                    });
                }
                if !c.is_whitespace() {
                    tokens.push(PosTok {
                        text: c.to_string(),
                        start: i,
                        end: i + c.len_utf8(),
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
        });
    }
    tokens
}

/// A table a FROM clause names, as [`table_refs`] finds it.
struct TableRef {
    /// Where the name is in the token stream.
    index: usize,
    /// The name is a CTE in scope there, not a table.
    cte: bool,
    /// An alias follows the name (`FROM spans s`, `FROM spans AS s`).
    alias: bool,
}

/// Where [`table_refs`] is, within one level of parentheses.
#[derive(Default)]
enum Clause {
    /// Outside a FROM clause: the select list, `WHERE`, `GROUP BY`, …
    #[default]
    Other,
    /// A join condition (`ON …`, `USING (…)`): a comma at this level
    /// starts the next source of the FROM list.
    Condition,
    /// A source comes next: after `FROM`, `JOIN` or a comma of a FROM list.
    Source,
    /// The first token inside `FROM (`: a subquery or a parenthesised join.
    SourceOpen,
    /// After a source; `Some(n)` when it is the table `refs[n]`.
    AfterSource(Option<usize>),
    /// After `AS`: the alias of the source comes next.
    Alias,
    /// After the alias of a source.
    AfterAlias,
    /// Inside a join type (`LEFT OUTER`, `NATURAL`, …): `JOIN` comes next.
    JoinType,
    /// After `WITH` or a comma of its list: the name of a CTE comes next.
    CteName,
    /// After the name of a CTE: `AS` comes next.
    CteAs(String),
    /// After `name AS`: `[NOT] MATERIALIZED`, then the body in parentheses.
    CteBody(String),
    /// After the body of a CTE: a comma and the next CTE, or the query.
    CteNext,
}

/// One level of parentheses of the walk of [`table_refs`].
#[derive(Default)]
struct Level {
    clause: Clause,
    /// The CTEs this level defines: in scope here and in deeper levels.
    ctes: Vec<String>,
    /// The CTE whose body this level is. Its name comes into scope one
    /// level up at the closing `)`, so inside the body it names the table.
    defines: Option<String>,
}

/// The words a table name follows: `FROM`, `JOIN`, and the DuckDB
/// statements that read a table without `FROM` (`TABLE t`, `SUMMARIZE t`,
/// `PIVOT t ON …`), which DuckDB also takes as a subquery. `DESC t` is one
/// as well, but only as the first word inside parentheses: after an
/// expression `DESC` sorts.
const TABLE_WORDS: &[&str] = &[
    "FROM",
    "JOIN",
    "TABLE",
    "SUMMARIZE",
    "DESCRIBE",
    "SHOW",
    "PIVOT",
    "PIVOT_WIDER",
    "PIVOT_LONGER",
    "UNPIVOT",
];

/// The words of a join type before `JOIN`.
const JOIN_TYPES: &[&str] = &[
    "LEFT",
    "RIGHT",
    "FULL",
    "INNER",
    "OUTER",
    "CROSS",
    "NATURAL",
    "ASOF",
    "POSITIONAL",
    "SEMI",
    "ANTI",
];

/// The words that end a FROM clause and start the next clause (`SELECT`
/// for DuckDB's `FROM t SELECT …`).
const FROM_ENDS: &[&str] = &[
    "WHERE",
    "GROUP",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "HAVING",
    "QUALIFY",
    "WINDOW",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "SELECT",
];

/// What a token after a source of a FROM list starts: the next source
/// (`,`), a join, a join condition or the next clause. `None` for anything
/// else.
fn after_source(upper: &str) -> Option<Clause> {
    if upper == "," {
        return Some(Clause::Source);
    }
    if JOIN_TYPES.contains(&upper) {
        return Some(Clause::JoinType);
    }
    if upper == "ON" || upper == "USING" {
        return Some(Clause::Condition);
    }
    FROM_ENDS.contains(&upper).then_some(Clause::Other)
}

/// Every table the FROM clauses of `tokens` name, at any depth: the source
/// after `FROM`, after `JOIN` (and the other [`TABLE_WORDS`]) and after
/// each comma of a FROM list, and the first source inside a parenthesised
/// join. A name is a CTE when a `WITH` of its level, or of a level around
/// it, defined it earlier: the body of a CTE names the table, never the
/// CTE itself.
///
/// # Errors
/// [`SqlGuardError::TableNotAllowed`] with the first token of a FROM clause
/// or a WITH list that is not a name, an alias, a join or the end of the
/// clause (`LATERAL`, a table function, a quoted alias, a column list, …):
/// what the walk cannot read fails closed.
fn table_refs<S: AsRef<str>>(tokens: &[S]) -> Result<Vec<TableRef>, SqlGuardError> {
    let mut refs: Vec<TableRef> = Vec::new();
    let mut outer: Vec<Level> = Vec::new();
    let mut level = Level::default();
    let mut last = "";
    for (index, token) in tokens.iter().enumerate() {
        let token = token.as_ref();
        let prev = std::mem::replace(&mut last, token);
        let upper = token.to_ascii_uppercase();
        let refuse = || SqlGuardError::TableNotAllowed(token.to_string());
        if TABLE_WORDS.contains(&upper.as_str()) || (upper == "DESC" && prev == "(") {
            level.clause = Clause::Source;
            continue;
        }
        if token == ")" {
            if let Some(up) = outer.pop() {
                let body = std::mem::replace(&mut level, up);
                level.ctes.extend(body.defines);
            }
            continue;
        }
        let clause = match std::mem::take(&mut level.clause) {
            Clause::SourceOpen if upper == "SELECT" || upper == "WITH" => Clause::Other,
            Clause::SourceOpen => Clause::Source,
            Clause::CteNext if token != "," => Clause::Other,
            clause => clause,
        };
        let mut inner = None;
        level.clause = match clause {
            c @ (Clause::Other | Clause::Condition) if token == "(" => {
                inner = Some(Level::default());
                c
            }
            Clause::Other if upper == "WITH" => Clause::CteName,
            Clause::Other => Clause::Other,
            Clause::Condition => after_source(&upper).unwrap_or(Clause::Condition),
            Clause::Source if token == "(" => {
                inner = Some(Level {
                    clause: Clause::SourceOpen,
                    ..Level::default()
                });
                Clause::AfterSource(None)
            }
            Clause::Source => {
                let lower = token.to_ascii_lowercase();
                let cte = outer
                    .iter()
                    .chain([&level])
                    .any(|l| l.ctes.contains(&lower));
                refs.push(TableRef {
                    index,
                    cte,
                    alias: false,
                });
                Clause::AfterSource(Some(refs.len() - 1))
            }
            Clause::AfterSource(table) => match after_source(&upper) {
                Some(next) => next,
                None if upper == "AS" || is_identifier_start(token) => {
                    if let Some(n) = table {
                        refs[n].alias = true;
                    }
                    if upper == "AS" {
                        Clause::Alias
                    } else {
                        Clause::AfterAlias
                    }
                }
                None => return Err(refuse()),
            },
            Clause::Alias if is_identifier_start(token) => Clause::AfterAlias,
            Clause::AfterAlias => after_source(&upper).ok_or_else(refuse)?,
            Clause::JoinType if JOIN_TYPES.contains(&upper.as_str()) => Clause::JoinType,
            Clause::CteName => Clause::CteAs(token.to_ascii_lowercase()),
            Clause::CteAs(name) if upper == "AS" => Clause::CteBody(name),
            Clause::CteBody(name) if upper == "NOT" || upper == "MATERIALIZED" => {
                Clause::CteBody(name)
            }
            Clause::CteBody(name) if token == "(" => {
                inner = Some(Level {
                    defines: Some(name),
                    ..Level::default()
                });
                Clause::CteNext
            }
            Clause::CteNext => Clause::CteName,
            _ => return Err(refuse()),
        };
        if let Some(inner) = inner {
            outer.push(std::mem::replace(&mut level, inner));
        }
    }
    Ok(refs)
}

/// Rewrite every base-table reference of a validated query (each table
/// [`table_refs`] finds that is not a CTE) into a filtering subquery
/// `(SELECT * FROM t WHERE <predicate>)`, so the caller can confine results
/// to rows matching per-table predicates — the per-user environment scoping
/// on `/api/ask`.
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
/// table that has no predicate, and the error of [`table_refs`] for a FROM
/// clause the walk cannot read (one [`validate_generated_sql`] refuses).
pub fn scope_tables(sql: &str, predicates: &[(&str, &str)]) -> Result<String, SqlGuardError> {
    let tokens = tokenize_positions(sql);
    let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
    let tables = table_refs(&texts)?;
    let preds: Vec<(String, &str)> = predicates
        .iter()
        .map(|(t, p)| (t.to_ascii_lowercase(), *p))
        .collect();

    let mut out = String::with_capacity(sql.len() + 64 * predicates.len().max(1));
    let mut cursor = 0usize;
    for table in tables.iter().filter(|t| !t.cte) {
        let name = &tokens[table.index];
        let lower = name.text.to_ascii_lowercase();
        let Some((_, pred)) = preds.iter().find(|(n, _)| *n == lower) else {
            return Err(SqlGuardError::TableNotScopable(name.text.clone()));
        };
        let alias = name.text.rsplit('.').next().unwrap_or(&name.text);
        out.push_str(&sql[cursor..name.start]);
        out.push_str("(SELECT * FROM ");
        out.push_str(&name.text);
        out.push_str(" WHERE ");
        out.push_str(pred);
        out.push(')');
        if !table.alias {
            out.push_str(" AS ");
            out.push_str(alias);
        }
        cursor = name.end;
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
    fn the_validator_and_the_scoper_split_sql_into_the_same_tokens() {
        // A quoted string, an escaped quote and a string that never ends:
        // `validate_generated_sql` and `scope_tables` must see one stream.
        for (sql, expected) in [
            (
                "SELECT 'a, b' FROM spans",
                vec!["SELECT", "?str?", "FROM", "spans"],
            ),
            (
                "SELECT 'it''s' FROM spans",
                vec!["SELECT", "?str?", "FROM", "spans"],
            ),
            (
                "SELECT * FROM spans WHERE x = 'from logs",
                vec!["SELECT", "*", "FROM", "spans", "WHERE", "x", "="],
            ),
            (
                "SELECT 1 AS \"it's\" FROM spans",
                vec!["SELECT", "1", "AS", "\"it's\"", "FROM", "spans"],
            ),
            (
                "SELECT 1 AS \"a\"\"b\" FROM spans",
                vec!["SELECT", "1", "AS", "\"a\"\"b\"", "FROM", "spans"],
            ),
            (
                "SELECT 1 AS `it's` FROM spans",
                vec!["SELECT", "1", "AS", "`it's`", "FROM", "spans"],
            ),
        ] {
            let positioned = tokenize_positions(sql);
            let texts: Vec<&str> = positioned.iter().map(|t| t.text.as_str()).collect();
            assert_eq!(tokenize(sql), texts, "{sql}");
            assert_eq!(texts, expected, "{sql}");
            for token in positioned.iter().filter(|t| t.text != "?str?") {
                assert_eq!(&sql[token.start..token.end], token.text, "{sql}");
            }
        }
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

    /// `sql` is refused because of `table`, a name off the allow-list.
    fn assert_table_refused(sql: &str, table: &str) {
        assert_eq!(
            validate_generated_sql(sql, ALLOWED),
            Err(SqlGuardError::TableNotAllowed(table.into())),
            "{sql}"
        );
    }

    /// `sql` passes the guard.
    fn assert_accepted(sql: &str) {
        let outcome = validate_generated_sql(sql, ALLOWED);
        assert!(outcome.is_ok(), "{sql}: {outcome:?}");
    }

    #[test]
    fn every_table_of_a_comma_list_is_checked() {
        for sql in [
            "SELECT count(*) FROM spans, secrets",
            "SELECT x.a FROM spans s, secrets x",
            "SELECT x.a FROM spans AS s, secrets AS x",
            "SELECT count(*) FROM spans, logs, secrets",
            "SELECT * FROM spans WHERE trace_id IN (SELECT l.trace_id FROM logs l, secrets x)",
            "WITH c AS (SELECT * FROM spans, secrets) SELECT count(*) FROM c",
        ] {
            assert_table_refused(sql, "secrets");
        }
    }

    #[test]
    fn a_comma_list_of_allowed_tables_passes() {
        for sql in [
            "SELECT count(*) FROM spans s, logs l WHERE s.trace_id = l.trace_id",
            "SELECT count(*) FROM spans, logs",
            "WITH x AS (SELECT * FROM spans) SELECT count(*) FROM x, logs",
            "SELECT service_name, count(*) FROM spans GROUP BY service_name, span_name \
             ORDER BY 2, 1 LIMIT 5",
            "SELECT * FROM spans s LEFT OUTER JOIN logs l ON s.trace_id = l.trace_id \
             ORDER BY s.ts_ns, l.ts_ns",
            "SELECT * FROM spans s JOIN logs l USING (trace_id, span_id), metric_sums m",
            "SELECT * FROM spans NATURAL JOIN logs",
            "SELECT * FROM (spans s JOIN logs l ON true), metric_sums",
            "SELECT * FROM (SELECT * FROM spans) AS t, logs",
            "SELECT * FROM (WITH x AS (SELECT 1 AS a) SELECT a FROM x) t",
            "WITH x AS (FROM spans SELECT experiment_id) SELECT * FROM x",
            "SELECT * FROM spans USING SAMPLE 10",
        ] {
            assert_accepted(sql);
        }
    }

    #[test]
    fn every_way_of_naming_a_second_table_is_checked() {
        for (sql, table) in [
            ("SELECT * FROM spans NATURAL JOIN secrets", "secrets"),
            ("SELECT * FROM spans CROSS JOIN secrets", "secrets"),
            (
                "SELECT * FROM spans s LEFT OUTER JOIN secrets x ON true",
                "secrets",
            ),
            ("SELECT * FROM spans, logs JOIN secrets ON true", "secrets"),
            (
                "SELECT * FROM spans s JOIN logs l ON s.trace_id = l.trace_id, secrets x",
                "secrets",
            ),
            (
                "SELECT * FROM spans JOIN logs USING (trace_id), secrets",
                "secrets",
            ),
            ("SELECT * FROM (SELECT * FROM spans) t, secrets", "secrets"),
            ("SELECT * FROM (secrets CROSS JOIN spans)", "secrets"),
            ("SELECT * FROM spans, (secrets)", "secrets"),
            (
                "SELECT * FROM spans WHERE trace_id IN (FROM secrets)",
                "secrets",
            ),
            ("SELECT * FROM SPANS, SeCrEtS", "SeCrEtS"),
            ("SELECT * FROM spans, main.secrets", "main.secrets"),
            ("SELECT * FROM spans, \"secrets\"", "\"secrets\""),
            ("SELECT * FROM spans, `secrets`", "`secrets`"),
            ("SELECT * FROM \"secrets\"", "\"secrets\""),
        ] {
            assert_table_refused(sql, table);
        }
    }

    #[test]
    fn statements_that_read_a_table_without_from_are_checked() {
        for sql in [
            "SELECT (TABLE secrets) AS a",
            "SELECT experiment_id FROM spans UNION TABLE secrets",
            "WITH c AS (SUMMARIZE secrets) SELECT * FROM c",
            "SELECT EXISTS (DESCRIBE secrets) AS e",
            "SELECT EXISTS (DESC secrets) AS e",
            "SELECT EXISTS (SHOW secrets) AS e",
            "SELECT EXISTS (PIVOT secrets ON a) AS e",
            "SELECT EXISTS (PIVOT_WIDER secrets ON a) AS e",
            "SELECT EXISTS (PIVOT_LONGER secrets ON a) AS e",
            "SELECT EXISTS (UNPIVOT secrets ON a) AS e",
        ] {
            assert_table_refused(sql, "secrets");
        }
        // DESC after an expression sorts: it names no table.
        assert_accepted("SELECT * FROM spans ORDER BY ts_ns DESC LIMIT 5");
    }

    #[test]
    fn a_from_list_the_guard_cannot_read_fails_closed() {
        for (sql, token) in [
            // `LATERAL` and a table function read as a table name that is
            // followed by `(`.
            ("SELECT * FROM spans s, LATERAL (SELECT 1) x", "("),
            ("SELECT * FROM spans, range(3)", "("),
            ("SELECT * FROM spans, 'spans.csv'", "?str?"),
            ("SELECT * FROM spans s x", "x"),
            ("SELECT * FROM spans \"s\"", "\"s\""),
            ("SELECT * FROM spans AS \"s\"", "\"s\""),
            ("SELECT * FROM spans s(a)", "("),
            ("SELECT * FROM spans LEFT logs", "logs"),
            ("WITH x(a) AS (SELECT 1) SELECT a FROM x", "("),
            ("WITH x (SELECT 1) SELECT 1", "("),
            ("WITH x AS SELECT 1", "SELECT"),
        ] {
            assert_table_refused(sql, token);
        }
    }

    #[test]
    fn a_cte_name_covers_only_the_query_of_its_with() {
        for sql in [
            // The CTE lives in the subquery: the outer `secrets` is the table.
            "SELECT * FROM secrets WHERE EXISTS (WITH secrets AS (SELECT 1 AS a) \
             SELECT a FROM secrets)",
            // The body of a CTE names the table, not the CTE it defines…
            "WITH secrets AS (SELECT * FROM secrets) SELECT * FROM secrets",
            "WITH RECURSIVE secrets AS (SELECT * FROM secrets) SELECT * FROM secrets",
            // …nor a CTE defined after it.
            "WITH c AS (SELECT * FROM secrets), secrets AS (SELECT 1 AS a) SELECT * FROM c",
        ] {
            assert_table_refused(sql, "secrets");
        }
        assert_table_refused(
            "SELECT * FROM spans WHERE EXISTS (WITH x AS (SELECT 1 AS a) SELECT a FROM x) \
             AND EXISTS (SELECT 1 FROM x)",
            "x",
        );
        for sql in [
            "WITH x AS (SELECT * FROM spans), y AS (SELECT * FROM x) SELECT * FROM x, y",
            "WITH x AS MATERIALIZED (SELECT * FROM spans) SELECT * FROM x",
            "WITH x AS NOT MATERIALIZED (SELECT * FROM spans) SELECT * FROM x",
            "SELECT * FROM spans WHERE EXISTS (WITH x AS (SELECT 1 AS a) SELECT a FROM x)",
        ] {
            assert_accepted(sql);
        }
    }

    #[test]
    fn a_quoted_identifier_is_one_token() {
        // A quote inside a quoted identifier starts no string literal.
        assert_table_refused("SELECT a AS \"it's\" FROM secrets", "secrets");
        assert_table_refused("SELECT a AS `it's` FROM secrets", "secrets");
        assert_eq!(
            validate_generated_sql("SELECT 1 AS \"it's\" FROM spans -- x", ALLOWED),
            Err(SqlGuardError::CommentFound)
        );
        assert_eq!(
            validate_generated_sql("SELECT 1 AS \"a\"\"b\" FROM spans -- x", ALLOWED),
            Err(SqlGuardError::CommentFound)
        );
        assert_eq!(
            validate_generated_sql("SELECT 1 AS `it's` FROM spans; SELECT 2", ALLOWED),
            Err(SqlGuardError::MultipleStatements)
        );
        assert_accepted("SELECT count(*) AS \"runs; total\" FROM spans");
        // A quoted function name is still a function call.
        for (sql, name) in [
            (
                "SELECT \"read_blob\"('/etc/passwd') FROM spans",
                "\"read_blob\"",
            ),
            (
                "SELECT `read_blob`('/etc/passwd') FROM spans",
                "`read_blob`",
            ),
        ] {
            assert_eq!(
                validate_generated_sql(sql, ALLOWED),
                Err(SqlGuardError::FunctionNotAllowed(name.into())),
                "{sql}"
            );
        }
    }

    #[test]
    fn sql_the_tokenizer_would_read_otherwise_than_duckdb_is_refused() {
        for (sql, part) in [
            // Dollar-quoted and escape strings end elsewhere than `'…'`.
            ("SELECT $$x$$ AS a FROM spans", "$"),
            ("SELECT $t$x$t$ AS a FROM spans", "$"),
            ("SELECT E'x' AS a FROM spans", "E'"),
            ("SELECT (e'x') AS a FROM spans", "e'"),
            // DuckDB reads a number, then the word glued to it.
            ("SELECT a, 1FROM secrets", "1FROM"),
            ("SELECT a, 1.FROM secrets", "1.FROM"),
            ("SELECT a, .5FROM secrets", ".5FROM"),
            ("SELECT a, 1e5FROM secrets", "1e5FROM"),
        ] {
            assert_eq!(
                validate_generated_sql(sql, ALLOWED),
                Err(SqlGuardError::Unreadable(part.into())),
                "{sql}"
            );
        }
        for sql in [
            "SELECT count(*) FROM spans WHERE span_name LIKE'resilience%'",
            "SELECT duration_ns / 1e9, 1_000, 1.5, .5, 2E3 FROM spans",
            "SELECT 'it''s $5' AS a, \"$\" FROM spans",
        ] {
            assert_accepted(sql);
        }
    }

    #[test]
    fn scope_tables_scopes_every_table_of_a_comma_list() {
        let preds: &[(&str, &str)] =
            &[("spans", "target_environment IN ('dev')"), ("logs", "true")];
        assert_eq!(
            scope_tables(
                "SELECT count(*) FROM spans s, logs l WHERE s.trace_id = l.trace_id",
                preds
            )
            .unwrap(),
            "SELECT count(*) FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) s, \
             (SELECT * FROM logs WHERE true) l WHERE s.trace_id = l.trace_id"
        );
        assert_eq!(
            scope_tables("SELECT count(*) FROM spans, logs", preds).unwrap(),
            "SELECT count(*) FROM (SELECT * FROM spans WHERE target_environment IN ('dev')) \
             AS spans, (SELECT * FROM logs WHERE true) AS logs"
        );
        assert_eq!(
            scope_tables("SELECT * FROM (spans s JOIN logs l ON true)", preds).unwrap(),
            "SELECT * FROM ((SELECT * FROM spans WHERE target_environment IN ('dev')) s \
             JOIN (SELECT * FROM logs WHERE true) l ON true)"
        );
        // A second table without a predicate fails closed.
        assert_eq!(
            scope_tables("SELECT count(*) FROM spans, logs", SCOPE),
            Err(SqlGuardError::TableNotScopable("logs".into()))
        );
    }

    #[test]
    fn scope_tables_scopes_the_table_a_cte_of_the_same_name_reads() {
        assert_eq!(
            scope_tables(
                "WITH spans AS (SELECT * FROM spans) SELECT count(*) FROM spans",
                SCOPE
            )
            .unwrap(),
            "WITH spans AS (SELECT * FROM (SELECT * FROM spans WHERE \
             target_environment IN ('dev')) AS spans) SELECT count(*) FROM spans"
        );
    }

    #[test]
    fn scope_tables_refuses_what_the_validator_cannot_read() {
        assert_eq!(
            scope_tables("SELECT * FROM spans s x", SCOPE),
            Err(SqlGuardError::TableNotAllowed("x".into()))
        );
        assert_eq!(
            scope_tables("SELECT * FROM spans, 'spans.csv'", SCOPE),
            Err(SqlGuardError::TableNotScopable("?str?".into()))
        );
    }
}
