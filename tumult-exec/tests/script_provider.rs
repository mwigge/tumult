//! Behavior tests for the `script` provider dispatch: discovery failures,
//! action/probe resolution, argument-to-env mapping, and script exit-status
//! handling. A temporary plugin tree is published through
//! `TUMULT_PLUGIN_PATH`; all scenarios run in one test function so the
//! process-wide environment mutation stays serialized.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;

use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType, Provider};
use tumult_exec::ProviderExecutor;

struct TempPluginTree(PathBuf);

impl TempPluginTree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("tumult-exec-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TempPluginTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn script_activity(
    plugin: &str,
    function: &str,
    arguments: HashMap<String, serde_json::Value>,
) -> Activity {
    Activity {
        name: "script-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Script {
            plugin: plugin.into(),
            function: function.into(),
            arguments,
            timeout_s: Some(10.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

#[test]
fn script_provider_scenarios() {
    let tree = TempPluginTree::new();
    tree.write(
        "cov-test-plugin/plugin.toon",
        "name: cov-test-plugin\n\
         version: 0.1.0\n\
         description: Coverage test plugin\n\
         actions[3]:\n\
         \x20 - name: greet\n\
         \x20   script: actions/greet.sh\n\
         \x20   description: Echo the greeting argument\n\
         \x20 - name: fail-with-stderr\n\
         \x20   script: actions/fail-with-stderr.sh\n\
         \x20   description: Exit non-zero with stderr\n\
         \x20 - name: missing-script\n\
         \x20   script: actions/not-on-disk.sh\n\
         \x20   description: Points at a script that does not exist\n\
         probes[1]:\n\
         \x20 - name: health\n\
         \x20   script: probes/health.sh\n\
         \x20   description: Probe entry\n",
    );
    tree.write(
        "cov-test-plugin/actions/greet.sh",
        "echo \"$TUMULT_GREETING:$TUMULT_COUNT\"\n",
    );
    tree.write(
        "cov-test-plugin/actions/fail-with-stderr.sh",
        "echo script-broke >&2\nexit 2\n",
    );
    tree.write("cov-test-plugin/probes/health.sh", "echo probe-ok\n");
    tree.write(
        "cov-quiet-plugin/plugin.toon",
        "name: cov-quiet-plugin\n\
         version: 0.1.0\n\
         description: Plugin whose action fails silently\n\
         actions[1]:\n\
         \x20 - name: fail-quietly\n\
         \x20   script: actions/fail-quietly.sh\n\
         \x20   description: Exit non-zero without any output\n\
         probes[0]:\n",
    );
    tree.write("cov-quiet-plugin/actions/fail-quietly.sh", "exit 3\n");

    // Publish the tree through the custom search path (serialized by running
    // every scenario inside this single test function).
    std::env::set_var("TUMULT_PLUGIN_PATH", &tree.0);
    let executor = ProviderExecutor::new();

    // Unknown plugin names what is available instead of failing silently.
    let outcome = executor.execute(&script_activity(
        "no-such-plugin-xyz",
        "greet",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown script plugin: no-such-plugin-xyz"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("cov-test-plugin"),
        "error must list available plugins: {error}"
    );

    // Unknown function on a known plugin names the available entries.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "no-such-action",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown cov-test-plugin action: no-such-action"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("greet") && error.contains("health"),
        "error must list available actions and probes: {error}"
    );

    // Arguments reach the script as TUMULT_* env vars; non-string values use
    // their JSON representation.
    let arguments = HashMap::from([
        (
            "greeting".to_string(),
            serde_json::Value::String("hi".to_string()),
        ),
        ("count".to_string(), serde_json::json!(3)),
    ]);
    let outcome = executor.execute(&script_activity("cov-test-plugin", "greet", arguments));
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_deref(), Some("hi:3"));

    // A function missing from actions falls back to the manifest's probes.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "health",
        HashMap::new(),
    ));
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_deref(), Some("probe-ok"));

    // Non-zero exit with stderr surfaces the stderr text.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "fail-with-stderr",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    assert_eq!(outcome.error.as_deref(), Some("script-broke"));

    // Non-zero exit without stderr names the script and its exit code.
    let outcome = executor.execute(&script_activity(
        "cov-quiet-plugin",
        "fail-quietly",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("fail-quietly.sh") && error.contains("exit code 3"),
        "unexpected error: {error}"
    );

    // A manifest pointing at a missing script reports the executor error.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "missing-script",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("script not found"),
        "unexpected error: {error}"
    );

    std::env::remove_var("TUMULT_PLUGIN_PATH");
}
