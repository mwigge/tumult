# tumult-plugin

Plugin system for the Tumult resilience intelligence platform -- define and load external actions and probes.

## Key Types

- `NativeExecutor` -- trait for implementing custom chaos actions and probes
- `discovery` -- discovers and loads script plugins (`plugin.toon`)
- `NativeError` -- error result from a plugin

## Usage

```rust
use tumult_plugin::{NativeArgs, NativeError, NativeExecutor};

struct MyAction;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for MyAction {
    fn name(&self) -> &'static str {
        "my-plugin"
    }

    fn functions(&self) -> &'static [&'static str] {
        &["my_action"]
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        // custom chaos logic
    }
}
```

## More Information

See the [main README](../README.md) for project overview and setup.
