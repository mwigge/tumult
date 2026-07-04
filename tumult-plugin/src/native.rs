//! Native plugin execution — trait, registry, and typed errors.
//!
//! Native plugin crates (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`, …)
//! implement [`NativeExecutor`] and are registered once, at the composition
//! root, in a [`NativeExecutorRegistry`]. The registry dispatches a
//! `plugin::function` call to the owning executor, so adding a plugin or a
//! function never requires touching the dispatch site — only the plugin crate
//! and a single `register` call.
//!
//! Errors are typed as [`NativeError`] and converted to strings exactly once,
//! at the `ActivityOutcome` boundary in the caller.

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

/// JSON arguments passed to a native plugin function, as parsed from the
/// `provider.arguments` map of an experiment activity.
pub type NativeArgs = HashMap<String, serde_json::Value>;

/// Errors raised while dispatching or executing a native plugin function.
#[derive(Error, Debug)]
pub enum NativeError {
    /// The requested plugin is not registered.
    #[error("unknown native plugin: {plugin} (available: {available})")]
    UnknownPlugin {
        /// The plugin name that was requested.
        plugin: String,
        /// Comma-separated list of registered plugin names.
        available: String,
    },

    /// The plugin is registered but does not provide the requested function.
    #[error("unknown {plugin} function: {function} (available: {available})")]
    UnknownFunction {
        /// The plugin that was addressed.
        plugin: String,
        /// The function name that was requested.
        function: String,
        /// Comma-separated list of functions the plugin provides.
        available: String,
    },

    /// A required argument was absent or had the wrong JSON type.
    #[error("missing or invalid argument: {argument}")]
    MissingArgument {
        /// The argument key that was missing or mistyped.
        argument: String,
    },

    /// An argument was present but held an unusable value.
    #[error("invalid argument `{argument}`: {reason}")]
    InvalidArgument {
        /// The argument key that was invalid.
        argument: String,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },

    /// The underlying plugin operation failed.
    ///
    /// `message` is the rendered error (optionally with context) and
    /// `source` preserves the plugin crate's typed error for chain
    /// inspection.
    #[error("{message}")]
    Execution {
        /// Rendered error message shown at the outcome boundary.
        message: String,
        /// Underlying typed error from the plugin crate.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The plugin function ran to completion but reported failure
    /// (e.g. a remote command exited non-zero).
    #[error("{0}")]
    Failed(String),
}

impl NativeError {
    /// Build an [`NativeError::InvalidArgument`] from a key and a reason.
    #[must_use]
    pub fn invalid_argument(argument: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArgument {
            argument: argument.into(),
            reason: reason.into(),
        }
    }

    /// Build an [`NativeError::UnknownFunction`] listing the functions a
    /// plugin actually provides.
    #[must_use]
    pub fn unknown_function(plugin: &str, function: &str, available: &[&str]) -> Self {
        Self::UnknownFunction {
            plugin: plugin.to_string(),
            function: function.to_string(),
            available: available.join(", "),
        }
    }

    /// Wrap a plugin crate's typed error as an [`NativeError::Execution`].
    #[must_use]
    pub fn execution(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        let source = source.into();
        Self::Execution {
            message: source.to_string(),
            source,
        }
    }

    /// Wrap a plugin crate's typed error with a context prefix.
    #[must_use]
    pub fn execution_context(
        context: impl std::fmt::Display,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        let source = source.into();
        Self::Execution {
            message: format!("{context}: {source}"),
            source,
        }
    }
}

/// Extract a string argument.
///
/// # Errors
///
/// Returns [`NativeError::MissingArgument`] if the key is absent or the
/// value is not a JSON string.
pub fn arg_str<'a>(args: &'a NativeArgs, key: &str) -> Result<&'a str, NativeError> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| NativeError::MissingArgument {
            argument: key.to_string(),
        })
}

/// Extract an optional numeric argument, converting via `i64`.
///
/// Returns `None` when the key is absent, not a number, or out of range
/// for the target type.
#[must_use]
pub fn arg_num<T: TryFrom<i64>>(args: &NativeArgs, key: &str) -> Option<T> {
    args.get(key)?.as_i64()?.try_into().ok()
}

/// A native Rust plugin that can execute functions on behalf of the
/// experiment runner.
///
/// Implementations live in the plugin crates themselves (`tumult-ssh`,
/// `tumult-net`, `tumult-kubernetes`, …) so this crate stays
/// dependency-light and the dependency direction stays clean.
#[async_trait::async_trait(?Send)]
pub trait NativeExecutor: Send + Sync {
    /// Plugin name as referenced by `provider.plugin` in experiment files.
    fn name(&self) -> &'static str;

    /// The function names this plugin can execute, for discovery and
    /// dispatch validation.
    fn functions(&self) -> &'static [&'static str];

    /// Execute `function` with `args`, returning its textual output.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::UnknownFunction`] for a function not listed by
    /// [`Self::functions`], argument errors for missing or invalid inputs,
    /// and [`NativeError::Execution`] / [`NativeError::Failed`] when the
    /// underlying operation fails.
    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError>;
}

/// Dispatch table for [`NativeExecutor`] trait objects.
///
/// Plugins are keyed by [`NativeExecutor::name`]; lookup failures produce
/// typed errors listing what is available.
#[derive(Default)]
pub struct NativeExecutorRegistry {
    executors: BTreeMap<&'static str, Box<dyn NativeExecutor>>,
}

impl NativeExecutorRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native executor under its own [`NativeExecutor::name`].
    pub fn register(&mut self, executor: Box<dyn NativeExecutor>) {
        self.executors.insert(executor.name(), executor);
    }

    /// Names of all registered plugins, in sorted order.
    #[must_use]
    pub fn plugin_names(&self) -> Vec<&'static str> {
        self.executors.keys().copied().collect()
    }

    /// Look up a registered executor by plugin name.
    #[must_use]
    pub fn get(&self, plugin: &str) -> Option<&dyn NativeExecutor> {
        self.executors.get(plugin).map(AsRef::as_ref)
    }

    /// `plugin::function` names for every registered executor, sorted by
    /// plugin name — the native counterpart of a script plugin's action
    /// list, used by discovery.
    #[must_use]
    pub fn qualified_functions(&self) -> Vec<String> {
        self.executors
            .iter()
            .flat_map(|(name, executor)| {
                executor
                    .functions()
                    .iter()
                    .map(move |function| format!("{name}::{function}"))
            })
            .collect()
    }

    /// Dispatch `plugin::function` to the owning executor.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::UnknownPlugin`] or
    /// [`NativeError::UnknownFunction`] (each listing what is available)
    /// when the lookup fails, otherwise whatever the executor returns.
    pub async fn dispatch(
        &self,
        plugin: &str,
        function: &str,
        args: &NativeArgs,
    ) -> Result<String, NativeError> {
        let Some(executor) = self.executors.get(plugin) else {
            return Err(NativeError::UnknownPlugin {
                plugin: plugin.to_string(),
                available: self.plugin_names().join(", "),
            });
        };
        if !executor.functions().contains(&function) {
            return Err(NativeError::unknown_function(
                plugin,
                function,
                executor.functions(),
            ));
        }
        executor.execute(function, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoExecutor;

    #[async_trait::async_trait(?Send)]
    impl NativeExecutor for EchoExecutor {
        fn name(&self) -> &'static str {
            "tumult-echo"
        }

        fn functions(&self) -> &'static [&'static str] {
            &["echo", "fail"]
        }

        async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
            match function {
                "echo" => Ok(arg_str(args, "message")?.to_string()),
                "fail" => Err(NativeError::Failed("it failed".into())),
                _ => Err(NativeError::unknown_function(
                    self.name(),
                    function,
                    self.functions(),
                )),
            }
        }
    }

    fn registry() -> NativeExecutorRegistry {
        let mut registry = NativeExecutorRegistry::new();
        registry.register(Box::new(EchoExecutor));
        registry
    }

    #[tokio::test]
    async fn dispatch_routes_to_registered_executor() {
        let args = NativeArgs::from([("message".into(), serde_json::json!("hello"))]);
        let output = registry()
            .dispatch("tumult-echo", "echo", &args)
            .await
            .unwrap();
        assert_eq!(output, "hello");
    }

    #[tokio::test]
    async fn dispatch_unknown_plugin_lists_available() {
        let err = registry()
            .dispatch("tumult-nope", "echo", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownPlugin { .. }),
            "expected UnknownPlugin, got: {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("unknown native plugin: tumult-nope"));
        assert!(message.contains("tumult-echo"));
    }

    #[tokio::test]
    async fn dispatch_unknown_function_lists_available() {
        let err = registry()
            .dispatch("tumult-echo", "shout", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("unknown tumult-echo function: shout"));
        assert!(message.contains("echo, fail"));
    }

    #[tokio::test]
    async fn dispatch_propagates_executor_failure() {
        let err = registry()
            .dispatch("tumult-echo", "fail", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::Failed(_)));
        assert_eq!(err.to_string(), "it failed");
    }

    #[test]
    fn registry_lookup_and_names() {
        let registry = registry();
        assert_eq!(registry.plugin_names(), vec!["tumult-echo"]);
        assert!(registry.get("tumult-echo").is_some());
        assert!(registry.get("tumult-nope").is_none());
    }

    #[test]
    fn qualified_functions_lists_plugin_function_pairs() {
        let registry = registry();
        assert_eq!(
            registry.qualified_functions(),
            vec!["tumult-echo::echo", "tumult-echo::fail"]
        );
    }

    #[test]
    fn arg_str_missing_key_is_typed_error() {
        let err = arg_str(&NativeArgs::new(), "host").unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert_eq!(err.to_string(), "missing or invalid argument: host");
    }

    #[test]
    fn arg_str_rejects_non_string_value() {
        let args = NativeArgs::from([("host".into(), serde_json::json!(42))]);
        assert!(arg_str(&args, "host").is_err());
    }

    #[test]
    fn arg_num_converts_and_bounds_checks() {
        let args = NativeArgs::from([
            ("port".into(), serde_json::json!(22)),
            ("big".into(), serde_json::json!(70_000)),
        ]);
        assert_eq!(arg_num::<u16>(&args, "port"), Some(22));
        assert_eq!(arg_num::<u16>(&args, "big"), None);
        assert_eq!(arg_num::<u16>(&args, "absent"), None);
    }

    #[test]
    fn execution_error_preserves_source() {
        let io = std::io::Error::new(std::io::ErrorKind::AddrInUse, "boom");
        let err = NativeError::execution_context("proxy start failed", io);
        assert_eq!(err.to_string(), "proxy start failed: boom");
        assert!(std::error::Error::source(&err).is_some());
    }
}
