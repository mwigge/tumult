//! `OTel` instrumentation for SSH operations.

use opentelemetry::trace::TraceContextExt;
use opentelemetry::KeyValue;
use tumult_otel::{client_span, SpanGuard};

const TRACER: &str = "tumult-ssh";

/// Thin wrapper over [`tumult_otel::client_span`] that fixes the tracer name.
fn ssh_span(name: &'static str, attrs: Vec<KeyValue>) -> SpanGuard {
    client_span(TRACER, name, attrs)
}

pub(crate) fn begin_connect(host: &str, port: u16, auth_method: &str) -> SpanGuard {
    ssh_span(
        "ssh.connect",
        vec![
            // Legacy tumult-specific attributes kept for backwards compatibility.
            KeyValue::new("ssh.host", host.to_string()),
            KeyValue::new("ssh.port", i64::from(port)),
            KeyValue::new("ssh.auth_method", auth_method.to_string()),
            // OTel semantic conventions: network peer attributes.
            KeyValue::new("net.peer.name", host.to_string()),
            KeyValue::new("net.peer.port", i64::from(port)),
        ],
    )
}

pub(crate) fn begin_execute(command: &str, timeout_s: Option<f64>) -> SpanGuard {
    let cmd_preview = if command.len() > 256 {
        format!("{}...", &command[..256])
    } else {
        command.to_string()
    };
    let mut attrs = vec![KeyValue::new("ssh.command", cmd_preview)];
    if let Some(t) = timeout_s {
        attrs.push(KeyValue::new("ssh.timeout_seconds", t));
    }
    ssh_span("ssh.execute", attrs)
}

pub(crate) fn begin_upload(remote_path: &str, file_bytes: u64) -> SpanGuard {
    ssh_span(
        "ssh.upload",
        vec![
            KeyValue::new("ssh.remote_path", remote_path.to_string()),
            KeyValue::new(
                "ssh.file_bytes",
                i64::try_from(file_bytes).unwrap_or(i64::MAX),
            ),
        ],
    )
}

pub(crate) fn event_command_completed(exit_code: i64, stdout_bytes: usize, stderr_bytes: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "ssh.command.completed",
        vec![
            KeyValue::new("ssh.exit_code", exit_code),
            KeyValue::new(
                "ssh.stdout_bytes",
                i64::try_from(stdout_bytes).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "ssh.stderr_bytes",
                i64::try_from(stderr_bytes).unwrap_or(i64::MAX),
            ),
        ],
    );
}

// Retained for future integration with upload-complete spans in upload_file; not yet called.
#[allow(dead_code)]
pub(crate) fn event_upload_completed(bytes: u64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "ssh.upload.completed",
        vec![KeyValue::new(
            "ssh.bytes_transferred",
            i64::try_from(bytes).unwrap_or(i64::MAX),
        )],
    );
}

pub(crate) fn event_auth_success(method: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "ssh.auth.success",
        vec![KeyValue::new("ssh.auth_method", method.to_string())],
    );
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::SpanKind;
    use tumult_test_utils::capture_spans;

    use super::*;

    #[test]
    fn connect_span_carries_host_port_auth_and_net_peer_attributes() {
        let ((), spans) = capture_spans(|| {
            let _g = begin_connect("db.internal", 2222, "agent");
            event_auth_success("agent");
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "ssh.connect");
        assert_eq!(span.span_kind, SpanKind::Client);
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("ssh.host", "db.internal"),
                KeyValue::new("ssh.port", 2222_i64),
                KeyValue::new("ssh.auth_method", "agent"),
                KeyValue::new("net.peer.name", "db.internal"),
                KeyValue::new("net.peer.port", 2222_i64),
            ]
        );
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "ssh.auth.success");
        assert_eq!(
            events[0].attributes,
            [KeyValue::new("ssh.auth_method", "agent")]
        );
    }

    #[test]
    fn execute_span_carries_command_timeout_and_completion_event() {
        let ((), spans) = capture_spans(|| {
            let _g = begin_execute("uname -a", Some(30.0));
            event_command_completed(3, 100, 7);
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "ssh.execute");
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("ssh.command", "uname -a"),
                KeyValue::new("ssh.timeout_seconds", 30.0),
            ]
        );
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "ssh.command.completed");
        assert_eq!(
            events[0].attributes,
            [
                KeyValue::new("ssh.exit_code", 3_i64),
                KeyValue::new("ssh.stdout_bytes", 100_i64),
                KeyValue::new("ssh.stderr_bytes", 7_i64),
            ]
        );
    }

    #[test]
    fn execute_span_omits_timeout_when_none_and_truncates_long_commands() {
        let command = "x".repeat(300);
        let ((), spans) = capture_spans(|| {
            let _g = begin_execute(&command, None);
        });

        assert_eq!(
            spans[0].attributes,
            [KeyValue::new(
                "ssh.command",
                format!("{}...", "x".repeat(256))
            )]
        );
    }

    #[test]
    fn upload_span_carries_path_size_and_completion_event() {
        let ((), spans) = capture_spans(|| {
            let _g = begin_upload("/tmp/script.sh", 4096);
            event_upload_completed(4000);
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "ssh.upload");
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("ssh.remote_path", "/tmp/script.sh"),
                KeyValue::new("ssh.file_bytes", 4096_i64),
            ]
        );
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "ssh.upload.completed");
        assert_eq!(
            events[0].attributes,
            [KeyValue::new("ssh.bytes_transferred", 4000_i64)]
        );
    }
}
