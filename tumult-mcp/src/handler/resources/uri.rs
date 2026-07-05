//! Resource kinds and the `tumult://{kind}/{filename}` URI scheme.

use std::path::Path;

use rust_mcp_sdk::schema::RpcError;

use crate::tools;

/// URI prefix shared by every Tumult resource.
pub(super) const URI_PREFIX: &str = "tumult://";

/// Kind of a workspace resource, deciding its URI scheme tail and MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceKind {
    /// Experiment journal — served as JSON (`tumult://journal/…`).
    Journal,
    /// Experiment definition — served as raw TOON (`tumult://experiment/…`).
    Experiment,
    /// `GameDay` campaign — served as raw TOON (`tumult://gameday/…`).
    Gameday,
}

impl ResourceKind {
    /// URI path segment between `tumult://` and the file name.
    pub(super) fn uri_kind(self) -> &'static str {
        match self {
            Self::Journal => "journal",
            Self::Experiment => "experiment",
            Self::Gameday => "gameday",
        }
    }

    /// MIME type of the content `resources/read` returns for this kind:
    /// journals are rendered as JSON, everything else is raw TOON text.
    pub(super) fn mime_type(self) -> &'static str {
        match self {
            Self::Journal => "application/json",
            Self::Experiment | Self::Gameday => "application/toon",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Journal => "Tumult experiment journal (read as JSON: {summary, journal}).",
            Self::Experiment => "Tumult chaos experiment definition (raw TOON).",
            Self::Gameday => "Tumult GameDay campaign definition (raw TOON).",
        }
    }
}

/// Classify a workspace `.toon` file. `.gameday.toon` wins on the file
/// name; otherwise journals are recognized by their `experiment_title:`
/// field before experiments are recognized by `title:` (a journal may embed
/// nested hypothesis `title:` lines), and unreadable/ambiguous files fall
/// back to journal — mirroring `tumult_list_journals`, which lists every
/// `.toon` file.
pub(crate) fn classify(path: &Path) -> ResourceKind {
    if path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with(".gameday"))
    {
        return ResourceKind::Gameday;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return ResourceKind::Journal;
    };
    if content
        .lines()
        .any(|line| line.trim_start().starts_with("experiment_title:"))
    {
        return ResourceKind::Journal;
    }
    if tools::extract_title(&content).is_some() {
        return ResourceKind::Experiment;
    }
    ResourceKind::Journal
}

/// Parse a `tumult://{kind}/{filename}` URI. The filename tail must be a
/// plain `.toon` file name — separators, traversal components, and unknown
/// kinds/schemes are protocol errors.
// Resource names are exact, lowercase identifiers we mint ourselves; a
// case-sensitive `.toon` suffix match is the intended, correct behaviour.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub(super) fn parse_resource_uri(uri: &str) -> Result<(ResourceKind, &str), RpcError> {
    let invalid =
        |message: String| RpcError::invalid_params().with_message(format!("{message}: {uri}"));
    let rest = uri
        .strip_prefix(URI_PREFIX)
        .ok_or_else(|| invalid("unsupported resource URI scheme".into()))?;
    let (kind, name) = rest
        .split_once('/')
        .ok_or_else(|| invalid("malformed resource URI".into()))?;
    let kind = match kind {
        "journal" => ResourceKind::Journal,
        "experiment" => ResourceKind::Experiment,
        "gameday" => ResourceKind::Gameday,
        other => return Err(invalid(format!("unknown resource kind '{other}'"))),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(invalid(
            "resource name must be a plain file name (no path separators or traversal)".into(),
        ));
    }
    match kind {
        ResourceKind::Gameday if !name.ends_with(".gameday.toon") => Err(invalid(
            "gameday resource names must end with .gameday.toon".into(),
        )),
        ResourceKind::Journal | ResourceKind::Experiment if !name.ends_with(".toon") => {
            Err(invalid("resource names must end with .toon".into()))
        }
        _ => Ok((kind, name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uri_accepts_plain_names_and_rejects_everything_else() {
        assert_eq!(
            parse_resource_uri("tumult://journal/a.toon").unwrap(),
            (ResourceKind::Journal, "a.toon")
        );
        assert_eq!(
            parse_resource_uri("tumult://gameday/x.gameday.toon").unwrap(),
            (ResourceKind::Gameday, "x.gameday.toon")
        );
        for bad in [
            "file:///etc/passwd",
            "tumult://journal",
            "tumult://journal/",
            "tumult://journal/..",
            "tumult://journal/../escape",
            "tumult://journal/sub/a.toon",
            "tumult://journal/a\\b.toon",
            "tumult://journal/passwd",
            "tumult://gameday/plain.toon",
            "tumult://prompt/a.toon",
        ] {
            assert!(parse_resource_uri(bad).is_err(), "{bad} must be rejected");
        }
    }
}
