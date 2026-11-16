//! `logging::init` reports a subscriber installed by someone else.
//!
//! Separate integration-test binary: the global subscriber is per process.

use tumult_otel::logging::{self, InitError, LogOptions};

#[test]
fn init_fails_when_another_subscriber_is_already_installed() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry())
        .expect("install the foreign subscriber");

    let result = logging::init(LogOptions::new("foreign-test"));

    assert!(
        matches!(result, Err(InitError::Install(_))),
        "init must surface the install failure: {result:?}"
    );
    assert!(!logging::is_initialized());
}
