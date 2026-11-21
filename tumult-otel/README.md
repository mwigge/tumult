# tumult-otel

OpenTelemetry for Tumult: the traces, metrics and logs of a run.

## Key types

- `TelemetryConfig`: the OTLP exporters and resource attributes, read from the environment
- `TumultTelemetry::new`: starts the OpenTelemetry SDK with the tracing bridge
- `TumultTelemetry::shutdown`: flushes and stops the exporters

## Use it from Rust

```rust
use tumult_otel::{TelemetryConfig, TumultTelemetry};

let telemetry = TumultTelemetry::new(TelemetryConfig::from_env());
// spans and metrics now go out over OTLP
telemetry.shutdown();
```

Where to send the data: [Observability setup](../docs/guides/observability-setup.md).
