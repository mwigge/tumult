//! Tests for the bundled script plugins.
//!
//! Validates that:
//! 1. Plugin manifests parse correctly as TOON
//! 2. Probe scripts execute and produce output on the current platform
//! 3. Scripts validate required environment variables
//! 4. Scripts that query a service fail when the query fails

mod lab;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use lab::{result, step, Lab};
use tumult_core::types::{ActivityStatus, ExperimentStatus};

/// Get workspace root (parent of tumult-cli/).
fn ws() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn plugin_path(relative: &str) -> String {
    ws().join("plugins")
        .join(relative)
        .to_str()
        .unwrap()
        .to_string()
}

/// Parse a plugin.toon manifest and verify it has expected fields.
fn assert_manifest_valid(plugin_dir: &str) {
    let path = plugin_path(&format!("{plugin_dir}/plugin.toon"));
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("manifest not found: {path}"));
    assert!(content.contains("name:"), "manifest missing name field");
    assert!(
        content.contains("version:"),
        "manifest missing version field"
    );
    assert!(
        content.contains("actions") || content.contains("probes"),
        "manifest missing actions/probes"
    );
}

fn assert_script_executable(relative: &str) {
    let path = plugin_path(relative);
    let metadata = std::fs::metadata(&path).unwrap_or_else(|_| panic!("script not found: {path}"));
    let mode = metadata.permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "script not executable: {path} (mode: {mode:o})"
    );
}

fn run_script(relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let path = plugin_path(relative);
    let mut cmd = Command::new("sh");
    cmd.arg(&path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run {path}: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════
// tumult-stress
// ═══════════════════════════════════════════════════════════════

#[test]
fn stress_manifest_parses() {
    assert_manifest_valid("tumult-stress");
}

#[test]
fn stress_scripts_are_executable() {
    for script in &[
        "tumult-stress/actions/cpu-stress.sh",
        "tumult-stress/actions/memory-stress.sh",
        "tumult-stress/actions/io-stress.sh",
        "tumult-stress/actions/combined-stress.sh",
        "tumult-stress/probes/cpu-utilization.sh",
        "tumult-stress/probes/memory-utilization.sh",
        "tumult-stress/probes/io-utilization.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn cpu_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/cpu-utilization.sh", &[]);
    assert_eq!(code, 0, "cpu-utilization probe failed");
    let value: f64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("cpu-utilization output not a number: '{}'", stdout.trim()));
    assert!(
        (0.0..=100.0).contains(&value),
        "cpu value out of range: {value}"
    );
}

#[test]
fn memory_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/memory-utilization.sh", &[]);
    assert_eq!(code, 0, "memory-utilization probe failed");
    let value: f64 = stdout.trim().parse().unwrap_or_else(|_| {
        panic!(
            "memory-utilization output not a number: '{}'",
            stdout.trim()
        )
    });
    assert!(
        (0.0..=100.0).contains(&value),
        "memory value out of range: {value}"
    );
}

#[test]
fn stress_action_fails_without_stressng() {
    let (code, _, stderr) = run_script(
        "tumult-stress/actions/cpu-stress.sh",
        &[("TUMULT_TIMEOUT", "1")],
    );
    if code != 0 {
        assert!(
            stderr.contains("stress-ng not found"),
            "expected stress-ng not found error, got: {stderr}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// tumult-containers
// ═══════════════════════════════════════════════════════════════

#[test]
fn containers_manifest_parses() {
    assert_manifest_valid("tumult-containers");
}

#[test]
fn containers_scripts_are_executable() {
    for script in &[
        "tumult-containers/actions/kill-container.sh",
        "tumult-containers/actions/stop-container.sh",
        "tumult-containers/actions/pause-container.sh",
        "tumult-containers/actions/unpause-container.sh",
        "tumult-containers/actions/limit-cpu.sh",
        "tumult-containers/actions/limit-memory.sh",
        "tumult-containers/probes/container-status.sh",
        "tumult-containers/probes/container-health.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn container_action_requires_container_id() {
    let (code, _, stderr) = run_script("tumult-containers/actions/kill-container.sh", &[]);
    assert_ne!(code, 0, "should fail without TUMULT_CONTAINER_ID");
    assert!(
        stderr.contains("TUMULT_CONTAINER_ID"),
        "error should mention missing var, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════
// tumult-process
// ═══════════════════════════════════════════════════════════════

#[test]
fn process_manifest_parses() {
    assert_manifest_valid("tumult-process");
}

#[test]
fn process_scripts_are_executable() {
    for script in &[
        "tumult-process/actions/kill-process.sh",
        "tumult-process/actions/suspend-process.sh",
        "tumult-process/actions/resume-process.sh",
        "tumult-process/probes/process-exists.sh",
        "tumult-process/probes/process-resources.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn process_exists_probe_detects_current_shell() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "true");
}

#[test]
fn process_exists_probe_detects_missing_pid() {
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", "999999999")],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "false");
}

#[test]
fn process_resources_probe_returns_json() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-resources.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("not valid JSON: '{}'", stdout.trim()));
    assert!(parsed.get("cpu_percent").is_some());
    assert!(parsed.get("mem_percent").is_some());
    assert!(parsed.get("running").is_some());
}

#[test]
fn process_action_requires_target() {
    let (code, _, stderr) = run_script("tumult-process/actions/kill-process.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PID") || stderr.contains("required"),
        "error should mention missing target, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Scripts that call a service or a tool of the machine, run against stand-ins
//
// The service tools these scripts call (the Kafka tools, docker, mysql, psql
// and redis-cli), the tools that read or change the machine (tc, ping, top)
// and uname are stand-ins this file writes into a temp dir, and every test
// checks that its stand-ins ran. No such tool of the machine is started.
// ═══════════════════════════════════════════════════════════════

const CONSUMER_LAG: &str = "tumult-kafka/probes/consumer-lag.sh";
const UNDER_REPLICATED: &str = "tumult-kafka/probes/under-replicated.sh";
const BROKER_COUNT: &str = "tumult-kafka/probes/broker-count.sh";
const CLOCK_OFFSET: &str = "tumult-timewarp/probes/clock-offset.sh";
const REDIS_INFO: &str = "tumult-db-redis/probes/redis-info.sh";
const MYSQL_KILL_CONNECTIONS: &str = "tumult-db-mysql/actions/kill-connections.sh";
const POSTGRES_KILL_CONNECTIONS: &str = "tumult-db-postgres/actions/kill-connections.sh";
const POSTGRES_BACKUP: &str = "tumult-db-postgres/actions/backup.sh";
const POSTGRES_RESTORE: &str = "tumult-db-postgres/actions/restore.sh";
const POSTGRES_STOP_PRIMARY: &str = "tumult-db-postgres/actions/stop-primary.sh";
const POSTGRES_START_PRIMARY: &str = "tumult-db-postgres/actions/start-primary.sh";
const POSTGRES_PROMOTE_REPLICA: &str = "tumult-db-postgres/actions/promote-replica.sh";
const POSTGRES_WRITE_MARKERS: &str = "tumult-db-postgres/actions/write-markers.sh";
const POSTGRES_MARKERS_LOST: &str = "tumult-db-postgres/probes/markers-lost.sh";

/// The service tools the scripts look for on `PATH`, and the tools of the
/// machine they read or change.
const SERVICE_TOOLS: [&str; 16] = [
    "docker",
    "mysql",
    "psql",
    "pg_dump",
    "pg_restore",
    "redis-cli",
    "kafka-topics",
    "kafka-topics.sh",
    "kafka-consumer-groups",
    "kafka-consumer-groups.sh",
    "kafka-broker-api-versions",
    "kafka-broker-api-versions.sh",
    "tc",
    "ping",
    "top",
    "uname",
];

/// The shell, and the tools of the machine a script needs to run at all.
const SHELL_TOOLS: [&str; 5] = ["sh", "cat", "dirname", "grep", "sed"];

/// The tool `name` on the `PATH` of this test.
fn tool_of_the_machine(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|tool| tool.is_file())
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

/// What a service tool does until a test gives it an answer.
const NO_STAND_IN: &str = "echo \"$0: this test gave the tool no answer\" >&2\nexit 97";

/// Stand-ins for the tools of a script, in a temp dir. A stand-in writes
/// its name and its arguments to `calls` before it answers, so a test sees
/// that the stand-in ran and not a tool of the machine.
struct Stubs {
    dir: tempfile::TempDir,
}

impl Stubs {
    /// Every service tool starts as a stand-in that refuses, so a test that
    /// forgets one cannot start a tool of the machine.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join("bin")).expect("bin dir");
        let stubs = Self { dir };
        for name in SERVICE_TOOLS {
            stubs.tool(&format!("bin/{name}"), NO_STAND_IN);
        }
        stubs
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A stand-in at `relative` that leaves its call in `calls`, then runs
    /// `answer`. Tools that a script finds on `PATH` go in `bin`, the Kafka
    /// tools in `kafka/bin`.
    fn tool(&self, relative: &str, answer: &str) {
        let path = self.path(relative);
        let dir = path.parent().expect("a tool has a dir");
        let name = path
            .file_name()
            .expect("a tool has a name")
            .to_string_lossy();
        std::fs::create_dir_all(dir).expect("tool dir");
        let calls = self.path("calls");
        let record = format!(r#"printf '%s\n' "{name} $*" >> '{}'"#, calls.display());
        tumult_test_utils::fake_bin(dir, &name, &format!("{record}\n{answer}"));
    }

    /// The calls of the stand-ins, one line each: the name and the arguments.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Run the plugin script at `relative` with `sh`: `bin` comes first on
    /// `PATH`, the Kafka tools are looked for in `kafka`, and the temp dir
    /// is `HOME` and `TMPDIR`.
    fn run(&self, relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        self.run_on(&path, relative, env)
    }

    /// Run the plugin script at `relative` as `run` does, with only `bin`
    /// and links to the shell tools of the machine on `PATH`. A tool the
    /// script looks for first, such as `pumba`, is then not found unless
    /// the test made a stand-in for it.
    fn run_alone(&self, relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let tools = self.path("tools");
        std::fs::create_dir(&tools).expect("tools dir");
        for name in SHELL_TOOLS {
            std::os::unix::fs::symlink(tool_of_the_machine(name), tools.join(name))
                .expect("link a shell tool");
        }
        let path = format!("{}:{}", self.path("bin").display(), tools.display());
        self.run_on(&path, relative, env)
    }

    fn run_on(&self, path: &str, relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let output = Command::new("sh")
            .arg(plugin_path(relative))
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("TMPDIR", self.dir.path())
            .env("TUMULT_KAFKA_DIR", self.path("kafka"))
            .envs(env.iter().copied())
            .output()
            .expect("run sh");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }
}

/// The probe failed, printed no reading, and passed on `message`, the
/// line its tool wrote on stderr.
fn assert_failed_with(run: &(i32, String, String), message: &str) {
    let (code, stdout, stderr) = run;
    assert_ne!(*code, 0, "a query that failed is not a reading: {stdout}");
    assert_eq!(stdout, "", "no reading on stdout");
    assert!(
        stderr.contains(message),
        "the message of the tool: {stderr}"
    );
}

/// A script whose tool got no answer from the test reaches the refusing
/// stand-in, not a tool of the machine.
#[test]
fn a_tool_the_test_gave_no_answer_refuses_and_no_tool_of_the_machine_runs() {
    let stubs = Stubs::new();

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), ["redis-cli -h localhost -p 6379 INFO"]);
    assert_eq!(run.0, 97, "{}", run.2);
    assert_failed_with(&run, "this test gave the tool no answer");
}

// ── tumult-kafka ──────────────────────────────────────────────

/// What a Kafka tool writes when no broker answers.
const KAFKA_DOWN: &str =
    "org.apache.kafka.common.errors.TimeoutException: Timed out waiting for a node assignment.";

/// A Kafka tool that finds no broker.
fn kafka_down() -> String {
    format!("echo '{KAFKA_DOWN}' >&2\nexit 1")
}

const DESCRIBE_GROUP: &str =
    "kafka-consumer-groups.sh --bootstrap-server localhost:9092 --group orders --describe";
const DESCRIBE_UNDER_REPLICATED: &str =
    "kafka-topics.sh --bootstrap-server localhost:9092 --describe --under-replicated-partitions";

/// `kafka-consumer-groups --describe` for a group of two partitions whose
/// lags are `first` and `second`.
fn group_with_lag(first: u32, second: u32) -> String {
    format!(
        "cat <<'EOF'

GROUP   TOPIC   PARTITION  CURRENT-OFFSET  LOG-END-OFFSET  LAG  CONSUMER-ID  HOST       CLIENT-ID
orders  orders  0          120             {}             {first}    c-1          /10.0.0.4  c-1
orders  orders  1          300             {}             {second}    c-1          /10.0.0.4  c-1
EOF",
        120 + first,
        300 + second
    )
}

#[test]
fn consumer_lag_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &kafka_down());

    let run = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn consumer_lag_probe_reads_zero_when_every_partition_is_caught_up() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &group_with_lag(0, 0));

    let (code, stdout, stderr) = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn consumer_lag_probe_sums_the_lag_of_every_partition() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &group_with_lag(5, 7));

    let (code, stdout, stderr) = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "12\n");
}

#[test]
fn under_replicated_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-topics.sh", &kafka_down());

    let run = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn under_replicated_probe_prints_one_zero_for_a_healthy_cluster() {
    let stubs = Stubs::new();
    // A healthy cluster has no partition to list.
    stubs.tool("kafka/bin/kafka-topics.sh", "exit 0");

    let (code, stdout, stderr) = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn under_replicated_probe_counts_each_partition_that_lacks_a_replica() {
    let stubs = Stubs::new();
    stubs.tool(
        "kafka/bin/kafka-topics.sh",
        "cat <<'EOF'
\tTopic: orders\tPartition: 1\tLeader: 1\tReplicas: 1,2,3\tIsr: 1,3
\tTopic: orders\tPartition: 2\tLeader: 3\tReplicas: 3,1,2\tIsr: 3,1
EOF",
    );

    let (code, stdout, stderr) = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "2\n");
}

#[test]
fn broker_count_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-broker-api-versions.sh", &kafka_down());

    let run = stubs.run(BROKER_COUNT, &[]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server localhost:9092"]
    );
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn broker_count_probe_prints_one_zero_when_no_broker_is_listed() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-broker-api-versions.sh", "exit 0");

    let (code, stdout, stderr) = stubs.run(BROKER_COUNT, &[]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server localhost:9092"]
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn broker_count_probe_counts_every_broker_of_the_cluster() {
    let stubs = Stubs::new();
    // Each broker is one line with its id, then the API versions it serves.
    stubs.tool(
        "kafka/bin/kafka-broker-api-versions.sh",
        "cat <<'EOF'
kafka-1:9092 (id: 1 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
kafka-2:9092 (id: 2 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
kafka-3:9092 (id: 3 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
EOF",
    );

    let (code, stdout, stderr) =
        stubs.run(BROKER_COUNT, &[("TUMULT_KAFKA_BOOTSTRAP", "kafka-1:9092")]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server kafka-1:9092"]
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "3\n");
}

/// The check of the steady state in the example of
/// `docs/plugins/tumult-kafka.md`.
const DOCUMENTED_REPLICA_CHECK: &str = "    - name: check-replicas
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/under-replicated.sh
        env:
          TUMULT_KAFKA_BOOTSTRAP: broker-1:9092
      tolerance:
        type: exact
        value: 0
";

/// The lag check of the example in `docs/blog/05-experiment-format.md`,
/// with its configuration filled in.
const DOCUMENTED_LAG_CHECK: &str = "    - name: consumer-lag
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/consumer-lag.sh
        env:
          TUMULT_KAFKA_BOOTSTRAP: broker-1:9092
          TUMULT_CONSUMER_GROUP: orders
      tolerance:
        type: range
        from: 0
        to: 100
";

/// An experiment whose steady state is `probe` and whose method does
/// nothing. A `plugins` link in the lab lets the probe name its script by
/// the path the docs give.
fn guarded_by(lab: &Lab, probe: &str) -> String {
    std::os::unix::fs::symlink(ws().join("plugins"), lab.path("plugins"))
        .expect("link the plugins");
    format!(
        "title: A plugin probe guards the steady state
description: The method does nothing

steady_state_hypothesis:
  title: The cluster is healthy
  probes[1]:
{probe}
method[1]:
{}",
        step("no-op", "action", "true")
    )
}

/// Every call of the stand-ins is `expected`, and there was one at least.
fn assert_only_calls(stubs: &Stubs, expected: &str) {
    let calls = stubs.calls();
    assert!(!calls.is_empty(), "the stand-in never ran");
    assert!(calls.iter().all(|call| call == expected), "{calls:?}");
}

#[test]
fn the_documented_kafka_check_passes_on_a_healthy_cluster() {
    let lab = Lab::with_state("ok");
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-topics.sh", "exit 0");

    let output = lab
        .run_command(&guarded_by(&lab, DOCUMENTED_REPLICA_CHECK))
        .env("TUMULT_KAFKA_DIR", stubs.path("kafka"))
        .output()
        .expect("run tumult");

    assert_only_calls(
        &stubs,
        "kafka-topics.sh --bootstrap-server broker-1:9092 --describe --under-replicated-partitions",
    );
    let journal = lab.journal(&output);
    assert_eq!(
        journal.status,
        ExperimentStatus::Completed,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0));
    let before = journal.steady_state_before.expect("the check ran");
    let check = result(&before.probe_results, "check-replicas");
    assert_eq!(check.output.as_deref(), Some("0"));
}

#[test]
fn a_consumer_lag_check_aborts_the_run_when_the_kafka_tool_is_down() {
    let lab = Lab::with_state("ok");
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &kafka_down());

    let output = lab
        .run_command(&guarded_by(&lab, DOCUMENTED_LAG_CHECK))
        .env("TUMULT_KAFKA_DIR", stubs.path("kafka"))
        .output()
        .expect("run tumult");

    assert_only_calls(
        &stubs,
        "kafka-consumer-groups.sh --bootstrap-server broker-1:9092 --group orders --describe",
    );
    let journal = lab.journal(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(journal.method_results.is_empty(), "the method did not run");
    let before = journal.steady_state_before.expect("the check ran");
    let lag = result(&before.probe_results, "consumer-lag");
    assert_eq!(lag.status, ActivityStatus::Failed);
    let error = lag.error.unwrap_or_default();
    assert!(error.contains(KAFKA_DOWN), "{error}");
}

// ── tumult-timewarp ───────────────────────────────────────────

/// The epoch the stand-in for `date` gives the runner.
const RUNNER_EPOCH: &str = "1700000000";
const DATE_CALL: &str = "date -u +%s";
const EXEC_CALL: &str = "docker exec app sh -c date -u +%s";

/// Stand-ins for `date`, which tells the runner `RUNNER_EPOCH`, and for
/// `docker`, which runs `answer` for the target.
fn clock_stubs(answer: &str) -> Stubs {
    let stubs = Stubs::new();
    stubs.tool("bin/date", &format!("echo {RUNNER_EPOCH}"));
    stubs.tool("bin/docker", answer);
    stubs
}

#[test]
fn clock_offset_probe_fails_with_the_message_of_docker_when_exec_fails() {
    let stubs =
        clock_stubs("echo 'Error response from daemon: No such container: app' >&2\nexit 1");

    let run = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_failed_with(&run, "No such container: app");
}

#[test]
fn clock_offset_probe_fails_when_the_target_answers_no_number() {
    let stubs = clock_stubs("echo 'Thu Nov 19 12:00:00 UTC 2026'");

    let run = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_failed_with(&run, "Thu Nov 19 12:00:00 UTC 2026");
}

#[test]
fn clock_offset_probe_reads_zero_when_the_clocks_agree() {
    let stubs = clock_stubs(&format!("echo {RUNNER_EPOCH}"));

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn clock_offset_probe_reads_how_far_the_target_clock_is_ahead() {
    let stubs = clock_stubs("echo 1700003600");

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "3600\n");
}

#[test]
fn clock_offset_probe_reads_zero_without_a_target_and_asks_no_container() {
    let stubs = clock_stubs("exit 1");

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "")]);

    assert_eq!(stubs.calls(), [DATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

// ── tumult-db-redis ───────────────────────────────────────────

const REDIS_DOWN: &str = "Could not connect to Redis at localhost:6379: Connection refused";
const INFO_CALL: &str = "redis-cli -h localhost -p 6379 INFO";

#[test]
fn redis_info_probe_fails_with_the_message_of_redis_cli_when_it_fails() {
    let stubs = Stubs::new();
    stubs.tool("bin/redis-cli", &format!("echo '{REDIS_DOWN}' >&2\nexit 1"));

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_failed_with(&run, REDIS_DOWN);
}

#[test]
fn redis_info_probe_fails_on_an_error_reply_that_exits_zero() {
    let stubs = Stubs::new();
    stubs.tool("bin/redis-cli", "echo 'NOAUTH Authentication required.'");

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_failed_with(&run, "NOAUTH Authentication required.");
}

#[test]
fn redis_info_probe_reads_the_stats_of_a_healthy_server() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/redis-cli",
        r"printf '# Server\r\nuptime_in_seconds:3600\r\n\r\n# Clients\r\nconnected_clients:3\r\n'
printf '\r\n# Memory\r\nused_memory:1048576\r\nused_memory_human:1.00M\r\n'
printf '\r\n# Stats\r\ninstantaneous_ops_per_sec:12\r\nkeyspace_hits:40\r\nkeyspace_misses:2\r\n'",
    );

    let (code, stdout, stderr) = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        r#"{"connected_clients": 3, "used_memory_bytes": 1048576, "used_memory_human": "1.00M", "ops_per_sec": 12, "keyspace_hits": 40, "keyspace_misses": 2, "uptime_seconds": 3600}"#
    );
}

// ── tumult-db-mysql ───────────────────────────────────────────

const PROCESSLIST_CALL: &str = "mysql -h localhost -P 3306 -u root -N -e SELECT id FROM information_schema.processlist WHERE db = 'app' AND id <> CONNECTION_ID();";

fn kill_call(id: u32) -> String {
    format!("mysql -h localhost -P 3306 -u root -e KILL {id};")
}

#[test]
fn mysql_kill_connections_counts_only_the_connections_it_killed() {
    let stubs = Stubs::new();
    // Connection 12 ends on its own between the list and its KILL.
    stubs.tool(
        "bin/mysql",
        r#"case "$*" in
    *SELECT*) printf '11\n12\n13\n' ;;
    *"KILL 12;"*) echo 'ERROR 1094 (HY000) at line 1: Unknown thread id: 12' >&2; exit 1 ;;
esac"#,
    );

    let (code, stdout, stderr) =
        stubs.run(MYSQL_KILL_CONNECTIONS, &[("TUMULT_MYSQL_DATABASE", "app")]);

    assert_eq!(
        stubs.calls(),
        [
            PROCESSLIST_CALL.to_string(),
            kill_call(11),
            kill_call(12),
            kill_call(13)
        ]
    );
    assert_eq!(
        code, 0,
        "a connection that went away is no failure: {stderr}"
    );
    assert_eq!(
        stdout,
        "killing connections to app\nkilled 2 connections to app\n"
    );
    assert!(stderr.contains("Unknown thread id: 12"), "{stderr}");
}

#[test]
fn mysql_kill_connections_fails_with_the_message_of_mysql_when_the_list_fails() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/mysql",
        r#"echo "ERROR 2003 (HY000): Can't connect to MySQL server on 'localhost:3306' (111)" >&2
exit 1"#,
    );

    let (code, stdout, stderr) =
        stubs.run(MYSQL_KILL_CONNECTIONS, &[("TUMULT_MYSQL_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [PROCESSLIST_CALL]);
    assert_ne!(code, 0, "{stdout}");
    assert!(!stdout.contains("killed"), "{stdout}");
    assert!(stderr.contains("ERROR 2003 (HY000)"), "{stderr}");
}

// ── tumult-db-postgres ────────────────────────────────────────

const TERMINATE_CALL: &str = "psql -h localhost -p 5432 -U postgres -d postgres -t -c SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $$app$$ AND pid <> pg_backend_pid();";

#[test]
fn postgres_kill_connections_prints_one_zero_when_no_connection_is_open() {
    let stubs = Stubs::new();
    stubs.tool("bin/psql", "echo ''");

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "terminated 0 connections to app\n");
}

#[test]
fn postgres_kill_connections_counts_only_the_rows_that_say_t() {
    let stubs = Stubs::new();
    // Backend 4242 ended on its own: the server warns and answers f.
    stubs.tool(
        "bin/psql",
        r"printf ' t\n t\n f\n\n'
echo 'WARNING:  PID 4242 is not a PostgreSQL backend process' >&2",
    );

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "terminated 2 connections to app\n");
}

#[test]
fn postgres_kill_connections_fails_with_the_message_of_psql() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/psql",
        r#"echo 'psql: error: connection to server at "localhost" (::1), port 5432 failed: Connection refused' >&2
exit 2"#,
    );

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_ne!(code, 0, "{stdout}");
    assert!(!stdout.contains("terminated"), "{stdout}");
    assert!(
        stderr.contains("port 5432 failed: Connection refused"),
        "{stderr}"
    );
}

// ── tumult-db-postgres: disaster recovery ─────────────────────

/// The connection flags the DR scripts give every client tool. `-w`: a
/// tool never waits for a password on a prompt.
const PG_CONNECTION: &str = "-h localhost -p 5432 -U postgres -w";
/// How the scripts ask `date` for the time, and the time the stand-in tells.
const NOW_CALL: &str = "date +%s.%N";
const NOW: &str = "1700000000.250";

/// Stand-ins with a `date` that tells the time `NOW`, to the nanosecond.
fn pg_stubs() -> Stubs {
    let stubs = Stubs::new();
    stubs.tool("bin/date", "echo 1700000000.250000000");
    stubs
}

/// A call of `psql` that reads rows from `database`.
fn query_call(database: &str, sql: &str) -> String {
    format!("psql {PG_CONNECTION} -d {database} -t -A -c {sql}")
}

/// A call of `psql` that runs a command in `database`.
fn command_call(database: &str, sql: &str) -> String {
    format!("psql {PG_CONNECTION} -d {database} -c {sql}")
}

/// A path as the text of an environment variable.
fn text(path: &std::path::Path) -> &str {
    path.to_str().expect("a temp path is text")
}

/// A stand-in for `pg_dump` that writes a small archive to the file of `-f`.
const PG_DUMP_WRITES: &str = r#"while [ "$#" -gt 1 ]; do
    [ "$1" = -f ] && printf 'PGDMP' > "$2"
    shift
done"#;

/// The call of `pg_dump` that backs up `app` to a file beside `backup`.
fn dump_call(backup: &std::path::Path) -> String {
    format!(
        "pg_dump {PG_CONNECTION} -d app -Fc -f {}.partial",
        backup.display()
    )
}

#[test]
fn postgres_backup_writes_the_dump_to_the_path_and_prints_its_size_and_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/pg_dump", PG_DUMP_WRITES);
    let backup = stubs.path("app.dump");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_BACKUP,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stubs.calls(), [dump_call(&backup), NOW_CALL.to_string()]);
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "PGDMP");
    assert!(!stubs.path("app.dump.partial").exists());
    assert_eq!(
        stdout,
        format!(
            "{{\"database\":\"app\",\"path\":\"{}\",\"size_bytes\":5,\"finished_at\":{NOW}}}\n",
            backup.display()
        )
    );
}

#[test]
fn postgres_backup_that_fails_keeps_the_older_backup_and_prints_no_time() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/pg_dump",
        &format!(
            "{PG_DUMP_WRITES}\necho 'pg_dump: error: connection to server at \"localhost\" (::1), port 5432 failed: Connection refused' >&2\nexit 1"
        ),
    );
    let backup = stubs.path("app.dump");
    std::fs::write(&backup, "OLDER").unwrap();

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_BACKUP,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert_eq!(stubs.calls(), [dump_call(&backup)]);
    assert_eq!(code, 1, "the status of pg_dump: {stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("port 5432 failed"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "OLDER");
    assert!(!stubs.path("app.dump.partial").exists());
}

#[test]
fn postgres_backup_refuses_a_path_with_a_quote_and_runs_no_tool() {
    let stubs = pg_stubs();

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_BACKUP,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", "/backups/app\".dump"),
        ],
    );

    assert!(stubs.calls().is_empty(), "{:?}", stubs.calls());
    assert_ne!(code, 0, "{stdout}");
    assert!(stderr.contains("TUMULT_BACKUP_PATH"), "{stderr}");
}

/// The calls of a restore of `backup` into `app`, before the time.
fn restore_calls(backup: &std::path::Path) -> Vec<String> {
    vec![
        format!("pg_restore -l {}", backup.display()),
        command_call("postgres", "DROP DATABASE IF EXISTS \"app\" WITH (FORCE);"),
        command_call("postgres", "CREATE DATABASE \"app\" TEMPLATE template0;"),
        format!(
            "pg_restore {PG_CONNECTION} -d app --single-transaction {}",
            backup.display()
        ),
    ]
}

#[test]
fn postgres_restore_makes_the_database_again_from_the_backup_and_prints_its_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "exit 0");
    stubs.tool("bin/pg_restore", "exit 0");
    let backup = stubs.path("app.dump");
    std::fs::write(&backup, "PGDMP").unwrap();

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_RESTORE,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    let mut expected = restore_calls(&backup);
    expected.push(NOW_CALL.to_string());
    assert_eq!(stubs.calls(), expected);
    assert_eq!(
        stdout,
        format!(
            "{{\"database\":\"app\",\"path\":\"{}\",\"finished_at\":{NOW}}}\n",
            backup.display()
        )
    );
}

#[test]
fn postgres_restore_drops_nothing_when_there_is_no_backup() {
    let stubs = pg_stubs();
    let backup = stubs.path("app.dump");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_RESTORE,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert!(stubs.calls().is_empty(), "{:?}", stubs.calls());
    assert_ne!(code, 0, "{stdout}");
    assert!(stderr.contains("no backup at"), "{stderr}");
}

#[test]
fn postgres_restore_drops_nothing_when_the_file_is_no_archive() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/pg_restore",
        "echo 'pg_restore: error: input file appears to be a text format dump. Please use psql.' >&2\nexit 1",
    );
    let backup = stubs.path("app.sql");
    std::fs::write(&backup, "CREATE TABLE orders ();").unwrap();

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_RESTORE,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert_eq!(
        stubs.calls(),
        [format!("pg_restore -l {}", backup.display())]
    );
    assert_ne!(code, 0, "{stdout}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("text format dump"), "{stderr}");
}

#[test]
fn postgres_restore_that_fails_prints_no_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "exit 0");
    stubs.tool(
        "bin/pg_restore",
        r#"[ "$1" = -l ] && exit 0
echo 'pg_restore: error: could not execute query: ERROR:  role "shop" does not exist' >&2
exit 1"#,
    );
    let backup = stubs.path("app.dump");
    std::fs::write(&backup, "PGDMP").unwrap();

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_RESTORE,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
        ],
    );

    assert_eq!(stubs.calls(), restore_calls(&backup));
    assert_eq!(code, 1, "the status of pg_restore: {stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("role \"shop\" does not exist"), "{stderr}");
}

const IN_RECOVERY: &str = "SELECT pg_is_in_recovery();";

#[test]
fn postgres_stop_primary_stops_the_container_of_a_primary_and_prints_the_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "echo f");
    stubs.tool("bin/docker", "echo pg-1");

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_STOP_PRIMARY, &[("TUMULT_CONTAINER_ID", "pg-1")]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            query_call("postgres", IN_RECOVERY),
            "docker stop --time 10 pg-1".to_string(),
            NOW_CALL.to_string()
        ]
    );
    assert_eq!(
        stdout,
        format!("{{\"container\":\"pg-1\",\"mode\":\"fast\",\"stopped_at\":{NOW}}}\n")
    );
}

#[test]
fn postgres_stop_primary_kills_the_container_in_kill_mode() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "echo f");
    stubs.tool("bin/docker", "echo pg-1");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_STOP_PRIMARY,
        &[
            ("TUMULT_CONTAINER_ID", "pg-1"),
            ("TUMULT_STOP_MODE", "kill"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            query_call("postgres", IN_RECOVERY),
            "docker kill --signal KILL pg-1".to_string(),
            NOW_CALL.to_string()
        ]
    );
    assert_eq!(
        stdout,
        format!("{{\"container\":\"pg-1\",\"mode\":\"kill\",\"stopped_at\":{NOW}}}\n")
    );
}

#[test]
fn postgres_stop_primary_refuses_to_stop_a_standby() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "echo t");

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_STOP_PRIMARY, &[("TUMULT_CONTAINER_ID", "pg-1")]);

    assert_eq!(stubs.calls(), [query_call("postgres", IN_RECOVERY)]);
    assert_ne!(code, 0, "{stdout}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("is a standby"), "{stderr}");
}

#[test]
fn postgres_stop_primary_stops_nothing_when_the_primary_does_not_answer() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/psql",
        r#"echo 'psql: error: connection to server at "localhost" (::1), port 5432 failed: Connection refused' >&2
exit 2"#,
    );

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_STOP_PRIMARY, &[("TUMULT_CONTAINER_ID", "pg-1")]);

    assert_eq!(stubs.calls(), [query_call("postgres", IN_RECOVERY)]);
    assert_eq!(code, 2, "the status of psql: {stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("Connection refused"), "{stderr}");
}

#[test]
fn postgres_start_primary_starts_the_container_again_and_prints_the_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/docker", "echo pg-1");

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_START_PRIMARY, &[("TUMULT_CONTAINER_ID", "pg-1")]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        ["docker start pg-1".to_string(), NOW_CALL.to_string()]
    );
    assert_eq!(
        stdout,
        format!("{{\"container\":\"pg-1\",\"started_at\":{NOW}}}\n")
    );
}

#[test]
fn postgres_dr_scripts_print_whole_seconds_where_date_prints_no_nanoseconds() {
    // busybox date prints nothing for %N, BSD date prints the letter.
    for nanoseconds in ["1700000000.", "1700000000.N"] {
        let stubs = Stubs::new();
        stubs.tool(
            "bin/date",
            &format!(r#"[ "$1" = +%s ] && echo 1700000000 || echo {nanoseconds}"#),
        );
        stubs.tool("bin/docker", "echo pg-1");

        let (code, stdout, stderr) =
            stubs.run(POSTGRES_START_PRIMARY, &[("TUMULT_CONTAINER_ID", "pg-1")]);

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stdout, "{\"container\":\"pg-1\",\"started_at\":1700000000}\n",
            "date said {nanoseconds}"
        );
    }
}

const PROMOTE: &str = "SELECT pg_promote(true, 60);";
const FIRST_WRITE: &str = "SELECT txid_current(), extract(epoch FROM clock_timestamp());";

#[test]
fn postgres_promote_replica_prints_the_time_the_standby_took_its_first_write() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/psql",
        r#"case "$*" in
    *pg_promote*) echo t ;;
    *txid_current*) echo '748|1763869012.5' ;;
esac"#,
    );

    let (code, stdout, stderr) = stubs.run(POSTGRES_PROMOTE_REPLICA, &[]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            query_call("postgres", PROMOTE),
            query_call("postgres", FIRST_WRITE)
        ]
    );
    assert_eq!(stdout, "{\"writable_at\":1763869012.5}\n");
}

#[test]
fn postgres_promote_replica_fails_when_the_promotion_does_not_end_in_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", "echo f");

    let (code, stdout, stderr) = stubs.run(POSTGRES_PROMOTE_REPLICA, &[]);

    assert_eq!(stubs.calls(), [query_call("postgres", PROMOTE)]);
    assert_ne!(code, 0, "{stdout}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("within 60s"), "{stderr}");
}

#[test]
fn postgres_promote_replica_fails_with_the_message_of_psql_on_a_server_that_is_no_standby() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/psql",
        "echo 'ERROR:  recovery is not in progress' >&2\nexit 1",
    );

    let (code, stdout, stderr) = stubs.run(POSTGRES_PROMOTE_REPLICA, &[]);

    assert_eq!(stubs.calls(), [query_call("postgres", PROMOTE)]);
    assert_eq!(code, 1, "the status of psql: {stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("recovery is not in progress"), "{stderr}");
}

/// The writer makes its table once. The server says nothing when the table
/// is there already: a notice on stderr would read as an error of the step.
const CREATE_MARKERS: &str = "SET client_min_messages = warning; CREATE TABLE IF NOT EXISTS tumult_markers (written_at double precision NOT NULL);";
const INSERT_MARKER: &str = "WITH marker AS (INSERT INTO tumult_markers (written_at) VALUES (extract(epoch FROM clock_timestamp())) RETURNING written_at) SELECT written_at FROM marker;";

/// A stand-in for `psql` that makes the table and answers the n-th marker
/// with the time `100.n`. The marker at `fail_at` fails, as a server that
/// goes away does. At 0 no marker fails.
fn marker_psql(stubs: &Stubs, fail_at: u32) -> String {
    format!(
        r#"case "$*" in
    *CREATE*) echo 'CREATE TABLE' ;;
    *INSERT*)
        n=$(cat '{count}' 2>/dev/null || echo 0)
        n=$((n + 1))
        echo "$n" > '{count}'
        if [ "$n" -eq {fail_at} ]; then
            echo 'FATAL:  terminating connection due to administrator command' >&2
            exit 2
        fi
        echo "100.$n" ;;
esac"#,
        count = stubs.path("inserts").display()
    )
}

#[test]
fn postgres_write_markers_adds_each_marker_the_store_confirmed_to_the_ledger() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &marker_psql(&stubs, 0));
    stubs.tool("bin/sleep", "exit 0");
    let ledger = stubs.path("ledger");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_WRITE_MARKERS,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_LEDGER_PATH", text(&ledger)),
            ("TUMULT_MARKER_COUNT", "3"),
            ("TUMULT_INTERVAL_MS", "500"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    let insert = query_call("app", INSERT_MARKER);
    assert_eq!(
        stubs.calls(),
        [
            command_call("app", CREATE_MARKERS),
            NOW_CALL.to_string(),
            insert.clone(),
            NOW_CALL.to_string(),
            "sleep 0.500".to_string(),
            insert.clone(),
            NOW_CALL.to_string(),
            "sleep 0.500".to_string(),
            insert,
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&ledger).unwrap(),
        "100.1\n100.2\n100.3\n"
    );
    assert_eq!(
        stdout,
        "{\"database\":\"app\",\"markers\":3,\"first_at\":100.1,\"last_at\":100.3}\n"
    );
}

#[test]
fn postgres_write_markers_stops_when_its_time_is_up() {
    let stubs = Stubs::new();
    // The clock reads 100.0 at the start, then 100.6, then 101.2.
    stubs.tool(
        "bin/date",
        &format!(
            r#"n=$(cat '{reads}' 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > '{reads}'
case "$n" in
    1) echo 100.000000000 ;;
    2) echo 100.600000000 ;;
    *) echo 101.200000000 ;;
esac"#,
            reads = stubs.path("clock-reads").display()
        ),
    );
    stubs.tool("bin/psql", &marker_psql(&stubs, 0));
    stubs.tool("bin/sleep", "exit 0");
    let ledger = stubs.path("ledger");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_WRITE_MARKERS,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_LEDGER_PATH", text(&ledger)),
            ("TUMULT_DURATION", "1"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs
            .calls()
            .iter()
            .filter(|c| c.starts_with("sleep"))
            .count(),
        1,
        "{:?}",
        stubs.calls()
    );
    assert_eq!(std::fs::read_to_string(&ledger).unwrap(), "100.1\n100.2\n");
    assert_eq!(
        stdout,
        "{\"database\":\"app\",\"markers\":2,\"first_at\":100.1,\"last_at\":100.2}\n"
    );
}

#[test]
fn postgres_write_markers_that_fails_keeps_the_ledger_of_the_confirmed_markers() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &marker_psql(&stubs, 3));
    stubs.tool("bin/sleep", "exit 0");
    let ledger = stubs.path("ledger");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_WRITE_MARKERS,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_LEDGER_PATH", text(&ledger)),
            ("TUMULT_MARKER_COUNT", "5"),
        ],
    );

    assert_eq!(code, 2, "the status of psql: {stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("terminating connection"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&ledger).unwrap(), "100.1\n100.2\n");
}

#[test]
fn postgres_write_markers_writes_nothing_when_it_cannot_write_the_ledger() {
    let stubs = pg_stubs();
    let ledger = stubs.path("missing/ledger");

    let (code, stdout, stderr) = stubs.run(
        POSTGRES_WRITE_MARKERS,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_LEDGER_PATH", text(&ledger)),
        ],
    );

    assert!(stubs.calls().is_empty(), "{:?}", stubs.calls());
    assert_ne!(code, 0, "{stdout}");
    assert!(stderr.contains("cannot write the ledger"), "{stderr}");
}

const MARKERS_TABLE: &str = "SELECT to_regclass('tumult_markers') IS NOT NULL;";
const NEWEST_MARKER: &str = "SELECT max(written_at) FROM tumult_markers;";

/// A stand-in for `psql` that says whether the table is there and answers
/// `newest` as the newest marker.
fn markers_psql(table: &str, newest: &str) -> String {
    format!(
        r#"case "$*" in
    *to_regclass*) echo {table} ;;
    *max*) echo '{newest}' ;;
esac"#
    )
}

/// Run the reader with a ledger that holds `ledger`, or no ledger.
fn markers_lost(stubs: &Stubs, ledger: Option<&str>) -> (i32, String, String) {
    let path = stubs.path("ledger");
    if let Some(times) = ledger {
        std::fs::write(&path, times).unwrap();
    }
    stubs.run(
        POSTGRES_MARKERS_LOST,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_LEDGER_PATH", text(&path)),
        ],
    )
}

#[test]
fn postgres_markers_lost_prints_zero_when_the_newest_marker_survived() {
    // A marker newer than the ledger was committed, but its writer stopped
    // before it could add it to the ledger.
    for newest in ["105.5", "106"] {
        let stubs = pg_stubs();
        stubs.tool("bin/psql", &markers_psql("t", newest));

        let (code, stdout, stderr) = markers_lost(&stubs, Some("100\n105.5\n"));

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stubs.calls(),
            [
                query_call("app", MARKERS_TABLE),
                query_call("app", NEWEST_MARKER)
            ]
        );
        assert_eq!(stdout, "0\n", "newest marker {newest}");
    }
}

#[test]
fn postgres_markers_lost_prints_the_newest_ledger_time_minus_the_newest_marker() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &markers_psql("t", "103.25"));

    let (code, stdout, stderr) = markers_lost(&stubs, Some("100\n103.25\n104\n105.5\n"));

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "2.25\n");
}

#[test]
fn postgres_markers_lost_prints_the_whole_ledger_when_no_marker_is_left() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &markers_psql("t", ""));

    let (code, stdout, stderr) = markers_lost(&stubs, Some("100\n103.25\n105.5\n"));

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "5.5\n");
}

#[test]
fn postgres_markers_lost_prints_the_whole_ledger_when_the_table_is_gone() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &markers_psql("f", "unused"));

    let (code, stdout, stderr) = markers_lost(&stubs, Some("100\n103.25\n105.5\n"));

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stubs.calls(), [query_call("app", MARKERS_TABLE)]);
    assert_eq!(stdout, "5.5\n");
}

#[test]
fn postgres_markers_lost_prints_zero_before_a_marker_was_written_and_still_reads_the_store() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &markers_psql("f", "unused"));

    let (code, stdout, stderr) = markers_lost(&stubs, None);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stubs.calls(), [query_call("app", MARKERS_TABLE)]);
    assert_eq!(stdout, "0\n");
}

#[test]
fn postgres_markers_lost_fails_with_the_message_of_psql_when_the_store_does_not_answer() {
    let stubs = pg_stubs();
    stubs.tool(
        "bin/psql",
        r#"echo 'psql: error: connection to server at "localhost" (::1), port 5432 failed: Connection refused' >&2
exit 2"#,
    );

    let run = markers_lost(&stubs, Some("100\n105.5\n"));

    assert_eq!(stubs.calls(), [query_call("app", MARKERS_TABLE)]);
    assert_eq!(run.0, 2, "the status of psql: {}", run.2);
    assert_failed_with(&run, "port 5432 failed: Connection refused");
}

#[test]
fn postgres_markers_lost_fails_on_a_ledger_line_that_is_no_time() {
    let stubs = pg_stubs();
    stubs.tool("bin/psql", &markers_psql("t", "100"));

    let run = markers_lost(&stubs, Some("100\nINSERT 0 1\n"));

    assert_ne!(run.0, 0, "{}", run.1);
    assert_failed_with(&run, "INSERT 0 1");
}

#[test]
fn postgres_dr_scripts_give_the_password_in_a_file_that_goes_when_they_end() {
    let stubs = pg_stubs();
    let seen = stubs.path("pgpass-seen");
    let file = stubs.path("pgpass-path");
    let env = stubs.path("password-env");
    stubs.tool(
        "bin/pg_dump",
        &format!(
            r#"cp "$PGPASSFILE" '{seen}'
printf '%s' "$PGPASSFILE" > '{file}'
printf '%s' "${{TUMULT_PG_PASSWORD-unset}}" > '{env}'
{PG_DUMP_WRITES}"#,
            seen = seen.display(),
            file = file.display(),
            env = env.display()
        ),
    );
    let backup = stubs.path("app.dump");

    let (code, _, stderr) = stubs.run(
        POSTGRES_BACKUP,
        &[
            ("TUMULT_PG_DATABASE", "app"),
            ("TUMULT_BACKUP_PATH", text(&backup)),
            ("TUMULT_PG_PASSWORD", r"pa:ss\word"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        "*:*:*:*:pa\\:ss\\\\word\n",
        "a ':' and a '\\' are escaped as the .pgpass format asks"
    );
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "unset");
    assert!(
        stubs.calls().iter().all(|call| !call.contains("word")),
        "{:?}",
        stubs.calls()
    );
    let used = std::fs::read_to_string(&file).unwrap();
    assert!(
        !std::path::Path::new(&used).exists(),
        "{used} is still there"
    );
}

#[test]
fn every_function_of_the_postgres_manifest_names_an_executable_script() {
    let manifest = std::fs::read_to_string(plugin_path("tumult-db-postgres/plugin.toon")).unwrap();
    let scripts: Vec<&str> = manifest
        .lines()
        .filter_map(|line| line.trim().strip_prefix("script: "))
        .collect();
    for name in [
        "actions/backup.sh",
        "actions/restore.sh",
        "actions/stop-primary.sh",
        "actions/start-primary.sh",
        "actions/promote-replica.sh",
        "actions/write-markers.sh",
        "probes/markers-lost.sh",
    ] {
        assert!(scripts.contains(&name), "{name} is not in the manifest");
    }
    for script in scripts {
        assert_script_executable(&format!("tumult-db-postgres/{script}"));
    }
}

/// The reader of the RPO as a steady-state probe, as the docs show it, with
/// its ledger in the lab.
const MARKERS_LOST_CHECK: &str = "    - name: data-lost
      activity_type: probe
      provider:
        type: script
        plugin: tumult-db-postgres
        function: markers-lost
        arguments:
          pg_database: app
          ledger_path: ledger
      tolerance:
        type: range
        from: 0
        to: 3600
";

#[test]
fn a_markers_lost_check_through_tumult_reads_the_seconds_a_restore_lost() {
    let lab = Lab::with_state("ok");
    std::fs::write(lab.path("ledger"), "100\n103.25\n105.5\n").unwrap();
    let stubs = Stubs::new();
    stubs.tool("bin/psql", &markers_psql("t", "103.25"));
    let path = format!(
        "{}:{}",
        stubs.path("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = lab
        .run_command(&guarded_by(&lab, MARKERS_LOST_CHECK))
        .env("PATH", path)
        .env("TMPDIR", lab.path(""))
        .output()
        .expect("run tumult");

    assert!(
        stubs.calls().contains(&query_call("app", NEWEST_MARKER)),
        "{:?}",
        stubs.calls()
    );
    let journal = lab.journal(&output);
    assert_eq!(
        journal.status,
        ExperimentStatus::Completed,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let before = journal.steady_state_before.expect("the check ran");
    let lost = result(&before.probe_results, "data-lost");
    assert_eq!(lost.output.as_deref(), Some("2.25"));
}

// ── tumult-network ────────────────────────────────────────────

const ADD_LATENCY: &str = "tumult-network/actions/add-latency.sh";
const PING_LATENCY: &str = "tumult-network/probes/ping-latency.sh";

/// Stand-ins for `uname`, which names `os`, and for `tool`, which runs
/// `answer`.
fn stubs_on(os: &str, tool: &str, answer: &str) -> Stubs {
    let stubs = Stubs::new();
    stubs.tool("bin/uname", &format!("echo {os}"));
    stubs.tool(&format!("bin/{tool}"), answer);
    stubs
}

#[test]
fn add_latency_delays_the_whole_interface_when_no_target_ip_is_given() {
    let stubs = stubs_on("Linux", "tc", "exit 0");

    let (code, stdout, stderr) = stubs.run(ADD_LATENCY, &[]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            "uname -s",
            "tc qdisc add dev eth0 root netem delay 100ms 10ms"
        ]
    );
    assert_eq!(
        stdout,
        "adding 100ms (±10ms) latency on eth0\nlatency injection active\n"
    );
}

#[test]
fn add_latency_delays_only_the_traffic_to_the_target_ip() {
    let stubs = stubs_on("Linux", "tc", "exit 0");

    let (code, stdout, stderr) = stubs.run(
        ADD_LATENCY,
        &[
            ("TUMULT_TARGET_IP", "10.0.1.50"),
            ("TUMULT_INTERFACE", "ens3"),
            ("TUMULT_DELAY_MS", "250"),
            ("TUMULT_JITTER_MS", "0"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            "uname -s",
            "tc qdisc add dev ens3 root handle 1: prio",
            "tc qdisc add dev ens3 parent 1:3 handle 30: netem delay 250ms 0ms",
            "tc filter add dev ens3 parent 1:0 protocol ip u32 match ip dst 10.0.1.50/32 flowid 1:3",
        ]
    );
    assert_eq!(
        stdout,
        "adding 250ms (±0ms) latency to 10.0.1.50 on ens3\nlatency injection active\n"
    );
}

const PING_CALL: &str = "ping -c 3 -q db";

/// What `ping -c 3 -q` prints on Linux for a host that answers.
const LINUX_PING: &str = "cat <<'EOF'
PING db (10.0.1.50) 56(84) bytes of data.

--- db ping statistics ---
3 packets transmitted, 3 received, 0% packet loss, time 2003ms
rtt min/avg/max/mdev = 0.045/0.052/0.061/0.007 ms
EOF";

/// What `ping -c 3 -q` prints on macOS for a host that answers.
const DARWIN_PING: &str = "cat <<'EOF'
PING db (10.0.1.50): 56 data bytes

--- db ping statistics ---
3 packets transmitted, 3 packets received, 0.0% packet loss
round-trip min/avg/max/stddev = 0.045/0.052/0.061/0.007 ms
EOF";

/// The line of the summary of Linux `ping` for a host that does not answer.
const NO_REPLY: &str = "3 packets transmitted, 0 received, 100% packet loss, time 2052ms";

#[test]
fn ping_latency_probe_reads_the_average_round_trip_on_linux() {
    let stubs = stubs_on("Linux", "ping", LINUX_PING);

    let (code, stdout, stderr) = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0.052\n");
}

#[test]
fn ping_latency_probe_fails_with_the_message_of_ping_when_the_host_does_not_answer() {
    // Linux ping writes its summary on stdout, also for a host that is down.
    let stubs = stubs_on(
        "Linux",
        "ping",
        &format!(
            "printf 'PING db (10.0.1.50) 56(84) bytes of data.\\n\\n--- db ping statistics ---\\n%s\\n' '{NO_REPLY}'\nexit 1"
        ),
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(run.0, 1, "the status of ping: {}", run.2);
    assert_failed_with(&run, NO_REPLY);
}

#[test]
fn ping_latency_probe_fails_when_ping_prints_no_round_trip_time_on_linux() {
    let stubs = stubs_on(
        "Linux",
        "ping",
        "echo 'PING db (10.0.1.50) 56(84) bytes of data.'",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_failed_with(&run, "PING db (10.0.1.50) 56(84) bytes of data.");
}

#[test]
fn ping_latency_probe_reads_the_average_round_trip_on_macos() {
    let stubs = stubs_on("Darwin", "ping", DARWIN_PING);

    let (code, stdout, stderr) = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0.052\n");
}

#[test]
fn ping_latency_probe_fails_with_the_message_of_ping_when_the_name_is_unknown_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "ping",
        "echo 'ping: cannot resolve db: Unknown host' >&2\nexit 68",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(run.0, 68, "the status of ping: {}", run.2);
    assert_failed_with(&run, "ping: cannot resolve db: Unknown host");
}

#[test]
fn ping_latency_probe_fails_when_ping_prints_no_round_trip_time_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "ping",
        "echo 'PING db (10.0.1.50): 56 data bytes'",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_failed_with(&run, "PING db (10.0.1.50): 56 data bytes");
}

// ── tumult-pumba ──────────────────────────────────────────────

/// The Pumba image the actions run when no `pumba` is on `PATH`, pinned
/// by tag and digest.
const PUMBA_IMAGE: &str = "ghcr.io/alexei-led/pumba:1.6.0@sha256:11ed15538c7ec688628e2f53917d949e8c16418c146742731c3030ef276b6057";

/// The stress-ng image the stress action asks Pumba to start, pinned by
/// tag and digest.
const STRESS_IMAGE: &str = "ghcr.io/alexei-led/stress-ng:0.20.01@sha256:6f5de54261666970ce98abb2d710b6337c44575cd31c5886c63745323a7c12a2";

/// The name of every action of the plugin. Each one runs Pumba.
fn pumba_actions() -> Vec<String> {
    let dir = plugin_path("tumult-pumba/actions");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {dir}: {e}"))
        .map(|entry| entry.expect("directory entry").path())
        .filter_map(|path| Some(path.file_name()?.to_str()?.strip_suffix(".sh")?.to_string()))
        .collect();
    names.sort();
    names
}

/// How every action starts Pumba in a container on the default socket.
const PUMBA_RUN: &str = "run --rm -v /var/run/docker.sock:/var/run/docker.sock -e DOCKER_HOST=unix:///var/run/docker.sock";

/// Stand-ins with a `docker` that writes each of its arguments on a line
/// of `argv`.
fn docker_stubs() -> Stubs {
    let stubs = Stubs::new();
    let argv = stubs.path("argv");
    stubs.tool(
        "bin/docker",
        &format!(r#"printf '%s\n' "$@" > '{}'"#, argv.display()),
    );
    stubs
}

/// Run a Pumba action on the container `app` with no `pumba` on `PATH`,
/// and return the arguments it gave `docker`.
fn docker_argv_of(stubs: &Stubs, action: &str, env: &[(&str, &str)]) -> Vec<String> {
    let script = format!("tumult-pumba/actions/{action}.sh");
    let mut all = vec![
        ("TUMULT_CONTAINER", "app"),
        ("DOCKER_HOST", "unix:///var/run/docker.sock"),
    ];
    all.extend_from_slice(env);
    let (code, _, stderr) = stubs.run_alone(&script, &all);
    assert_eq!(code, 0, "{action}: {stderr}");
    std::fs::read_to_string(stubs.path("argv"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn every_pumba_action_runs_the_pinned_pumba_image() {
    let actions = pumba_actions();
    assert!(!actions.is_empty(), "the plugin has no actions");
    for action in &actions {
        let stubs = docker_stubs();

        let argv = docker_argv_of(&stubs, action, &[]);

        let image_at = PUMBA_RUN.split(' ').count();
        assert_eq!(argv[..image_at].join(" "), PUMBA_RUN, "{action}");
        assert_eq!(argv[image_at], PUMBA_IMAGE, "{action}");
    }
}

#[test]
fn the_stress_action_asks_pumba_for_the_pinned_stress_ng_image() {
    let stubs = docker_stubs();

    let argv = docker_argv_of(&stubs, "stress-container", &[]);

    let flag = argv
        .iter()
        .position(|word| word == "--stress-image")
        .unwrap_or_else(|| panic!("no --stress-image in {argv:?}"));
    assert_eq!(argv[flag + 1], STRESS_IMAGE);
}

#[test]
fn the_stress_action_gives_pumba_its_stressors_as_one_argument() {
    let stubs = docker_stubs();

    let argv = docker_argv_of(
        &stubs,
        "stress-container",
        &[("TUMULT_STRESS_TYPE", "memory"), ("TUMULT_DURATION", "5s")],
    );

    let stressors = argv
        .iter()
        .position(|word| word.starts_with("--stressors="))
        .unwrap_or_else(|| panic!("no --stressors in {argv:?}"));
    assert_eq!(
        argv[stressors..],
        ["--stressors=--vm 1 --vm-bytes 64M --timeout 5s", "app"]
    );
}

// ── tumult-stress ─────────────────────────────────────────────

const CPU_UTILIZATION: &str = "tumult-stress/probes/cpu-utilization.sh";
const TOP_CALL: &str = "top -l 1 -n 0";

#[test]
fn cpu_utilization_probe_reads_the_busy_share_from_top_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "cat <<'EOF'
Processes: 512 total, 3 running, 509 sleeping, 2390 threads
2026/11/19 12:00:00
Load Avg: 1.80, 1.86, 1.89
CPU usage: 5.26% user, 10.52% sys, 84.21% idle
SharedLibs: 371M resident, 61M data, 25M linkedit.
EOF",
    );

    let (code, stdout, stderr) = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "15.79\n");
}

#[test]
fn cpu_utilization_probe_fails_with_the_message_of_top_when_top_fails_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "echo 'top: sysctl: Operation not permitted' >&2\nexit 1",
    );

    let run = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_failed_with(&run, "top: sysctl: Operation not permitted");
}

#[test]
fn cpu_utilization_probe_fails_when_top_prints_no_cpu_usage_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "echo 'Processes: 512 total, 3 running, 509 sleeping, 2390 threads'",
    );

    let run = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_failed_with(&run, "Processes: 512 total");
}
