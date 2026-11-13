//! Choosing the log format: JSON unless a human is watching.

use super::format::LogFormat;

/// Environment variable that forces the log format: `json` or `text`.
pub const FORMAT_ENV: &str = "TUMULT_LOG_FORMAT";

/// The outcome of [`choose_format`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatChoice {
    /// The format to use.
    pub format: LogFormat,
    /// The requested value, when it was not a known format. The caller logs
    /// one warning for it once the subscriber is installed.
    pub rejected: Option<String>,
}

impl FormatChoice {
    fn accepted(format: LogFormat) -> Self {
        Self {
            format,
            rejected: None,
        }
    }
}

/// Picks the log format.
///
/// * `requested` is the value of [`FORMAT_ENV`], if set. `json` and `text`
///   (any case, surrounding whitespace ignored) are honoured; an empty value
///   counts as unset; anything else falls back to JSON and is reported in
///   [`FormatChoice::rejected`].
/// * With no request, the format is text when the writer is a terminal and
///   JSON otherwise, so a process without a TTY (systemd, containers, CI)
///   is structured with no configuration.
#[must_use]
pub fn choose_format(requested: Option<&str>, writer_is_terminal: bool) -> FormatChoice {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    let Some(value) = requested else {
        return FormatChoice::accepted(automatic_format(writer_is_terminal));
    };
    if value.eq_ignore_ascii_case("json") {
        return FormatChoice::accepted(LogFormat::Json);
    }
    if value.eq_ignore_ascii_case("text") {
        return FormatChoice::accepted(LogFormat::Text);
    }
    FormatChoice {
        format: LogFormat::Json,
        rejected: Some(value.to_string()),
    }
}

fn automatic_format(writer_is_terminal: bool) -> LogFormat {
    if writer_is_terminal {
        LogFormat::Text
    } else {
        LogFormat::Json
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_format, FormatChoice};
    use crate::logging::LogFormat;

    fn chosen(requested: Option<&str>, terminal: bool) -> FormatChoice {
        choose_format(requested, terminal)
    }

    #[test]
    fn defaults_to_json_when_the_writer_is_not_a_terminal() {
        let choice = chosen(None, false);
        assert_eq!(choice.format, LogFormat::Json);
        assert_eq!(choice.rejected, None);
    }

    #[test]
    fn defaults_to_text_on_a_terminal() {
        assert_eq!(chosen(None, true).format, LogFormat::Text);
    }

    #[test]
    fn explicit_json_wins_on_a_terminal() {
        assert_eq!(chosen(Some("json"), true).format, LogFormat::Json);
    }

    #[test]
    fn explicit_text_wins_without_a_terminal() {
        let choice = chosen(Some("text"), false);
        assert_eq!(choice.format, LogFormat::Text);
        assert_eq!(choice.rejected, None);
    }

    #[test]
    fn value_is_trimmed_and_case_insensitive() {
        assert_eq!(chosen(Some(" JSON "), true).format, LogFormat::Json);
        assert_eq!(chosen(Some("Text"), false).format, LogFormat::Text);
    }

    #[test]
    fn empty_value_counts_as_unset() {
        assert_eq!(chosen(Some(""), false), chosen(None, false));
        assert_eq!(chosen(Some("  "), true), chosen(None, true));
    }

    #[test]
    fn invalid_value_falls_back_to_json_and_is_reported() {
        for terminal in [false, true] {
            let choice = chosen(Some("yaml"), terminal);
            assert_eq!(choice.format, LogFormat::Json);
            assert_eq!(choice.rejected.as_deref(), Some("yaml"));
        }
    }
}
