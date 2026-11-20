# tumult-otel

OpenTelemetry instrumentation for the Tumult resilience intelligence platform -- traces, metrics, and logs for experiment observability.

## Key Types

- `TelemetryConfig` -- configures OTLP exporters and resource attributes
- `TumultTelemetry::new` -- initializes the OpenTelemetry SDK with tracing bridge
- `TumultTelemetry::shutdown` -- flushes and shuts down exporters

## Usage

```rust
use tumult_otel::{TelemetryConfig, TumultTelemetry};

let config = TelemetryConfig::from_env();
let telemetry = TumultTelemetry::new(config);
// spans and metrics are now exported via OTLP
telemetry.shutdown();
```

## More Information

See the [main README](../README.md) for project overview and setup.
