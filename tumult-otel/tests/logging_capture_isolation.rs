//! A capture keeps seeing a callsite that a thread without a subscriber
//! reached first.
//!
//! Separate integration-test binary with a single test: the capture has to
//! own the only subscriber in the process when the other thread logs.

use tracing::Level;
use tumult_otel::logging::testing::LogCapture;

fn emit() {
    tracing::warn!(name: "capture.shared_callsite", "from a shared callsite");
}

#[test]
fn a_thread_without_a_subscriber_does_not_blind_a_capture() {
    let capture = LogCapture::start(Level::INFO);

    std::thread::spawn(emit).join().unwrap();
    emit();

    assert_eq!(
        capture.snapshot().named("capture.shared_callsite").len(),
        1,
        "the event of the capturing thread must be captured"
    );
}
