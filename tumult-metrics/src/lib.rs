// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-metrics` — a YAML semantic metric layer (Rill-style metrics
//! views) compiled to SQL.
//!
//! A [`MetricDef`] names a source table, a measure and dimensions. [`to_sql`]
//! compiles a definition (plus caller-supplied group-bys and a time range)
//! into a single `SELECT`. **Every identifier is strictly validated**
//! (`[a-z0-9_.]` only) before it is interpolated, which makes SQL injection
//! through a metric definition impossible by construction. Literals in
//! equality conditions are escaped when rendered.

use std::path::Path;

use serde::Deserialize;

/// Errors from loading, validating or compiling metric definitions.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),

    #[error(
        "invalid identifier {0:?}: only lowercase [a-z0-9_.] is allowed \
         (this restriction exists to make SQL injection impossible)"
    )]
    InvalidIdentifier(String),

    #[error("invalid literal {0:?} in a condition: strings must not contain NUL")]
    InvalidLiteral(String),
}

/// One side of a [`Measure::Rate`]: a column to aggregate, optionally
/// restricted by equality conditions. The special column `"*"` counts rows.
/// `condition` and `conditions` are ANDed together (`conditions` exists so a
/// rate term can pin more than one column, e.g. a metric name plus a dim).
#[derive(Debug, Clone, Deserialize)]
pub struct Term {
    pub column: String,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// A literal in an equality condition (rendered escaped).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Literal {
    Bool(bool),
    Num(f64),
    Str(String),
}

/// `column = literal`, safely rendered.
#[derive(Debug, Clone, Deserialize)]
pub struct Condition {
    pub column: String,
    pub equals: Literal,
}

/// How a metric aggregates.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Measure {
    /// `COUNT(*)`
    Count,
    /// `SUM(column)`
    Sum { column: String },
    /// `AVG(column)`
    Avg { column: String },
    /// `COUNT(DISTINCT column)` — e.g. coverage of target systems.
    CountDistinct { column: String },
    /// `num / NULLIF(den, 0)` where each side is a [`Term`].
    Rate { num: Term, den: Term },
}

/// A semantic metric definition, one per YAML file.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source_table: String,
    pub measure: Measure,
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Time column used for range filtering (epoch nanoseconds).
    #[serde(default = "default_time_col")]
    pub time_col: String,
    /// Optional metric-level equality filter.
    #[serde(default)]
    pub condition: Option<Condition>,
}

fn default_time_col() -> String {
    "ts_ns".to_string()
}

/// `true` if `s` is a non-empty identifier of `[a-z0-9_.]` characters only.
fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

fn check_ident(s: &str) -> Result<(), MetricsError> {
    if valid_ident(s) {
        Ok(())
    } else {
        Err(MetricsError::InvalidIdentifier(s.to_string()))
    }
}

fn render_literal(literal: &Literal) -> Result<String, MetricsError> {
    Ok(match literal {
        Literal::Bool(b) => b.to_string().to_uppercase(),
        Literal::Num(n) => n.to_string(),
        Literal::Str(s) => {
            if s.contains('\0') {
                return Err(MetricsError::InvalidLiteral(s.clone()));
            }
            format!("'{}'", s.replace('\'', "''"))
        }
    })
}

fn render_condition(cond: &Condition) -> Result<String, MetricsError> {
    check_ident(&cond.column)?;
    Ok(format!(
        "{} = {}",
        cond.column,
        render_literal(&cond.equals)?
    ))
}

fn render_term(term: &Term) -> Result<String, MetricsError> {
    let mut conds = Vec::new();
    if let Some(cond) = &term.condition {
        conds.push(render_condition(cond)?);
    }
    for cond in &term.conditions {
        conds.push(render_condition(cond)?);
    }
    let condition = if conds.is_empty() {
        None
    } else {
        Some(conds.join(" AND "))
    };
    if term.column == "*" {
        return Ok(match condition {
            Some(cond) => format!("COUNT(*) FILTER (WHERE {cond})"),
            None => "COUNT(*)".to_string(),
        });
    }
    check_ident(&term.column)?;
    Ok(match condition {
        Some(cond) => format!(
            "SUM(CASE WHEN {cond} THEN CAST({} AS DOUBLE) ELSE 0 END)",
            term.column
        ),
        None => format!("SUM(CAST({} AS DOUBLE))", term.column),
    })
}

fn render_measure(measure: &Measure) -> Result<String, MetricsError> {
    Ok(match measure {
        Measure::Count => "COUNT(*)".to_string(),
        Measure::Sum { column } => {
            check_ident(column)?;
            format!("SUM({column})")
        }
        Measure::Avg { column } => {
            check_ident(column)?;
            format!("AVG({column})")
        }
        Measure::CountDistinct { column } => {
            check_ident(column)?;
            format!("COUNT(DISTINCT {column})")
        }
        Measure::Rate { num, den } => {
            format!("{} / NULLIF({}, 0)", render_term(num)?, render_term(den)?)
        }
    })
}

/// Validate every identifier in a definition (also called by `load_dir`).
pub fn validate(def: &MetricDef) -> Result<(), MetricsError> {
    check_ident(&def.name)?;
    check_ident(&def.source_table)?;
    check_ident(&def.time_col)?;
    for dim in &def.dimensions {
        check_ident(dim)?;
    }
    render_measure(&def.measure)?; // validates measure internals
    if let Some(cond) = &def.condition {
        render_condition(cond)?;
    }
    Ok(())
}

/// Compile a definition into SQL. `group_by` dimensions are caller-supplied
/// (and equally validated); `time_range` is `[start_ns, end_ns)` on
/// `def.time_col`.
///
/// # Errors
/// Returns [`MetricsError::InvalidIdentifier`] if any identifier (including
/// caller-supplied group-bys) contains characters outside `[a-z0-9_.]`.
pub fn to_sql(
    def: &MetricDef,
    group_by: &[&str],
    time_range: Option<(i64, i64)>,
) -> Result<String, MetricsError> {
    validate(def)?;

    let mut dims: Vec<String> = def.dimensions.clone();
    for dim in group_by {
        check_ident(dim)?;
        dims.push((*dim).to_string());
    }

    let measure_sql = render_measure(&def.measure)?;
    let select_dims = if dims.is_empty() {
        String::new()
    } else {
        format!("{}, ", dims.join(", "))
    };

    let mut wheres = Vec::new();
    if let Some(cond) = &def.condition {
        wheres.push(render_condition(cond)?);
    }
    if let Some((start, end)) = time_range {
        wheres.push(format!("{} >= {start}", def.time_col));
        wheres.push(format!("{} < {end}", def.time_col));
    }
    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!("\nWHERE {}", wheres.join("\n  AND "))
    };

    let group_sql = if dims.is_empty() {
        String::new()
    } else {
        format!(
            "\nGROUP BY {}\nORDER BY {}",
            dims.join(", "),
            dims.join(", ")
        )
    };

    Ok(format!(
        "SELECT {select_dims}{measure_sql} AS value\nFROM {}{where_sql}{group_sql}",
        def.source_table
    ))
}

/// Compile a definition into a **time-bucketed series** SQL: the same measure
/// as [`to_sql`], aggregated per `bucket_ns`-wide bucket of `def.time_col`,
/// plus the caller-supplied `group_by` dimensions. Each row carries
/// `bucket_s` (bucket start, epoch seconds) and `value`.
///
/// The bucket expression is derived inside a subquery so the definition's own
/// `time_col` filter still applies to raw rows; everything interpolated is
/// either validated by [`validate`] or the integer `bucket_ns`.
///
/// # Errors
/// Same as [`to_sql`].
pub fn to_sql_bucketed(
    def: &MetricDef,
    bucket_ns: i64,
    group_by: &[&str],
    time_range: Option<(i64, i64)>,
) -> Result<String, MetricsError> {
    validate(def)?;
    if bucket_ns <= 0 {
        return Err(MetricsError::InvalidIdentifier(format!(
            "bucket_ns must be positive, got {bucket_ns}"
        )));
    }
    let mut dims: Vec<String> = Vec::new();
    for dim in group_by {
        check_ident(dim)?;
        dims.push((*dim).to_string());
    }
    let measure_sql = render_measure(&def.measure)?;
    let select_dims = if dims.is_empty() {
        String::new()
    } else {
        format!(", {}", dims.join(", "))
    };
    let mut wheres = Vec::new();
    if let Some(cond) = &def.condition {
        wheres.push(render_condition(cond)?);
    }
    if let Some((start, end)) = time_range {
        wheres.push(format!("{} >= {start}", def.time_col));
        wheres.push(format!("{} < {end}", def.time_col));
    }
    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!("\nWHERE {}", wheres.join("\n  AND "))
    };
    let group_sql = if dims.is_empty() {
        "\nGROUP BY bucket_s\nORDER BY bucket_s".to_string()
    } else {
        format!(
            "\nGROUP BY bucket_s, {}\nORDER BY bucket_s, {}",
            dims.join(", "),
            dims.join(", ")
        )
    };
    Ok(format!(
        "SELECT ({} // {bucket_ns}) * {bucket_ns} // 1000000000 AS bucket_s{select_dims}, \
         {measure_sql} AS value\nFROM {}{where_sql}{group_sql}",
        def.time_col, def.source_table
    ))
}

/// Load every `*.yaml`/`*.yml` metric definition in a directory (sorted by
/// file name) and validate them.
///
/// # Errors
/// Returns an error if the directory cannot be read, a file fails to parse,
/// or a definition fails validation.
pub fn load_dir(path: &Path) -> Result<Vec<MetricDef>, MetricsError> {
    let mut files: Vec<_> = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
        .collect();
    files.sort();
    let mut defs = Vec::with_capacity(files.len());
    for file in files {
        let def: MetricDef = serde_yaml::from_str(&std::fs::read_to_string(&file)?)?;
        validate(&def)?;
        defs.push(def);
    }
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass_rate_def() -> MetricDef {
        serde_yaml::from_str(
            r#"
name: hypothesis_pass_rate
description: Fraction of experiment runs whose hypothesis held
source_table: spans
time_col: ts_ns
measure:
  type: rate
  num:
    column: hypothesis_met
    condition: { column: span_name, equals: "resilience.experiment" }
  den:
    column: "*"
    condition: { column: span_name, equals: "resilience.experiment" }
dimensions: []
"#,
        )
        .unwrap()
    }

    #[test]
    fn rate_compiles_with_filters() {
        let sql = to_sql(&pass_rate_def(), &[], None).unwrap();
        assert!(sql.contains("COUNT(*) FILTER (WHERE span_name = 'resilience.experiment')"));
        assert!(sql.contains("NULLIF"));
        assert!(sql.contains("FROM spans"));
    }

    #[test]
    fn group_by_and_time_range_compile() {
        let def: MetricDef = serde_yaml::from_str(
            r#"
name: experiment_count
source_table: spans
measure: { type: count }
dimensions: [target_system]
condition: { column: span_name, equals: "resilience.experiment" }
"#,
        )
        .unwrap();
        let sql = to_sql(&def, &["target_environment"], Some((10, 20))).unwrap();
        assert!(sql.contains("SELECT target_system, target_environment, COUNT(*) AS value"));
        assert!(sql.contains("WHERE span_name = 'resilience.experiment'"));
        assert!(sql.contains("ts_ns >= 10"));
        assert!(sql.contains("ts_ns < 20"));
        assert!(sql.contains("GROUP BY target_system, target_environment"));
    }

    #[test]
    fn rate_term_and_lists_conditions() {
        let def: MetricDef = serde_yaml::from_str(
            r#"
name: deviation_rate
source_table: metric_sums
measure:
  type: rate
  num:
    column: "*"
    conditions:
      - { column: metric_name, equals: "tumult.hypothesis.deviations.total" }
  den:
    column: "*"
    condition: { column: metric_name, equals: "tumult.experiments.total" }
    conditions:
      - { column: outcome_status, equals: "success" }
"#,
        )
        .unwrap();
        let sql = to_sql(&def, &[], None).unwrap();
        assert!(sql.contains(
            "COUNT(*) FILTER (WHERE metric_name = 'tumult.hypothesis.deviations.total')"
        ));
        assert!(
            sql.contains("metric_name = 'tumult.experiments.total' AND outcome_status = 'success'")
        );
    }

    #[test]
    fn injection_in_table_name_is_rejected() {
        let def: MetricDef = serde_yaml::from_str(
            r#"
name: evil
source_table: "spans; DROP TABLE spans"
measure: { type: count }
"#,
        )
        .unwrap();
        assert!(matches!(
            to_sql(&def, &[], None),
            Err(MetricsError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn injection_in_group_by_is_rejected() {
        let def = pass_rate_def();
        assert!(matches!(
            to_sql(&def, &["x' OR '1'='1"], None),
            Err(MetricsError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn string_literals_are_escaped() {
        let cond = Condition {
            column: "outcome_status".into(),
            equals: Literal::Str("it's".into()),
        };
        assert_eq!(render_condition(&cond).unwrap(), "outcome_status = 'it''s'");
    }

    #[test]
    fn bucketed_series_compiles() {
        let sql =
            to_sql_bucketed(&pass_rate_def(), 3_600_000_000_000, &[], Some((10, 20))).unwrap();
        assert!(sql
            .contains("SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s"));
        assert!(sql.contains("AS value\nFROM spans"));
        assert!(sql.contains("ts_ns >= 10"));
        assert!(sql.contains("GROUP BY bucket_s\nORDER BY bucket_s"));
    }

    #[test]
    fn bucketed_series_with_dimension() {
        let def: MetricDef = serde_yaml::from_str(
            r#"
name: experiment_count
source_table: spans
measure: { type: count }
condition: { column: span_name, equals: "resilience.experiment" }
"#,
        )
        .unwrap();
        let sql = to_sql_bucketed(&def, 86_400_000_000_000, &["target_system"], None).unwrap();
        assert!(sql.contains("AS bucket_s, target_system, COUNT(*) AS value"));
        assert!(sql.contains("GROUP BY bucket_s, target_system"));
    }

    #[test]
    fn load_dir_reads_yaml_definitions() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mttr.yaml"),
            r#"
name: mttr
source_table: spans
measure: { type: avg, column: recovery_time_s }
condition: { column: span_name, equals: "resilience.experiment" }
"#,
        )
        .unwrap();
        let defs = load_dir(dir.path()).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "mttr");
        assert_eq!(defs[0].time_col, "ts_ns");
    }

    #[test]
    fn sum_avg_and_count_distinct_render_their_aggregates() {
        let def: MetricDef = serde_yaml::from_str(
            r#"
name: action_duration_s
source_table: spans
measure: { type: sum, column: duration_s }
"#,
        )
        .unwrap();
        assert!(to_sql(&def, &[], None)
            .unwrap()
            .contains("SUM(duration_s) AS value"));

        let def: MetricDef = serde_yaml::from_str(
            r#"
name: avg_duration
source_table: spans
measure: { type: avg, column: duration_s }
"#,
        )
        .unwrap();
        assert!(to_sql(&def, &[], None)
            .unwrap()
            .contains("AVG(duration_s) AS value"));

        let def: MetricDef = serde_yaml::from_str(
            r#"
name: experiment_coverage
source_table: spans
measure: { type: count_distinct, column: target_system }
"#,
        )
        .unwrap();
        assert!(to_sql(&def, &[], None)
            .unwrap()
            .contains("COUNT(DISTINCT target_system) AS value"));
    }

    #[test]
    fn boolean_and_numeric_literals_render_unquoted() {
        let bool_cond = Condition {
            column: "hypothesis_met".into(),
            equals: Literal::Bool(true),
        };
        assert_eq!(
            render_condition(&bool_cond).unwrap(),
            "hypothesis_met = TRUE"
        );

        let num_cond = Condition {
            column: "status_code".into(),
            equals: Literal::Num(2.5),
        };
        assert_eq!(render_condition(&num_cond).unwrap(), "status_code = 2.5");
    }

    #[test]
    fn nul_in_string_literal_is_rejected() {
        let cond = Condition {
            column: "outcome_status".into(),
            equals: Literal::Str("ok\0evil".into()),
        };
        assert!(matches!(
            render_condition(&cond),
            Err(MetricsError::InvalidLiteral(_))
        ));
    }

    #[test]
    fn bucketed_series_rejects_non_positive_bucket() {
        let def = pass_rate_def();
        for bucket_ns in [0, -1] {
            let err = to_sql_bucketed(&def, bucket_ns, &[], None).unwrap_err();
            assert!(
                matches!(err, MetricsError::InvalidIdentifier(_)),
                "bucket_ns {bucket_ns}: got: {err:?}"
            );
            assert!(err.to_string().contains("bucket_ns must be positive"));
        }
    }

    #[test]
    fn load_dir_on_missing_directory_is_io_error() {
        let missing = Path::new("/nonexistent/tumult-metrics-no-such-dir");
        assert!(matches!(load_dir(missing), Err(MetricsError::Io(_))));
    }

    #[test]
    fn load_dir_with_malformed_yaml_is_yaml_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("broken.yaml"), "name: [unclosed").unwrap();
        assert!(matches!(load_dir(dir.path()), Err(MetricsError::Yaml(_))));
    }

    #[test]
    fn validate_rejects_empty_and_uppercase_identifiers() {
        let empty_name: MetricDef = serde_yaml::from_str(
            r#"
name: ""
source_table: spans
measure: { type: count }
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&empty_name),
            Err(MetricsError::InvalidIdentifier(_))
        ));

        let uppercase_name: MetricDef = serde_yaml::from_str(
            r#"
name: PassRate
source_table: spans
measure: { type: count }
"#,
        )
        .unwrap();
        let err = validate(&uppercase_name).unwrap_err();
        assert!(matches!(err, MetricsError::InvalidIdentifier(_)));
        assert!(err.to_string().contains("PassRate"));

        let uppercase_dim: MetricDef = serde_yaml::from_str(
            r#"
name: pass_rate
source_table: spans
measure: { type: count }
dimensions: [TargetSystem]
"#,
        )
        .unwrap();
        assert!(matches!(
            validate(&uppercase_dim),
            Err(MetricsError::InvalidIdentifier(_))
        ));
    }
}
