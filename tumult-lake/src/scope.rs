//! The environment scope of a read: the one rule that confines a query to
//! the environments a reader may see. An empty list is every environment,
//! so there is no predicate. Otherwise a row is in scope when its
//! environment is in the list; a row with no environment is in no list,
//! so a scoped reader never sees it.

/// `column IN ('a', 'b')`: `column` holds one of `envs`. `None` when
/// `envs` is empty (every environment). A name is quoted as an SQL string,
/// with each single quote doubled.
#[must_use]
pub fn in_list(column: &str, envs: &[String]) -> Option<String> {
    if envs.is_empty() {
        return None;
    }
    let list = envs
        .iter()
        .map(|env| format!("'{}'", env.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{column} IN ({list})"))
}

/// `" AND <predicate>"`, to append to a WHERE clause, or nothing when there
/// is no predicate (every environment).
#[must_use]
pub fn and(predicate: Option<String>) -> String {
    predicate.map_or_else(String::new, |p| format!(" AND {p}"))
}

/// The scope of the rows of `table`, one of `spans` and the `metric_*`
/// tables. `spans` binds its own `target_environment` (it lives on the
/// root span); a metric point has no environment, so it reaches the one of
/// the root spans of its experiment through `experiment_name`. `None` when
/// `envs` is empty.
#[must_use]
pub fn table_predicate(table: &str, envs: &[String]) -> Option<String> {
    if table == "spans" {
        return in_list("target_environment", envs);
    }
    in_list("se.target_environment", envs).map(|env| {
        format!(
            "EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = {table}.experiment_name AND {env})"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn an_empty_list_is_every_environment_and_no_predicate() {
        assert_eq!(in_list("target_environment", &[]), None);
        assert_eq!(table_predicate("spans", &[]), None);
        assert_eq!(table_predicate("metric_sums", &[]), None);
    }

    #[test]
    fn a_list_keeps_the_column_inside_its_names() {
        assert_eq!(
            in_list("s.target_environment", &envs(&["prod"])).as_deref(),
            Some("s.target_environment IN ('prod')")
        );
        assert_eq!(
            in_list("target_environment", &envs(&["dev", "staging"])).as_deref(),
            Some("target_environment IN ('dev', 'staging')")
        );
    }

    #[test]
    fn a_single_quote_in_a_name_is_doubled() {
        assert_eq!(
            in_list("env", &envs(&["it's", "dev"])).as_deref(),
            Some("env IN ('it''s', 'dev')")
        );
    }

    #[test]
    fn spans_bind_their_own_environment() {
        assert_eq!(
            table_predicate("spans", &envs(&["dev"])).as_deref(),
            Some("target_environment IN ('dev')")
        );
    }

    #[test]
    fn a_metric_table_reaches_the_environment_through_the_root_spans() {
        assert_eq!(
            table_predicate("metric_sums", &envs(&["dev", "staging"])).as_deref(),
            Some(
                "EXISTS (SELECT 1 FROM spans se \
                 WHERE se.experiment_name = metric_sums.experiment_name \
                 AND se.target_environment IN ('dev', 'staging'))"
            )
        );
    }

    #[test]
    fn the_suffix_joins_a_predicate_and_is_empty_without_one() {
        assert_eq!(and(Some("x = 1".to_string())), " AND x = 1");
        assert_eq!(and(None), "");
        assert_eq!(and(in_list("env", &envs(&["dev"]))), " AND env IN ('dev')");
        assert_eq!(and(in_list("env", &[])), "");
    }
}
