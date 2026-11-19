//! A script gets the trace context of the step that runs it.
//!
//! A process installs a tracer provider and no propagator (see
//! `TumultTelemetry::new`). The unit tests of this crate install the W3C
//! propagator globally (`tumult_test_utils::capture_spans_async`); this test
//! binary installs no global at all, so a script sees here what it sees in
//! production.
#![cfg(unix)]

use std::collections::HashMap;

use opentelemetry::trace::{Span, TraceContextExt, Tracer, TracerProvider};
use opentelemetry::Context;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tumult_plugin::executor::execute_script;

/// Run a script that prints its `TRACEPARENT` while `context` is current,
/// and return what it printed.
async fn traceparent_seen_in(context: Context) -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let script = dir.path().join("traceparent.sh");
    tumult_test_utils::write_executable(&script, "#!/bin/sh\nprintf '%s' \"${TRACEPARENT-}\"\n");
    let _attached = context.attach();
    execute_script(&script, dir.path(), &HashMap::<String, String>::new(), None)
        .await
        .unwrap()
        .stdout
}

#[tokio::test]
async fn a_script_continues_the_trace_of_the_span_that_runs_it() {
    let provider = SdkTracerProvider::builder().build();
    let span = provider.tracer("trace-context-test").start("run");
    let trace_id = span.span_context().trace_id().to_string();
    let traceparent = traceparent_seen_in(Context::current_with_span(span)).await;
    assert_eq!(
        traceparent.split('-').nth(1),
        Some(trace_id.as_str()),
        "TRACEPARENT={traceparent:?}"
    );
}

#[tokio::test]
async fn a_script_gets_no_trace_context_when_no_span_runs_it() {
    assert_eq!(traceparent_seen_in(Context::new()).await, "");
}
