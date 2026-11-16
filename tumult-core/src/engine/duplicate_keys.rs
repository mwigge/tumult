//! A key written twice in one object.
//!
//! The TOON decoder keeps the last of two equal keys and says nothing, so a
//! block left behind by an edit silently replaces the one above it.
//! [`first_duplicate`] reads the text of a document that decodes and names
//! the first such key. It only counts the lines it is sure are `key:` lines:
//! the rows of a table and plain list values are never keys.

use std::collections::HashMap;

/// Spaces per nesting level in TOON.
const INDENT: usize = 2;

/// A key that one object holds twice. Lines count from 1.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DuplicateKey {
    pub(crate) key: String,
    pub(crate) first_line: usize,
    pub(crate) line: usize,
}

/// The keys of one object, with the line each was first written on.
struct Scope {
    indent: usize,
    keys: HashMap<String, usize>,
}

/// A `key:` line: the key, and whether it opens a table (`key[N]{a,b}:`).
struct KeyLine<'a> {
    key: &'a str,
    opens_table: bool,
}

/// The first key that an object of `toon` holds twice. Pure.
pub(crate) fn first_duplicate(toon: &str) -> Option<DuplicateKey> {
    let mut scopes: Vec<Scope> = Vec::new();
    let mut table_indent: Option<usize> = None;
    for (index, raw) in toon.lines().enumerate() {
        let content = raw.trim_start_matches(' ');
        if content.trim().is_empty() {
            continue;
        }
        let indent = raw.len() - content.len();
        if table_indent.is_some_and(|table| indent > table) {
            continue;
        }
        table_indent = None;
        let (indent, content) = enter_line(&mut scopes, indent, content);
        let Some(key_line) = key_line(content) else {
            continue;
        };
        if key_line.opens_table {
            table_indent = Some(indent);
        }
        if let Some(first_line) = record(&mut scopes, indent, key_line.key, index + 1) {
            return Some(DuplicateKey {
                key: key_line.key.to_string(),
                first_line,
                line: index + 1,
            });
        }
    }
    None
}

/// Close the objects that end before this line. A list item also closes the
/// item before it, and its first key sits one level deeper than its hyphen.
/// Returns the indent and the text of the line as a plain `key:` line.
fn enter_line<'a>(scopes: &mut Vec<Scope>, indent: usize, content: &'a str) -> (usize, &'a str) {
    let Some(item) = list_item(content) else {
        scopes.retain(|scope| scope.indent <= indent);
        return (indent, content);
    };
    scopes.retain(|scope| scope.indent < indent);
    (indent + INDENT, item)
}

/// The text after the hyphen of a list item line.
fn list_item(content: &str) -> Option<&str> {
    if content == "-" {
        return Some("");
    }
    content.strip_prefix("- ")
}

/// Note `key` in the object at `indent`. Returns the line that already wrote
/// it there.
fn record(scopes: &mut Vec<Scope>, indent: usize, key: &str, line: usize) -> Option<usize> {
    let open = scopes.last_mut().filter(|scope| scope.indent == indent);
    let Some(scope) = open else {
        scopes.push(Scope {
            indent,
            keys: HashMap::from([(key.to_string(), line)]),
        });
        return None;
    };
    if let Some(&first_line) = scope.keys.get(key) {
        return Some(first_line);
    }
    scope.keys.insert(key.to_string(), line);
    None
}

/// Read `content` as `key: value`, `key:`, `key[N]: ...` or `key[N]{a,b}:`.
/// Anything else, a list value or a table row among them, is `None`.
fn key_line(content: &str) -> Option<KeyLine<'_>> {
    let (key, rest) = split_key(content)?;
    let (rest, has_length) = skip_group(rest, '[', ']');
    let (rest, has_fields) = skip_group(rest, '{', '}');
    let value = rest.strip_prefix(':')?;
    if !value.is_empty() && !value.starts_with(' ') {
        return None;
    }
    Some(KeyLine {
        key,
        opens_table: has_length && has_fields,
    })
}

/// The key at the start of `content` and what follows it. A quoted key is
/// returned without its quotes, so both spellings compare equal.
fn split_key(content: &str) -> Option<(&str, &str)> {
    if let Some(quoted) = content.strip_prefix('"') {
        let end = closing_quote(quoted)?;
        return Some((&quoted[..end], &quoted[end + 1..]));
    }
    let end = content
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .unwrap_or(content.len());
    (end > 0).then(|| content.split_at(end))
}

/// The byte offset of the quote that closes a string, given the text after
/// its opening quote. A backslash escapes the character after it.
fn closing_quote(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (offset, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(offset);
        }
    }
    None
}

/// Skip one `open ... close` group at the start of `text`, when there is one.
fn skip_group(text: &str, open: char, close: char) -> (&str, bool) {
    let skipped = text
        .strip_prefix(open)
        .and_then(|inner| inner.split_once(close));
    match skipped {
        Some((_, rest)) => (rest, true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duplicate(toon: &str) -> Option<(String, usize, usize)> {
        first_duplicate(toon).map(|d| (d.key, d.first_line, d.line))
    }

    #[test]
    fn the_first_of_several_duplicates_is_named_with_its_first_line() {
        let found = duplicate("a: 1\nb: 2\na: 3\na: 4\nb: 5\n");

        assert_eq!(found, Some(("a".into(), 1, 3)));
    }

    #[test]
    fn a_key_line_is_a_key_then_a_colon_then_a_space_or_the_end() {
        for (line, key) in [
            ("name: x", "name"),
            ("name:", "name"),
            ("tags[2]: a, b", "tags"),
            ("rows[2]{a,b}:", "rows"),
            ("\"my key\": 1", "my key"),
            ("\"a\\\"b\": 1", "a\\\"b"),
            ("with-dash.and_dot: 1", "with-dash.and_dot"),
        ] {
            assert_eq!(key_line(line).map(|k| k.key), Some(key), "{line}");
        }
        for line in [
            "plain value",
            "http://127.0.0.1:8080/health",
            "\"quoted: value\"",
            "a,b,\"c: d\"",
            "[2]: a, b",
            ": no key",
            "",
        ] {
            assert!(key_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn only_a_length_with_fields_opens_a_table() {
        assert!(key_line("rows[2]{a,b}:").unwrap().opens_table);
        assert!(!key_line("tags[2]: a, b").unwrap().opens_table);
        assert!(!key_line("name: x").unwrap().opens_table);
    }

    #[test]
    fn a_table_that_is_the_first_field_of_a_list_item_hides_its_rows() {
        let toon = "items[2]:\n  - rows[2]{k,v}:\n      a,1\n      a,1\n    name: x\n  - rows[1]{k,v}:\n      a,1\n    name: y\n";

        assert_eq!(duplicate(toon), None);
    }

    #[test]
    fn a_line_that_is_not_a_key_still_closes_the_objects_below_it() {
        // `odd key!` is not read as a key, but its children must not be
        // counted into the object of `first`.
        let toon = "first:\n  x: 1\nodd key!:\n  x: 2\n";

        assert_eq!(duplicate(toon), None);
    }

    #[test]
    fn blank_lines_do_not_end_an_object() {
        assert_eq!(
            duplicate("a:\n  x: 1\n\n  x: 2\n"),
            Some(("x".into(), 2, 4))
        );
    }
}
