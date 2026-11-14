//! Paging for the entity reads.
//!
//! A page is bounded twice: in rows (`limit`) and in bytes, because one
//! row can carry a description and relationships of 64 KiB each. A page
//! that leaves rows behind says `truncated: true` and gives `next_cursor`,
//! the position of its last row in the order of the listing; sent back as
//! `cursor`, it continues after that row. The cursor is a position, not a
//! snapshot: an entity that reports between two pages moves to the front
//! of the listing and is not seen again by a walk already past it.

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

/// What a row weighs besides its two JSON columns: the key, the id and
/// the scalar columns.
const ROW_OVERHEAD: usize = 512;

/// A cursor longer than this was not given out by this API.
const MAX_CURSOR_LEN: usize = 64 * 1024;

const INVALID_CURSOR: &str = "invalid cursor";

pub(super) fn row_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// `position` as the text a client sends back: its JSON in hexadecimal,
/// which needs no encoding in a URL.
pub(super) fn encode_cursor<T: Serialize>(position: &T) -> Result<String, String> {
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
pub(super) fn decode_cursor<T: DeserializeOwned>(text: Option<&str>) -> Result<Option<T>, String> {
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

/// The bytes `row` adds to a response, near enough. The JSON columns are
/// still text here.
fn weight(row: &Value) -> usize {
    let text = |column: &str| row.get(column).and_then(Value::as_str).map_or(0, str::len);
    ROW_OVERHEAD + text("description") + text("relationships")
}

/// Cut `rows` to `limit` rows and to `budget` bytes, keeping at least one.
/// The statement asks for one row more than `limit`, so a row cut here
/// means the listing goes on.
pub(super) fn cut(mut rows: Vec<Value>, limit: u32, budget: usize) -> Page {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut bytes = 0;
    let mut keep = 0;
    for row in rows.iter().take(limit) {
        if keep > 0 && bytes >= budget {
            break;
        }
        bytes += weight(row);
        keep += 1;
    }
    let truncated = keep < rows.len();
    rows.truncate(keep);
    Page { rows, truncated }
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

    fn rows(n: usize, description_bytes: usize) -> Vec<Value> {
        vec![json!({"description": "x".repeat(description_bytes), "relationships": "[]"}); n]
    }

    #[test]
    fn a_page_holds_the_limit_and_says_when_more_is_left() {
        let page = cut(rows(3, 2), 3, MAX_PAGE_BYTES);
        assert_eq!((page.rows.len(), page.truncated), (3, false));
        let page = cut(rows(4, 2), 3, MAX_PAGE_BYTES);
        assert_eq!((page.rows.len(), page.truncated), (3, true));
        let page = cut(Vec::new(), 3, MAX_PAGE_BYTES);
        assert_eq!((page.rows.len(), page.truncated), (0, false));
    }

    #[test]
    fn a_page_ends_once_it_is_heavy_enough_but_holds_one_row_at_least() {
        let page = cut(rows(10, 1_000), 10, 3_000);
        assert_eq!((page.rows.len(), page.truncated), (2, true));
        let page = cut(rows(2, 1_000), 10, 1);
        assert_eq!((page.rows.len(), page.truncated), (1, true));
        let page = cut(rows(1, 1_000), 10, 0);
        assert_eq!((page.rows.len(), page.truncated), (1, false));
    }
}
