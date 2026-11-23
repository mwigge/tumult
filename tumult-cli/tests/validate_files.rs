//! What `tumult validate` says about an experiment file, and about the
//! starter that `tumult init` writes.
//!
//! The binary reads real files: the experiment, and the manifests of two
//! script plugins in the `plugins` directory it is started in.
#![cfg(unix)]

mod lab;

use lab::{experiment, fatal_error, heal, inject, leave_marker, with_block, Lab};

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

/// Validate `definition`; whether it passed, what was printed and the error.
fn validate(lab: &Lab, definition: &str) -> (bool, String, String) {
    validate_with(lab, definition, &[])
}

/// Validate `definition` with the flags `args` after the file.
fn validate_with(lab: &Lab, definition: &str, args: &[&str]) -> (bool, String, String) {
    std::fs::write(lab.path("experiment.toon"), definition).expect("experiment");
    let command: Vec<&str> = ["validate", "experiment.toon"]
        .iter()
        .chain(args)
        .copied()
        .collect();
    let output = lab
        .command(&command)
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
fn an_rpo_objective_on_a_metric_that_does_not_count_lost_data_is_refused() {
    let definition = experiment_with(
        "nfr[1]{id,kind,metric,operator,threshold}:\n  lost,rpo,fault.duration_s,lte,5\n",
    );

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "nfr objective 'lost' is invalid: kind rpo does not fit metric 'fault.duration_s', \
         which measures recovery; no metric of the catalogue measures rpo"
    );
}

/// An oha load of a minute shaped by `profile`, and an objective of
/// `kind` on `metric`, which reads the windows of that load.
fn load_objective(kind: &str, metric: &str, profile: &str) -> String {
    format!(
        "nfr[1]{{id,kind,metric,operator,threshold}}:\n  robust,{kind},{metric},lte,20\n\
         load:\n  tool: oha\n  script: http://127.0.0.1:8080/\n  vus: 2\n  duration_s: 60\n\
         {profile}"
    )
}

const SOAK: &str = "  profile:\n    type: soak\n    sample_interval_s: 10\n";
const SPIKE: &str = "  profile:\n    type: spike\n    peak_vus: 20\n    after_s: 20\n    hold_s: 10\n    sample_interval_s: 2\n";

#[test]
fn a_drift_objective_on_a_load_that_is_not_a_soak_is_refused_with_the_profile_it_needs() {
    let definition = experiment_with(&load_objective("latency", "load.p95_drift_pct", SPIKE));

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "nfr objective 'robust' is invalid: metric 'load.p95_drift_pct' needs a load with a \
         soak profile, and this load has a spike profile"
    );
}

#[test]
fn a_spike_objective_on_a_load_that_is_not_a_spike_is_refused_with_the_profile_it_needs() {
    let definition = experiment_with(&load_objective("recovery", "load.spike_recovery_s", SOAK));

    let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "nfr objective 'robust' is invalid: metric 'load.spike_recovery_s' needs a load with a \
         spike profile, and this load has a soak profile"
    );
}

#[test]
fn drift_and_spike_objectives_on_the_profiles_they_need_pass() {
    for (kind, metric, profile) in [
        ("latency", "load.p95_drift_pct", SOAK),
        ("error_rate", "load.error_rate_drift", SOAK),
        ("latency", "load.spike_recovery_s", SPIKE),
        ("recovery", "load.spike_recovery_s", SPIKE),
    ] {
        let definition = experiment_with(&load_objective(kind, metric, profile));

        let (passed, stdout, error) = validate(&Lab::with_state("ok"), &definition);

        assert!(passed, "{kind} on {metric}: {stdout}{error}");
    }
}

/// A step of the script plugin `lab-target` that takes down the node `node`.
fn take_down(node: &str) -> String {
    format!(
        "  - name: take-down
    activity_type: action
    provider:
      type: script
      plugin: lab-target
      function: take-down
      arguments:
        node: {node}
"
    )
}

#[test]
fn a_placeholder_that_nothing_fills_is_refused_and_named() {
    let definition = experiment(&take_down("${zone_a_node}"), &leave_marker());

    let (passed, stdout, error) = validate(&lab_with_plugins(), &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(
        error,
        "nothing fills ${zone_a_node}: give a ${name} with --var name=value, and declare a \
         ${config.name} or ${secrets.group.key} under configuration or secrets"
    );
}

#[test]
fn a_placeholder_that_a_var_fills_passes() {
    let definition = experiment(&take_down("${zone_a_node}"), &leave_marker());

    let (passed, stdout, error) = validate_with(
        &lab_with_plugins(),
        &definition,
        &["--var", "zone_a_node=node-1"],
    );

    assert!(passed, "{stdout}{error}");
}

#[test]
fn a_placeholder_of_a_declared_entry_passes_while_its_variable_is_not_set() {
    let definition = with_block(
        &experiment(&take_down("${config.zone_a_node}"), &leave_marker()),
        "configuration:\n  zone_a_node:\n    type: env\n    key: LAB_ZONE_A_NODE\n",
    );

    let (passed, stdout, error) = validate(&lab_with_plugins(), &definition);

    assert!(passed, "{stdout}{error}");
    assert!(
        stdout.contains("references env var 'LAB_ZONE_A_NODE' which is not set"),
        "{stdout}"
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

/// Run `tumult init` with `args` in `lab`.
fn init(lab: &Lab, args: &[&str]) -> std::process::Output {
    let all: Vec<&str> = ["init"].iter().chain(args).copied().collect();
    lab.command(&all)
        .env_remove("TUMULT_PLUGIN_PATH")
        .output()
        .expect("run tumult")
}

#[test]
fn the_starter_that_init_writes_for_a_plugin_reads_a_probe_of_it_and_passes() {
    let lab = lab_with_plugins();

    let output = init(&lab, &["--plugin", "lab-target"]);

    assert!(output.status.success(), "{}", fatal_error(&output.stderr));
    let starter = std::fs::read_to_string(lab.path("experiment.toon")).expect("the starter");
    let experiment = tumult_core::engine::parse_experiment(&starter).expect("an experiment");
    let reads_the_plugin = experiment.method.iter().any(|step| {
        matches!(&step.provider, tumult_core::types::Provider::Script { plugin, function, .. }
            if plugin == "lab-target" && function == "read")
    });
    assert!(reads_the_plugin, "{starter}");
    let (passed, stdout, error) = validate(&lab, &starter);
    assert!(passed, "{stdout}{error}");
}

#[test]
fn init_refuses_a_plugin_that_is_not_installed_and_writes_nothing() {
    let lab = lab_with_plugins();

    let output = init(&lab, &["--plugin", "lab-cache"]);

    assert!(!output.status.success());
    let error = fatal_error(&output.stderr);
    assert!(error.contains("lab-cache"), "{error}");
    assert!(error.contains("lab-queue, lab-target"), "{error}");
    assert!(!lab.path("experiment.toon").exists());
}
