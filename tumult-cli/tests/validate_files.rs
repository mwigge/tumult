//! What `tumult validate` says about an experiment file.
//!
//! The binary reads real files: the experiment, and the manifests of two
//! script plugins in the `plugins` directory it is started in.
#![cfg(unix)]

mod lab;

use lab::{experiment, heal, inject, leave_marker, with_block, Lab};

/// The manifest of the plugin `name` with one action and one probe.
fn manifest(name: &str, action: &str, probe: &str) -> String {
    format!(
        "name: {name}
version: 0.1.0
description: a plugin of the lab
author: lab

actions[1]:
  - name: {action}
    script: actions/{action}.sh
    description: change the target

probes[1]:
  - name: {probe}
    script: probes/{probe}.sh
    description: read the target
"
    )
}

/// A lab with the script plugins `lab-target` and `lab-queue`.
fn lab_with_plugins() -> Lab {
    let lab = Lab::with_state("ok");
    let plugins = [
        ("lab-queue", manifest("lab-queue", "pause", "depth")),
        ("lab-target", manifest("lab-target", "take-down", "read")),
    ];
    for (name, manifest) in plugins {
        let dir = lab.path("plugins").join(name);
        std::fs::create_dir_all(&dir).expect("plugin directory");
        std::fs::write(dir.join("plugin.toon"), manifest).expect("manifest");
    }
    lab
}

/// A step of the method that calls `function` of the script plugin `plugin`.
fn script_step(plugin: &str, function: &str) -> String {
    format!(
        "  - name: fault
    activity_type: action
    provider:
      type: script
      plugin: {plugin}
      function: {function}
"
    )
}

/// The error the binary ended with: the `error.message` of its last log
/// line. Empty when it logged nothing.
fn fatal_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let Some(last) = text.lines().last() else {
        return String::new();
    };
    let log: serde_json::Value =
        serde_json::from_str(last).unwrap_or_else(|e| panic!("no log line ({e}): {text}"));
    log["error.message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Validate `definition`; whether it passed, what was printed and the error.
fn validate(lab: &Lab, definition: &str) -> (bool, String, String) {
    std::fs::write(lab.path("experiment.toon"), definition).expect("experiment");
    let output = lab
        .command(&["validate", "experiment.toon"])
        .env_remove("TUMULT_PLUGIN_PATH")
        .output()
        .expect("run tumult");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        fatal_error(&output.stderr),
    )
}

/// Validate an experiment whose method is one step of a script plugin.
fn validate_script_step(plugin: &str, function: &str) -> (bool, String, String) {
    let method = script_step(plugin, function);
    validate(&lab_with_plugins(), &experiment(&method, &leave_marker()))
}

#[test]
fn a_step_of_a_script_plugin_that_is_installed_passes() {
    for (plugin, function) in [
        ("lab-target", "take-down"),
        ("lab-target", "read"),
        ("lab-queue", "pause"),
        ("lab-queue", "depth"),
    ] {
        let (passed, stdout, _) = validate_script_step(plugin, function);

        assert!(passed, "{plugin}::{function}: {stdout}");
        assert!(stdout.ends_with("\nValidation passed.\n"), "{stdout}");
    }
}

#[test]
fn a_function_the_plugin_does_not_have_is_named_with_the_ones_it_has() {
    let (passed, stdout, error) = validate_script_step("lab-target", "pause");

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "1 step names something this binary cannot run:\n  \
         activity 'fault' uses unknown action 'lab-target::pause' (available: take-down, read)"
    );
}

#[test]
fn a_plugin_that_is_not_installed_is_named_with_the_ones_that_are() {
    let (passed, stdout, error) = validate_script_step("lab-cache", "flush");

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "1 step names something this binary cannot run:\n  \
         activity 'fault' uses unknown script plugin 'lab-cache' (available: lab-queue, lab-target)"
    );
}

#[test]
fn a_key_written_twice_in_a_step_is_named_with_both_lines() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    let definition = experiment(&method, &leave_marker());
    let twice = definition.replacen("  - name: heal\n", "  - name: heal\n    name: restore\n", 1);

    let (passed, stdout, error) = validate(&lab, &twice);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "failed to parse experiment: experiment.toon: \
         experiment file parse error: key 'name' is written twice in one object \
         (lines 28 and 29); only the last one would be used"
    );
}

/// The experiment of the lab with `block` among the keys of the file.
fn experiment_with(block: &str) -> String {
    let method = [inject(), heal()].concat();
    with_block(&experiment(&method, &leave_marker()), block)
}

/// An objective on `metric`.
fn objective_on(metric: &str) -> String {
    format!("nfr[1]{{id,kind,metric,operator,threshold}}:\n  fast,latency,{metric},lte,100\n")
}

/// A guard whose probe is `function` of the script plugin `plugin`.
fn guard_of(plugin: &str, function: &str) -> String {
    format!(
        r#"guards[1]:
  - name: watch
    probe:
      name: watch-read
      activity_type: probe
      provider:
        type: script
        plugin: {plugin}
        function: {function}
      tolerance:
        type: regex
        pattern: "^ok$"
"#
    )
}

/// A control that runs `function` of the script plugin `plugin`.
fn control_of(plugin: &str, function: &str) -> String {
    format!(
        "controls[1]:
  - name: lifecycle
    provider:
      type: script
      plugin: {plugin}
      function: {function}
"
    )
}

#[test]
fn an_objective_on_a_probe_of_the_steady_state_passes() {
    let definition = experiment_with(&objective_on("probe.steady.p95_ms"));

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(passed, "{stdout}{error}");
}

#[test]
fn an_objective_on_a_probe_the_steady_state_does_not_have_is_refused() {
    let definition = experiment_with(&objective_on("probe.health.p95_ms"));

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "nfr objective 'fast' is invalid: metric 'probe.health.p95_ms' names the probe \
         'health', which the steady-state hypothesis does not have (its probes: steady)"
    );
}

#[test]
fn an_objective_on_a_metric_no_run_measures_is_refused_with_the_ones_a_run_does() {
    let definition = experiment_with(&objective_on("latency.p42"));

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(!passed, "{stdout}");
    let known = error
        .strip_prefix(
            "nfr objective 'fast' is invalid: metric 'latency.p42' is not in the catalogue (known: ",
        )
        .unwrap_or_else(|| panic!("{error}"));
    assert!(known.starts_with("load.throughput_rps, "), "{known}");
    assert!(
        known.ends_with(", probe.<name>.<mean_ms|max_ms|p95_ms|error_rate>)"),
        "{known}"
    );
}

#[test]
fn a_guard_and_a_control_of_an_installed_script_plugin_pass() {
    let definition = experiment_with(
        &[
            guard_of("lab-target", "read"),
            control_of("lab-queue", "pause"),
        ]
        .concat(),
    );

    let (passed, stdout, error) = validate(&lab_with_plugins(), &definition);

    assert!(passed, "{stdout}{error}");
}

#[test]
fn a_guard_of_a_script_plugin_that_is_not_installed_is_named() {
    let definition = experiment_with(&guard_of("lab-cache", "read"));

    let (passed, stdout, error) = validate(&lab_with_plugins(), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "1 step names something this binary cannot run:\n  \
         guard 'watch' uses unknown script plugin 'lab-cache' (available: lab-queue, lab-target)"
    );
}

#[test]
fn a_control_of_a_function_its_script_plugin_does_not_have_is_named() {
    let definition = experiment_with(&control_of("lab-queue", "depth-of"));

    let (passed, stdout, error) = validate(&lab_with_plugins(), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "1 step names something this binary cannot run:\n  \
         control 'lifecycle' uses unknown action 'lab-queue::depth-of' (available: pause, depth)"
    );
}

#[test]
fn a_script_plugin_name_that_is_a_path_is_refused_for_a_guard_and_for_a_control() {
    let of_guard = experiment_with(&guard_of("../outside", "read"));
    let of_control = experiment_with(&control_of("../outside", "pause"));

    let (_, _, guard) = validate(&lab_with_plugins(), &of_guard);
    let (_, _, control) = validate(&lab_with_plugins(), &of_control);

    let refused = |owner: &str| {
        format!(
            "invalid script provider in activity '{owner}': \
             plugin '../outside' must not contain path separators"
        )
    };
    assert_eq!(guard, refused("watch"));
    assert_eq!(control, refused("lifecycle"));
}
