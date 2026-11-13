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
    use super::{err, error_type, short_type_name};

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
}
