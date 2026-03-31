# tumult-otel

OpenTelemetry instrumentation for the Tumult chaos engineering platform -- traces, metrics, and logs for experiment observability.

## Key Types

- `TelemetryConfig` -- configures OTLP exporters and resource attributes
- `init_telemetry` -- initializes the OpenTelemetry SDK with tracing bridge
- `shutdown_telemetry` -- flushes and shuts down exporters

## Usage

```rust
use tumult_otel::{TelemetryConfig, init_telemetry};

let config = TelemetryConfig::default();
let _guard = init_telemetry(&config)?;
// spans and metrics are now exported via OTLP
```

## More Information

See the [main README](../README.md) for project overview and setup.
