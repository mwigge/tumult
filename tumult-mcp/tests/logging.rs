//! The `tumult-mcp` binary's logging contract: stdout belongs to the MCP
//! transport, so every log event is a structured line on stderr.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// Runs `tumult-mcp` with `args` in a clean environment. `home` isolates the
/// default `~/.tumult/mcp-auth.toml` lookup from the developer's machine.
fn tumult_mcp(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tumult-mcp"))
        .args(args)
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .env_remove("TUMULT_LOG_FORMAT")
        .env_remove("TUMULT_ENVIRONMENT")
        .env_remove("TUMULT_MCP_TOKEN")
        .env_remove("TUMULT_MCP_AUTH_CONFIG")
        .env_remove("OTEL_SERVICE_NAME")
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .expect("run tumult-mcp")
}

fn json_lines(stderr: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("stderr line is not JSON ({e}): {line}"))
        })
        .collect()
}

#[test]
fn a_refused_network_bind_is_a_structured_fatal_event_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    let output = tumult_mcp(
        home.path(),
        &["--transport", "http", "--host", "0.0.0.0", "--port", "0"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout is reserved for the MCP transport: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let events = json_lines(&output.stderr);
    for event in &events {
        for key in ["timestamp", "level", "target", "message", "service.name"] {
            assert!(event.get(key).is_some(), "missing {key}: {event}");
        }
    }
    assert!(
        events
            .iter()
            .any(|event| event["event.name"] == "mcp.auth.disabled"),
        "the open-mode warning passes the default level: {events:?}"
    );
    let fatal = events.last().expect("at least one event");
    assert_eq!(fatal["event.name"], "process.fatal");
    assert_eq!(fatal["level"], "ERROR");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.contains("non-loopback"), "{message}");
}

#[test]
fn a_malformed_auth_config_never_puts_a_token_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("auth.toml");
    std::fs::write(
        &config,
        "[[tokens]]\ntoken = \"tok-super-secret-value\nrole = \"viewer\"\n",
    )
    .unwrap();
    let output = tumult_mcp(home.path(), &["--auth-config", config.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("tok-super-secret-value"), "{stderr}");
    let events = json_lines(&output.stderr);
    let fatal = events.last().expect("at least one event");
    assert_eq!(fatal["event.name"], "process.fatal");
    let message = fatal["error.message"].as_str().unwrap();
    assert!(message.contains("invalid MCP auth config"), "{message}");
}
