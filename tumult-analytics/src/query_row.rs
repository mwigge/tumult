//! Typed newtype for a single row returned by an analytics query.
//!
//! [`QueryRow`] wraps a `Vec<String>` and provides `Deref<Target = [String]>`
//! so all existing slice operations (`row[n]`, `row.join()`, `row.iter()`, …)
//! continue to work without modification at call sites.

use std::ops::Deref;

/// A single row of string values returned by an analytics SQL query.
///
/// Wraps `Vec<String>` with a newtype to make query return types self-documenting
/// and to prevent accidental mixing with other `Vec<String>` values.
///
/// # Examples
///
/// ```
/// use tumult_analytics::QueryRow;
///
/// let row = QueryRow::from(vec!["hello".to_string(), "world".to_string()]);
/// assert_eq!(row[0], "hello");
/// assert_eq!(row.join(", "), "hello, world");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRow(Vec<String>);

impl QueryRow {
    /// Creates a new `QueryRow` from a vector of string values.
    #[must_use]
    pub fn new(values: Vec<String>) -> Self {
        Self(values)
    }

    /// Returns the underlying string slice.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Returns the number of columns in this row.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if this row has no columns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Deref for QueryRow {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<String>> for QueryRow {
    fn from(values: Vec<String>) -> Self {
        Self(values)
    }
}

impl From<QueryRow> for Vec<String> {
    fn from(row: QueryRow) -> Self {
        row.0
    }
}

impl IntoIterator for QueryRow {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a QueryRow {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_row_index_access() {
        let row = QueryRow::from(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(row[0], "a");
        assert_eq!(row[1], "b");
        assert_eq!(row[2], "c");
    }

    #[test]
    fn query_row_join() {
        let row = QueryRow::from(vec!["x".to_string(), "y".to_string()]);
        assert_eq!(row.join("\t"), "x\ty");
    }

    #[test]
    fn query_row_iter() {
        let row = QueryRow::from(vec!["p".to_string(), "q".to_string()]);
        let collected: Vec<&String> = row.iter().collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0], "p");
    }

    #[test]
    fn query_row_len_and_empty() {
        let empty = QueryRow::from(vec![]);
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let non_empty = QueryRow::from(vec!["val".to_string()]);
        assert!(!non_empty.is_empty());
        assert_eq!(non_empty.len(), 1);
    }

    #[test]
    fn query_row_roundtrip_vec() {
        let original = vec!["one".to_string(), "two".to_string()];
        let row = QueryRow::from(original.clone());
        let back: Vec<String> = row.into();
        assert_eq!(back, original);
    }

    #[test]
    fn query_row_first() {
        let row = QueryRow::from(vec!["first".to_string(), "second".to_string()]);
        assert_eq!(row.first(), Some(&"first".to_string()));
    }
}
