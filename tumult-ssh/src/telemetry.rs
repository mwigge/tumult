//! OTel instrumentation for SSH operations.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER: &str = "tumult-ssh";

pub(crate) struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}

pub(crate) fn begin_connect(host: &str, port: u16, auth_method: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("ssh.connect")
        .with_kind(SpanKind::Client)
        .with_attributes(vec![
            KeyValue::new("ssh.host", host.to_string()),
            KeyValue::new("ssh.port", i64::from(port)),
            KeyValue::new("ssh.auth_method", auth_method.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard {
        _guard: cx.attach(),
    }
}

pub(crate) fn begin_execute(command: &str, timeout_s: Option<f64>) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let cmd_preview = if command.len() > 256 {
        format!("{}...", &command[..256])
    } else {
        command.to_string()
    };
    let mut attrs = vec![KeyValue::new("ssh.command", cmd_preview)];
    if let Some(t) = timeout_s {
        attrs.push(KeyValue::new("ssh.timeout_seconds", t));
    }
    let span = tracer
        .span_builder("ssh.execute")
        .with_kind(SpanKind::Client)
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard {
        _guard: cx.attach(),
    }
}

pub(crate) fn begin_upload(remote_path: &str, file_bytes: u64) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("ssh.upload")
        .with_kind(SpanKind::Client)
        .with_attributes(vec![
            KeyValue::new("ssh.remote_path", remote_path.to_string()),
            KeyValue::new("ssh.file_bytes", file_bytes as i64),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard {
        _guard: cx.attach(),
    }
}

pub(crate) fn event_command_completed(exit_code: i64, stdout_bytes: usize, stderr_bytes: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "ssh.command.completed",
        vec![
            KeyValue::new("ssh.exit_code", exit_code),
            KeyValue::new("ssh.stdout_bytes", stdout_bytes as i64),
            KeyValue::new("ssh.stderr_bytes", stderr_bytes as i64),
        ],
    );
}

#[allow(dead_code)]
pub(crate) fn event_upload_completed(bytes: u64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "ssh.upload.completed",
        vec![KeyValue::new("ssh.bytes_transferred", bytes as i64)],
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
    use super::*;

    #[test]
    fn connect_span_does_not_panic() {
        let _g = begin_connect("host.example.com", 22, "key");
        event_auth_success("key");
    }

    #[test]
    fn execute_span_does_not_panic() {
        let _g = begin_execute("uname -a", Some(30.0));
        event_command_completed(0, 100, 0);
    }

    #[test]
    fn upload_span_does_not_panic() {
        let _g = begin_upload("/tmp/script.sh", 4096);
        event_upload_completed(4096);
    }
}
