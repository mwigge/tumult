//! `logging::init` installs the process-global subscriber exactly once.
//!
//! This lives in its own integration-test binary because the global
//! subscriber can only be set once per process.

use tumult_otel::logging::{self, InitError, LogFormat, LogOptions};

#[test]
fn second_init_returns_an_error_instead_of_silently_succeeding() {
    assert!(!logging::is_initialized());

    let first = logging::init(LogOptions::new("init-test").with_format(LogFormat::Json));
    assert!(first.is_ok(), "first init must succeed: {first:?}");
    assert!(logging::is_initialized());

    let second = logging::init(LogOptions::new("init-test"));
    assert!(
        matches!(second, Err(InitError::AlreadyInitialized)),
        "second init must fail: {second:?}"
    );

    // The subscriber installed by the first call is still the active one.
    assert!(tracing::dispatcher::has_been_set());
    assert!(logging::is_initialized());
}
