//! Sanitisation for operator- and journal-controlled text before it is
//! interpolated into LLM prompts.
//!
//! Prompt injection does not need exotic payloads: invisible Unicode (bidi
//! overrides that reorder what a reviewer sees, zero-width characters that
//! hide instructions) and control bytes are enough to smuggle directives
//! past a human skimming the rendered prompt. These helpers strip that
//! class of characters and cap each field's length so one field cannot
//! flood the prompt budget.
//!
//! Deliberately manual (no `unicode-normalization`): the threat model here
//! is invisible/format characters, not confusable glyphs.

/// Maximum length of the operator `goal` field.
pub const GOAL_MAX_CHARS: usize = 2000;
/// Maximum length of the journal-derived heuristic context.
pub const JOURNAL_CONTEXT_MAX_CHARS: usize = 8000;
/// Appended when a field is truncated so the model (and a reader) can tell.
const TRUNCATION_MARKER: &str = "… [truncated]";

/// True for characters stripped from prompt-bound text: bidi overrides and
/// isolates, zero-width characters, and control characters other than the
/// whitespace a prompt legitimately uses (`\n`, `\t`).
fn is_stripped(c: char) -> bool {
    matches!(c,
        // Bidi overrides and embeddings (reorder displayed text).
        '\u{202A}'..='\u{202E}'
        // Bidi isolates.
        | '\u{2066}'..='\u{2069}'
        // Zero-width space, non-joiner, joiner.
        | '\u{200B}'..='\u{200D}'
        // BOM / zero-width no-break space.
        | '\u{FEFF}')
        || (c.is_control() && c != '\n' && c != '\t')
}

/// Strip invisible/control characters and cap `input` at `max_chars`
/// (counting Unicode scalar values), appending a marker on truncation.
fn sanitise(input: &str, max_chars: usize) -> String {
    let stripped: String = input.chars().filter(|c| !is_stripped(*c)).collect();
    if stripped.chars().count() <= max_chars {
        stripped
    } else {
        let kept: String = stripped.chars().take(max_chars).collect();
        format!("{kept}{TRUNCATION_MARKER}")
    }
}

/// Sanitise the operator goal before prompt interpolation.
#[must_use]
pub fn goal(input: &str) -> String {
    sanitise(input, GOAL_MAX_CHARS)
}

/// Sanitise journal-derived heuristic context before prompt interpolation.
#[must_use]
pub fn journal_context(input: &str) -> String {
    sanitise(input, JOURNAL_CONTEXT_MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bidi_characters() {
        // \u{202E} (right-to-left override) and \u{2066} (LTR isolate).
        assert_eq!(goal("pay\u{202E}load \u{2066}hidden"), "payload hidden");
        assert_eq!(goal("\u{202A}\u{202B}\u{202C}\u{202D}\u{202E}x"), "x");
        assert_eq!(goal("\u{2067}\u{2068}\u{2069}x"), "x");
    }

    #[test]
    fn strips_zero_width_characters() {
        assert_eq!(goal("a\u{200B}b\u{200C}c\u{200D}d\u{FEFF}"), "abcd");
    }

    #[test]
    fn strips_control_characters_but_keeps_newline_and_tab() {
        assert_eq!(goal("a\0b\u{0007}c\u{001B}d"), "abcd");
        assert_eq!(
            goal("line one\nline two\tindented"),
            "line one\nline two\tindented"
        );
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let clean = "harden the cache tier — coverage: 3/64 (5%)";
        assert_eq!(goal(clean), clean);
    }

    #[test]
    fn truncates_goal_with_marker() {
        let long = "x".repeat(GOAL_MAX_CHARS + 500);
        let out = goal(&long);
        assert_eq!(
            out.chars().count(),
            GOAL_MAX_CHARS + TRUNCATION_MARKER.chars().count()
        );
        assert!(out.ends_with(TRUNCATION_MARKER));
        // Exactly at the cap: no truncation.
        let exact = "y".repeat(GOAL_MAX_CHARS);
        assert_eq!(goal(&exact), exact);
    }

    #[test]
    fn truncates_journal_context_with_marker() {
        let long = "z".repeat(JOURNAL_CONTEXT_MAX_CHARS + 1);
        let out = journal_context(&long);
        assert!(out.ends_with(TRUNCATION_MARKER));
        assert_eq!(
            out.chars().count(),
            JOURNAL_CONTEXT_MAX_CHARS + TRUNCATION_MARKER.chars().count()
        );
    }

    #[test]
    fn truncation_counts_chars_not_bytes() {
        // Multi-byte characters count once each.
        let long = "é".repeat(GOAL_MAX_CHARS + 1);
        let out = goal(&long);
        assert_eq!(
            out.chars().count(),
            GOAL_MAX_CHARS + TRUNCATION_MARKER.chars().count()
        );
    }
}
