//! The bounds every version string is held to, wherever it enters: a
//! declared `target.version`, `--target-version`, a query parameter, or a
//! `service.version` read from telemetry.

use std::borrow::Cow;

/// The longest version, in bytes.
pub const MAX_VERSION_LEN: usize = 128;

/// Why a version string is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VersionTextError {
    /// Longer than [`MAX_VERSION_LEN`] bytes.
    #[error("a version is at most {MAX_VERSION_LEN} bytes; this one is {len}")]
    TooLong { len: usize },
    /// Holds a control character (a newline, a tab, an escape, …).
    #[error("a version holds no control characters")]
    ControlCharacter,
}

/// Whether `text` is within the bounds. What a person or a client sends
/// is refused when it is not.
///
/// # Errors
/// Returns the bound `text` breaks.
pub fn check_version_text(text: &str) -> Result<(), VersionTextError> {
    if text.len() > MAX_VERSION_LEN {
        return Err(VersionTextError::TooLong { len: text.len() });
    }
    if text.chars().any(char::is_control) {
        return Err(VersionTextError::ControlCharacter);
    }
    Ok(())
}

/// `text` brought within the bounds, and whether anything had to go:
/// control characters are dropped and the rest is cut at the last whole
/// character inside [`MAX_VERSION_LEN`]. For what telemetry carries,
/// which cannot be refused.
#[must_use]
pub fn bounded_version_text(text: &str) -> (Cow<'_, str>, bool) {
    if check_version_text(text).is_ok() {
        return (Cow::Borrowed(text), false);
    }
    let mut kept = String::with_capacity(text.len().min(MAX_VERSION_LEN));
    for c in text.chars().filter(|c| !c.is_control()) {
        if kept.len() + c.len_utf8() > MAX_VERSION_LEN {
            break;
        }
        kept.push(c);
    }
    (Cow::Owned(kept), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_text_within_the_bounds_is_accepted_and_beyond_them_refused() {
        for fine in [
            "1.5.0",
            "v2.0.0-rc.1+build.5",
            "main",
            "5114f85",
            "ünïcode",
            "",
        ] {
            assert_eq!(check_version_text(fine), Ok(()), "{fine:?}");
        }
        assert_eq!(check_version_text(&"9".repeat(MAX_VERSION_LEN)), Ok(()));
        assert_eq!(
            check_version_text(&"9".repeat(MAX_VERSION_LEN + 1)),
            Err(VersionTextError::TooLong {
                len: MAX_VERSION_LEN + 1
            })
        );
        for control in [
            "1.0\n2.0",
            "1.0\t",
            "\u{1b}[31m1.0",
            "1.0\0",
            "1.0\u{7f}",
            "1.0\u{85}",
        ] {
            assert_eq!(
                check_version_text(control),
                Err(VersionTextError::ControlCharacter),
                "{control:?}"
            );
        }
    }

    #[test]
    fn text_from_telemetry_is_cut_to_the_bound_and_flagged() {
        let (kept, changed) = bounded_version_text("1.5.0");
        assert_eq!((kept.as_ref(), changed), ("1.5.0", false));
        assert!(matches!(kept, Cow::Borrowed(_)));

        let (kept, changed) = bounded_version_text("1.0\n<b>\u{1b}[0m");
        assert_eq!((kept.as_ref(), changed), ("1.0<b>[0m", true));

        let long = "x".repeat(MAX_VERSION_LEN + 40);
        let (kept, changed) = bounded_version_text(&long);
        assert_eq!((kept.len(), changed), (MAX_VERSION_LEN, true));

        // The cut never splits a character: 2-byte characters from an odd
        // offset leave one byte unused.
        let wide = format!("a{}", "é".repeat(MAX_VERSION_LEN));
        let (kept, changed) = bounded_version_text(&wide);
        assert!(changed);
        assert_eq!(kept.len(), MAX_VERSION_LEN - 1);
        assert!(wide.starts_with(kept.as_ref()));
        // Whatever comes out passes the check.
        for text in [long.as_str(), wide.as_str(), "a\nb", ""] {
            assert_eq!(check_version_text(&bounded_version_text(text).0), Ok(()));
        }
    }
}
