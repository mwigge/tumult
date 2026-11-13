//! Helpers for the `error.message` and `error.type` fields.

use std::fmt;

/// Formats an error for the `error.message` field.
///
/// Uses the alternate form (`{:#}`), which for `anyhow::Error` renders the
/// whole cause chain on one line (`outer: inner: root`) and for other errors
/// is the same as `Display`.
///
/// ```
/// # use tumult_otel::logging::err;
/// let error = std::io::Error::other("disk full");
/// tracing::warn!(name: "lake.write.failed", { error.message = %err(&error) }, "write failed");
/// ```
#[must_use]
pub fn err<E: fmt::Display + ?Sized>(error: &E) -> ErrorChain<'_, E> {
    ErrorChain(error)
}

/// The value returned by [`err`]; its `Display` is the error's `{:#}` form.
pub struct ErrorChain<'a, E: ?Sized>(&'a E);

impl<E: fmt::Display + ?Sized> fmt::Display for ErrorChain<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

/// Formats a `std::error::Error` for the `error.message` field, following
/// its `source()` chain.
///
/// Use this instead of [`err`] for errors whose `Display` names only the
/// outermost failure (`tonic::transport::Error` prints `transport error` and
/// keeps the reason in its source). A cause whose text the message already
/// contains is not repeated.
#[must_use]
pub fn err_sources<E: std::error::Error + ?Sized>(error: &E) -> SourceChain<'_, E> {
    SourceChain(error)
}

/// The value returned by [`err_sources`].
pub struct SourceChain<'a, E: ?Sized>(&'a E);

impl<E: std::error::Error + ?Sized> fmt::Display for SourceChain<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rendered = self.0.to_string();
        let mut source = self.0.source();
        while let Some(cause) = source {
            let cause_text = cause.to_string();
            if !rendered.contains(&cause_text) {
                rendered.push_str(": ");
                rendered.push_str(&cause_text);
            }
            source = cause.source();
        }
        f.write_str(&rendered)
    }
}

/// The value for the `error.type` field: the error's type name without its
/// module path and generic arguments (`ExporterBuildError`, `Error`).
///
/// Only useful for concrete error types. For a type-erased error
/// (`anyhow::Error`, `Box<dyn Error>`) write a literal class instead, such
/// as `"io"` or `"timeout"`.
#[must_use]
pub fn error_type<E: ?Sized>(error: &E) -> &'static str {
    short_type_name(std::any::type_name_of_val(error))
}

/// `a::b::Name<c::Arg>` → `Name`.
fn short_type_name(full: &'static str) -> &'static str {
    let without_generics = full.split('<').next().unwrap_or(full);
    without_generics
        .rsplit("::")
        .next()
        .unwrap_or(without_generics)
}

#[cfg(test)]
mod tests {
    use super::{err, err_sources, error_type, short_type_name};

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] std::io::Error);

    /// Mimics `anyhow`: the alternate form appends the cause chain.
    struct Chained;

    impl std::fmt::Display for Chained {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if f.alternate() {
                return f.write_str("outer: inner: root");
            }
            f.write_str("outer")
        }
    }

    #[test]
    fn err_uses_the_alternate_form_so_cause_chains_are_kept() {
        assert_eq!(err(&Chained).to_string(), "outer: inner: root");
    }

    #[test]
    fn err_is_plain_display_for_errors_without_an_alternate_form() {
        let error = Outer(std::io::Error::other("disk full"));
        assert_eq!(err(&error).to_string(), "outer");
        assert_eq!(err("already a string").to_string(), "already a string");
    }

    #[test]
    fn error_type_is_the_bare_type_name() {
        let error = Outer(std::io::Error::other("disk full"));
        assert_eq!(error_type(&error), "Outer");
        assert_eq!(error_type(&std::io::Error::other("x")), "Error");
    }

    #[test]
    fn short_type_name_drops_paths_and_generics() {
        assert_eq!(short_type_name("a::b::Name"), "Name");
        assert_eq!(short_type_name("a::Wrapper<c::d::Arg>"), "Wrapper");
        assert_eq!(short_type_name("Plain"), "Plain");
    }

    /// A wrapper whose own text already repeats its cause.
    #[derive(Debug, thiserror::Error)]
    #[error("read config: {0}")]
    struct Repeating(#[source] std::io::Error);

    #[derive(Debug, thiserror::Error)]
    #[error("start server")]
    struct Nested(#[source] Outer);

    #[test]
    fn err_sources_appends_every_cause_the_text_omits() {
        let error = Nested(Outer(std::io::Error::other("address in use")));
        assert_eq!(
            err_sources(&error).to_string(),
            "start server: outer: address in use"
        );
    }

    #[test]
    fn err_sources_does_not_repeat_a_cause_already_in_the_text() {
        let error = Repeating(std::io::Error::other("disk full"));
        assert_eq!(err_sources(&error).to_string(), "read config: disk full");
    }

    #[test]
    fn err_sources_is_plain_display_without_a_cause() {
        let error = std::io::Error::other("disk full");
        assert_eq!(err_sources(&error).to_string(), "disk full");
    }
}
