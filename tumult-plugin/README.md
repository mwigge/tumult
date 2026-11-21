# tumult-plugin

The plugin system of Tumult. It finds script plugins on disk, runs their
scripts, and defines the trait that native plugins implement. To write a
script plugin, read [Write a plugin](../docs/plugins/authoring-guide.md).

## Key types

| Item | What it does |
|---|---|
| `NativeExecutor` | The trait of a native plugin: a name, a list of functions, and `execute` |
| `NativeExecutorRegistry` | Dispatches `plugin::function` to the executor that owns it |
| `ScriptPluginManifest` | The parsed `plugin.toon` of a script plugin |
| `discovery` | Finds script plugins in `./plugins`, `~/.tumult/plugins` and `TUMULT_PLUGIN_PATH` |
| `executor` | Runs a script with `/bin/sh`, its arguments as `TUMULT_*` variables, with a timeout |
| `NativeError` | The error of a native plugin call |

## A native plugin

```rust
use tumult_plugin::{NativeArgs, NativeError, NativeExecutor};

struct MyPlugin;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for MyPlugin {
    fn name(&self) -> &'static str {
        "my-plugin"
    }

    fn functions(&self) -> &'static [&'static str] {
        &["my_action"]
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        let _ = (function, args); // inject the fault here
        Ok("done".to_string())
    }
}
```

Register it in the native registry of `tumult-exec` (`tumult-exec/src/native.rs`),
and `tumult discover` and the runner both see it.
