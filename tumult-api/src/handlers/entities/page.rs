//! Paging for the entity reads and the DR tests.
//!
//! A page is bounded twice: in rows (`limit`) and in bytes, because one
//! row can carry 64 KiB of description and relationships. A page that
//! leaves rows behind says `truncated: true` and gives `next_cursor`, the
//! position of its last row in the order of the listing; sent back as
//! `cursor`, it continues after that row. The cursor is a position, not a
//! snapshot. In an order that nothing moves a row in (the entity list by
//! key, the history) a walk meets every row that exists throughout it
//! exactly once; rows that appear or are removed meanwhile may or may not
//! be met. In the order by receipt an entity that reports between two
//! pages moves to the front and is not met again by a walk already past
//! it.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// Rows returned when the request names no `limit`.
pub(super) const DEFAULT_LIMIT: u32 = 100;
/// The most rows one request returns.
pub(super) const MAX_LIMIT: u32 = 1000;
/// Bytes of rows after which a page ends, whatever its `limit`. The last
/// row may take the page past it, by one row at most.
pub(super) const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// What a row is taken to weigh besides its two JSON columns: the key,
/// the id and the scalar columns.
const ROW_OVERHEAD: usize = 512;

/// A cursor longer than this was not given out by this API.
const MAX_CURSOR_LEN: usize = 64 * 1024;

const INVALID_CURSOR: &str = "invalid cursor";

pub(crate) fn row_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// `position` as the text a client sends back: its JSON in hexadecimal,
/// which needs no encoding in a URL.
pub(crate) fn encode_cursor<T: Serialize>(position: &T) -> Result<String, String> {
    use std::fmt::Write as _;
    let json = serde_json::to_vec(position).map_err(|e| e.to_string())?;
    let mut text = String::with_capacity(json.len() * 2);
    for byte in json {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

/// The position a `cursor` parameter names; none when it is absent or
/// empty.
pub(crate) fn decode_cursor<T: DeserializeOwned>(text: Option<&str>) -> Result<Option<T>, String> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.len() > MAX_CURSOR_LEN {
        return Err(INVALID_CURSOR.into());
    }
    let json = hex_bytes(text).ok_or(INVALID_CURSOR)?;
    let position = serde_json::from_slice(&json).map_err(|_| INVALID_CURSOR)?;
    Ok(Some(position))
}

/// The rows of one response.
pub(super) struct Page {
    pub(super) rows: Vec<Value>,
    /// Whether rows that match were left out.
    pub(super) truncated: bool,
}

/// The statement of one page: the rows of `matched` in `order`, at most
/// `limit` of them and no more than reach `budget` bytes, one row at
/// least. The store weighs the rows (the bytes of the two JSON columns
/// and [`ROW_OVERHEAD`]) and hands over the page only, so a request
/// never holds `limit` rows of 64 KiB in memory to return a few of them.
/// Every row says in `page_candidates` how many rows were weighed, one
/// more than `limit` at most: [`paged`] learns from it whether the
/// listing goes on.
pub(super) fn page_sql(matched: &str, order: &str, limit: u32, budget: usize) -> String {
    let limit = u64::from(limit);
    format!(
        "WITH candidates AS ({matched} ORDER BY {order} LIMIT {}), \
         weighed AS (\
             SELECT *, count(*) OVER () AS page_candidates, \
                 sum({ROW_OVERHEAD} + strlen(description) + strlen(relationships)) OVER (\
                     ORDER BY {order} \
                     ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS page_bytes_before \
             FROM candidates) \
         SELECT * FROM weighed \
         WHERE coalesce(page_bytes_before, 0) < {budget} \
         ORDER BY {order} LIMIT {limit}",
        limit + 1
    )
}

/// The page the rows of a [`page_sql`] statement are, without the two
/// columns that statement added.
pub(super) fn paged(mut rows: Vec<Value>) -> Page {
    let candidates = rows
        .first()
        .and_then(|row| row["page_candidates"].as_u64())
        .unwrap_or(0);
    let returned = u64::try_from(rows.len()).unwrap_or(u64::MAX);
    for row in &mut rows {
        let Some(columns) = row.as_object_mut() else {
            continue;
        };
        columns.remove("page_candidates");
        columns.remove("page_bytes_before");
    }
    Page {
        rows,
        truncated: candidates > returned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_limit_is_a_hundred_by_default_and_at_most_a_thousand() {
        assert_eq!(row_limit(None), 100);
        assert_eq!(row_limit(Some(0)), 1);
        assert_eq!(row_limit(Some(7)), 7);
        assert_eq!(row_limit(Some(u32::MAX)), 1000);
    }

    #[test]
    fn a_cursor_reads_back_as_the_position_it_was_made_from() {
        let position = (
            i64::MIN,
            "host|host.id=a\"b\\c ?#".to_owned(),
            String::new(),
        );
        let text = encode_cursor(&position).unwrap();
        assert!(text.bytes().all(|b| b.is_ascii_hexdigit()), "{text}");
        let back: Option<(i64, String, String)> = decode_cursor(Some(&text)).unwrap();
        assert_eq!(back, Some(position));
    }

    #[test]
    fn an_absent_or_empty_cursor_is_none() {
        assert_eq!(decode_cursor::<(i64, String)>(None), Ok(None));
        assert_eq!(decode_cursor::<(i64, String)>(Some("")), Ok(None));
    }

    #[test]
    fn a_cursor_this_api_did_not_give_is_refused() {
        let other_shape = encode_cursor(&(1, 2)).unwrap();
        let too_long = "00".repeat(MAX_CURSOR_LEN);
        for broken in ["zz", "5", "5b", "é0", "+1", &other_shape, &too_long] {
            assert_eq!(
                decode_cursor::<(i64, String)>(Some(broken)),
                Err("invalid cursor".to_owned()),
                "{}",
                &broken[..broken.len().min(16)]
            );
        }
    }

    fn rows(returned: usize, candidates: usize) -> Vec<Value> {
        vec![
            json!({"entity_key": "k", "page_candidates": candidates, "page_bytes_before": 7});
            returned
        ]
    }

    #[test]
    fn a_page_says_when_more_is_left_and_drops_the_columns_of_the_statement() {
        let page = paged(rows(3, 3));
        assert_eq!((page.rows.len(), page.truncated), (3, false));
        assert_eq!(page.rows[0], json!({"entity_key": "k"}));
        let page = paged(rows(3, 4));
        assert_eq!((page.rows.len(), page.truncated), (3, true));
        assert_eq!(page.rows[2], json!({"entity_key": "k"}));
        let page = paged(Vec::new());
        assert_eq!((page.rows.len(), page.truncated), (0, false));
    }

    #[test]
    fn the_statement_asks_for_one_row_more_than_the_limit() {
        let sql = page_sql("SELECT 1", "a, b DESC", 7, 99);
        assert!(
            sql.contains("(SELECT 1 ORDER BY a, b DESC LIMIT 8)"),
            "{sql}"
        );
        assert!(sql.ends_with("< 99 ORDER BY a, b DESC LIMIT 7"), "{sql}");
    }
}
