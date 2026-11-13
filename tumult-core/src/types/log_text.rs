//! The bound on text that reaches a log from outside the process: a
//! tool's stderr, an error a remote end worded, a name from telemetry.

use std::borrow::Cow;

/// The longest such value in a log event, in bytes.
pub const MAX_LOG_VALUE_LEN: usize = 512;

/// `text` fit for a log field: control characters (a newline that would
/// forge a second record, an escape that would recolour a terminal)
/// become spaces, and the rest is cut at the last whole character
/// inside [`MAX_LOG_VALUE_LEN`].
#[must_use]
pub fn bounded_log_text(text: &str) -> Cow<'_, str> {
    if text.len() <= MAX_LOG_VALUE_LEN && !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut kept = String::with_capacity(text.len().min(MAX_LOG_VALUE_LEN));
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        if kept.len() + c.len_utf8() > MAX_LOG_VALUE_LEN {
            break;
        }
        kept.push(c);
    }
    Cow::Owned(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_logged_as_it_is() {
        assert!(matches!(
            bounded_log_text("connection refused (os error 111)"),
            Cow::Borrowed("connection refused (os error 111)")
        ));
    }

    #[test]
    fn control_characters_in_logged_text_become_spaces() {
        assert_eq!(
            bounded_log_text("down\nlevel=INFO forged\u{1b}[31m"),
            "down level=INFO forged [31m"
        );
    }

    #[test]
    fn logged_text_is_cut_at_a_whole_character_inside_the_bound() {
        let long = "é".repeat(MAX_LOG_VALUE_LEN);
        let kept = bounded_log_text(&long);
        assert_eq!(kept.len(), MAX_LOG_VALUE_LEN);
        assert!(kept.chars().all(|c| c == 'é'));
    }
}
