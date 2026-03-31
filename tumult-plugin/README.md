# tumult-plugin

Plugin system for the Tumult chaos engineering platform -- define and load external actions and probes.

## Key Types

- `Plugin` -- trait for implementing custom chaos actions and probes
- `PluginLoader` -- discovers and loads plugins
- `PluginResult` -- execution result from a plugin

## Usage

```rust
use tumult_plugin::{Plugin, PluginResult};

struct MyAction;

impl Plugin for MyAction {
    async fn execute(&self, params: &serde_json::Value) -> PluginResult {
        // custom chaos logic
    }
}
```

## More Information

See the [main README](../README.md) for project overview and setup.
