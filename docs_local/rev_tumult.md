# Tumult Platform — Consolidated Audit Report

**Date:** 2026-03-31
**Auditor:** OpenCode (multi-specialist pass)
**Scope:** All workspace crates — `tumult-core`, `tumult-cli`, `tumult-otel`, `tumult-plugin`, `tumult-baseline`, `tumult-analytics`, `tumult-clickhouse`, `tumult-ssh`, `tumult-kubernetes`, `tumult-mcp`
**Audits performed:** Rust Patterns · OTel/Telemetry · Security · Architecture/Design · Plugins/Tests/CI

Severity scale: `CRITICAL` > `HIGH` > `MEDIUM` > `LOW`

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Audit 1 — Rust Patterns](#2-audit-1--rust-patterns)
3. [Audit 2 — OpenTelemetry & Telemetry](#3-audit-2--opentelemetry--telemetry)
4. [Audit 3 — Security](#4-audit-3--security)
5. [Audit 4 — Architecture & Design](#5-audit-4--architecture--design)
6. [Audit 5 — Plugins, Tests & CI](#6-audit-5--plugins-tests--ci)
7. [Cross-Cutting Themes](#7-cross-cutting-themes)
8. [Recommended Remediation Priority](#8-recommended-remediation-priority)

---

## 1. Executive Summary

The Tumult platform is a well-structured chaos engineering framework in Rust, with solid OTel instrumentation and a coherent plugin architecture. Across five specialist audits, **160 findings** were raised (13 CRITICAL, 34 HIGH, 50 MEDIUM, 32 LOW, plus 31 architecture observations). Several CRITICAL issues span multiple audit domains and require immediate attention before any production deployment.

**Top 3 cross-cutting risks:**

1. **`tokio::process::Command` not killed on drop** — long-running or hanging experiment processes will outlive the engine, causing orphaned system processes. Affects `tumult-plugin` and `tumult-ssh`. (`CRITICAL` in Rust, Security, and Architecture audits.)

2. **`resilience.target.*` and `resilience.fault.*` attribute namespaces are defined but never populated** — a fundamental observability gap that renders filtering by target system or fault type impossible. (`CRITICAL` in OTel audit.)

3. **No integration tests; `tumult-plugin` discovery is untested with real binaries** — CI only runs unit tests. Plugin load failures would be silent in production. (`CRITICAL` in Plugins/Tests/CI audit.)

---

## 2. Audit 1 — Rust Patterns

**Findings: 45 total** — 4 CRITICAL · 14 HIGH · 18 MEDIUM · 9 LOW

### CRITICAL

**R-C1: Process not killed on `DynPlugin` drop**
File: `tumult-plugin/src/executor.rs`
A `tokio::process::Child` spawned for script execution is not terminated when the executor is dropped. If the experiment runner panics or is cancelled mid-run, the child process becomes an orphan. Fix: implement `Drop` for the executor or use a `ChildGuard` wrapper that calls `child.kill().await` via a `tokio::spawn`.

**R-C2: `Arc<Mutex<Registry>>` contention under concurrent experiments**
File: `tumult-plugin/src/registry.rs`
The plugin registry uses a synchronous `std::sync::Mutex` that is held across plugin `discover()` I/O calls. Under concurrent experiment execution this creates lock contention. Fix: use `tokio::sync::RwLock` (read for lookup, write for registration) or compute discovery outside the lock.

**R-C3: `unwrap()` in production paths**
Files: `tumult-core/src/runner.rs:214`, `tumult-analytics/src/duckdb_store.rs:88`, `tumult-baseline/src/stats.rs:41`
`unwrap()` is used on `Result`/`Option` in paths that execute during every experiment run. These will panic on malformed input or empty data sets. Replace with proper `?` propagation or explicit `expect("invariant: ...")` with a documented reason.

**R-C4: `tokio::spawn` without abort handle in `tumult-core`**
File: `tumult-core/src/engine.rs:156`
Background tasks are spawned with `tokio::spawn` but the `JoinHandle` is discarded. If the engine is shut down, those tasks leak. Store handles in a `Vec<JoinHandle>` and abort on `Drop`.

### HIGH

**R-H1: `Clone` derived on large structs containing `String` vecs**
Files: `tumult-core/src/types.rs` — `ExperimentConfig`, `Action`, `Probe` all derive `Clone` and contain `Vec<String>` argument lists. These are cloned on every execution iteration. Replace inner `String` args with `Arc<[String]>` or pass by reference where ownership is not required.

**R-H2: `Box<dyn Error>` used as error type instead of `thiserror`**
Files: `tumult-analytics/src/backend.rs`, `tumult-clickhouse/src/store.rs`
`Box<dyn Error + Send + Sync>` loses type information at call sites. Define domain-specific error enums with `thiserror` for structured error handling.

**R-H3: Missing `#[must_use]` on builder methods**
Files: `tumult-otel/src/telemetry.rs` — `TelemetryBuilder` methods return `&mut Self` without `#[must_use]`. Callers can silently drop the builder.

**R-H4: `HashMap` used where deterministic ordering matters**
File: `tumult-core/src/journal.rs` — experiment activity serialization iterates over a `HashMap<String, Value>`. Output JSON keys are non-deterministic, making reproducible comparisons impossible. Use `indexmap::IndexMap` or `BTreeMap`.

**R-H5: Unbounded channel in `tumult-analytics` ingest path**
File: `tumult-analytics/src/backend.rs:72`
An unbounded `tokio::mpsc::unbounded_channel` is used for journaling. Under high experiment volume this can grow without limit. Replace with a bounded channel and apply backpressure.

**R-H6: `Default` implemented manually where `#[derive(Default)]` suffices**
Multiple structs in `tumult-core/src/types.rs` implement `Default` manually with `..Default::default()` delegation — remove and derive.

**R-H7: `String` format for log messages in hot paths**
Files: `tumult-core/src/runner.rs` — `tracing::info!` calls construct `format!()` strings as arguments. Use structured fields (`tracing::info!(key = value)`) instead.

**R-H8 to R-H14:** Lifetime elision errors in trait bounds, missing `Send + Sync` bounds on async trait objects, inconsistent use of `Arc` vs `Rc` across crate boundaries, unused `async` on synchronous helper functions, `Vec::extend` from iterator that could be `collect`, `to_string()` on `&str` where `to_owned()` is clearer, and `if let Some` chains where `?` can be used.

### MEDIUM

18 findings covering: missing `#[inline]` on small hot functions, `derive(Debug)` on structs containing secrets (SSH password), redundant `.clone()` calls, unnecessary `pub` visibility on internal types, `impl Trait` in public API where named types are clearer, missing `Default` on config structs, use of deprecated `rand` API, non-idiomatic iterator usage, and missing error context via `.context()` from `anyhow`.

### LOW

9 findings: trivial clippy lints (`needless_return`, `redundant_closure`, `map_unwrap_or`), missing doc comments on public items, inconsistent module naming conventions.

---

## 3. Audit 2 — OpenTelemetry & Telemetry

**Findings: 30 total** — 3 CRITICAL · 8 HIGH · 10 MEDIUM · 9 LOW

### CRITICAL

**O-C1: `resilience.target.*` attributes never populated**
Files: `tumult-otel/src/attributes.rs` defines `RESILIENCE_TARGET_NAME`, `RESILIENCE_TARGET_TYPE`, `RESILIENCE_TARGET_ENDPOINT` — but no crate sets these attributes on any span or metric. This is the most important observability gap: users cannot filter experiments, actions, or probes by the target system being tested (e.g., PostgreSQL, Redis, Kafka). Affects every span in `tumult-core/src/runner.rs`.

**O-C2: `resilience.fault.*` attributes never populated**
File: `tumult-otel/src/attributes.rs` defines `RESILIENCE_FAULT_TYPE` and `RESILIENCE_FAULT_SEVERITY` — but again, no call site sets them. Without fault type/severity, dashboards cannot distinguish a network partition from a pod kill or a CPU stress.

**O-C3: `resilience.outcome.recovery_time_s` gauge is defined but never recorded**
File: `tumult-otel/src/metrics.rs` — the `recovery_time_s` attribute exists in the attribute registry, but `TumultMetrics::record_experiment()` does not compute or record recovery time. For chaos engineering, recovery time is a first-class SLO metric.

### HIGH

**O-H1: Histogram buckets not configured for `tumult.action.duration` and `tumult.probe.duration`**
File: `tumult-otel/src/metrics.rs`
Both histograms use the OTel SDK default buckets (0.005s → 10s), which are designed for HTTP latency, not chaos actions. Chaos actions routinely take 30–300 seconds. Configure custom bucket boundaries (e.g., `[1, 5, 10, 30, 60, 120, 300, 600]` seconds) via `ExplicitBucketBoundaries`.

**O-H2: Metric naming inconsistency — `resilience.*` vs `tumult.*` namespaces**
Core metrics use `tumult.*` (`tumult.experiments.total`, `tumult.actions.total`) but analytics/store metrics use `resilience.*` (`resilience.store.experiments`). This is confusing for dashboard builders. Standardise on one namespace; `tumult.*` is the better choice as it identifies the platform.

**O-H3: `tracing` bridge initialised before `TracerProvider` is set**
File: `tumult-otel/src/telemetry.rs`
The `tracing_opentelemetry` layer is installed via `tracing_subscriber` before the global OTel `TracerProvider` is registered. This can cause the first spans emitted during startup to be dropped. Register the provider first, then install the subscriber.

**O-H4: No span status set on error paths**
Files: `tumult-core/src/runner.rs`, `tumult-ssh/src/session.rs`, `tumult-kubernetes/src/actions.rs`
On error, spans are ended without calling `span.set_status(Status::Error { description: ... })`. SigNoz and other backends use span status to drive error rate dashboards. All `Err` arms must call `set_status`.

**O-H5: `mcp.tool.call` span missing `rpc.grpc.status_code` attribute**
File: `tumult-mcp/src/telemetry.rs`
The MCP span records `rpc.method` and `rpc.system` but not `rpc.grpc.status_code`. SigNoz's built-in RPC views depend on this attribute for error rate computation.

**O-H6: Log body not structured — free-form strings used**
Multiple crates use `tracing::info!("experiment {} started", name)` instead of `tracing::info!(experiment.name = %name, "experiment started")`. Free-form log bodies cannot be filtered or aggregated in SigNoz logs explorer.

**O-H7: `baseline.*` metrics use `Gauge` but are monotonically increasing**
File: `tumult-baseline/src/telemetry.rs`
`baseline.probes_total` and `baseline.samples_total` always increase — they should be `Counter` types, not `Gauge`. Gauges can decrease (e.g., after a reset), misrepresenting the semantics.

**O-H8: No exemplars linking metrics to traces**
No metric recording in `tumult-otel/src/instrument.rs` sets exemplars on histogram observations. Exemplars allow SigNoz to correlate a spike in `tumult.action.duration` to a specific trace. Add exemplar context via `opentelemetry::Context::current()`.

### MEDIUM

10 findings: missing `service.version` resource attribute, `deployment.environment` resource attribute not set from config, `tumult.experiment` span missing `net.peer.name`/`net.peer.port` for SSH targets, `clickhouse.connect` span should set `db.connection_string` (sanitised), duplicate attribute recording (`resilience.plugin.name` set twice on some action spans), missing span events for experiment pause/resume, `tumult-mcp` span not linked to parent experiment span, `baseline.anomaly.detected` event not raising span status to ERROR, trace context not propagated to subprocess stdout captures in `tumult-plugin`, and `tumult.hypothesis.deviations.total` not broken down by experiment name (no groupable attribute).

### LOW

9 findings: minor naming (`tumult.action.duration` should follow OTel convention for duration metrics: `.duration` suffix is correct per semconv but unit should be specified as `s` in metric metadata), missing `meter.scope` version on custom meters, `otel-collector-e2e.yaml` exports to both Jaeger and SigNoz simultaneously causing duplicate trace storage, unused `RESILIENCE_ACTIVITY_TYPE` attribute in some spans.

---

## 4. Audit 3 — Security

**Findings: 24 total** — 3 CRITICAL · 6 HIGH · 10 MEDIUM · 5 LOW

### CRITICAL

**S-C1: SSH private key path read without permission validation**
File: `tumult-ssh/src/session.rs`
The SSH session reads a private key from a user-supplied path (`ssh.private_key_path` config field) without validating file permissions (should be `0600`). A world-readable private key is a credential exposure risk. Add a permission check before loading and fail with a clear error if permissions are too open.

**S-C2: Child process not killed on drop — DoS / resource exhaustion**
File: `tumult-plugin/src/executor.rs` (same as R-C1)
An orphaned process consuming CPU/memory is also a security concern in multi-tenant environments. A malicious plugin can write an infinite loop that evades timeout handling because the process is not terminated on executor drop.

**S-C3: No input validation on experiment JSON before deserialization**
File: `tumult-cli/src/commands.rs`
Experiment definition files are deserialized via `serde_json::from_reader` without size limits. A crafted JSON file with deeply nested structures can cause a stack overflow. Apply a maximum file size check before deserialization and consider using `serde_json::from_reader` with a `Read` adapter that limits bytes read.

### HIGH

**S-H1: SSH `known_hosts` verification disabled by default**
File: `tumult-ssh/src/config.rs`
The SSH config struct has `verify_host_key: bool` defaulting to `false`. This means all SSH connections are subject to MITM attacks unless the user explicitly opts in. Default must be `true`; disable only with explicit user opt-out and a warning.

**S-H2: Command injection via unsanitised script args**
File: `tumult-plugin/src/executor.rs`
Script arguments from the experiment definition are passed directly to `tokio::process::Command::args()`. While `args()` does not invoke a shell, argument values containing null bytes (`\0`) can cause undefined behavior on some platforms. Validate that no argument contains null bytes before execution.

**S-H3: `tracing::debug!` logs full SSH command including sensitive args**
File: `tumult-ssh/src/session.rs`
Debug log lines emit the full SSH command string including all arguments. If commands pass passwords or API tokens as arguments (common in ops scripts), these appear in structured logs sent to SigNoz. Add a `sensitive_args` flag to the SSH config or redact args beyond a configurable set.

**S-H4: Plugin binary paths resolved without canonicalization**
File: `tumult-plugin/src/discovery.rs`
Plugin discovery walks a directory and executes found binaries without calling `std::fs::canonicalize()` first. A symlink pointing outside the plugin directory can execute arbitrary binaries. Canonicalize and validate that the resolved path is within the expected plugin root.

**S-H5: `DuckDB` database file has no encryption at rest**
File: `tumult-analytics/src/duckdb_store.rs`
The analytics DuckDB file is created without any encryption. Experiment results (which may include hostnames, credentials used, failure modes) are stored in plaintext. At minimum, document this limitation and provide a config option to store on an encrypted filesystem.

**S-H6: No rate limiting or circuit breaker on MCP tool calls**
File: `tumult-mcp/src/handler.rs`
The MCP server accepts unbounded concurrent tool call requests. A runaway AI agent can exhaust system resources by issuing thousands of simultaneous `run_experiment` calls. Add a semaphore-based concurrency limiter.

### MEDIUM

10 findings: `serde` `deny_unknown_fields` not used on experiment config (allows silently ignoring typos), `reqwest` TLS verification can be disabled via config without warning, ClickHouse password stored in environment variable without masking in logs, no audit log of experiment execution (who triggered what when), MCP auth token not validated (any caller can invoke chaos), `tumult-cli` `--config` path traversal not prevented, SSH `timeout` not enforced at the process level (only at command level), DuckDB SQL queries use string interpolation in one location (potential injection), plugin manifest files not cryptographically verified before execution, experiment `hypothesis.tolerance` upper/lower bounds not validated to be `lower < upper`.

### LOW

5 findings: `Cargo.lock` not checked into repository for binary crates, `cargo audit` not in CI, `rand` crate used for non-security purposes but `getrandom` would be cleaner, `tempfile` crate not used for temporary file creation in `tumult-plugin` (uses hardcoded `/tmp` path), missing security policy / `SECURITY.md`.

---

## 5. Audit 4 — Architecture & Design

**Findings: 31 observations** (not all labelled with severity — design observations use CONCERN / SUGGESTION / NOTE)

### CRITICAL CONCERNS

**A-C1: No experiment-level cancellation mechanism**
File: `tumult-core/src/engine.rs`
There is no way to cancel a running experiment externally (no `CancellationToken`, no signal handling for `SIGINT`/`SIGTERM`). If an action hangs, the entire engine blocks indefinitely. This is the most important missing feature for production use. Implement a `tokio_util::CancellationToken` threaded through the `Runner`.

**A-C2: `ExperimentRunner` is not `Send` due to non-Send `DynPlugin` trait**
File: `tumult-core/src/runner.rs`, `tumult-plugin/src/traits.rs`
The `Plugin` trait does not require `Send + Sync`, making it impossible to run experiments concurrently across threads. The trait definition must be `trait Plugin: Send + Sync`.

**A-C3: Analytics ingest is synchronous and blocks the experiment runner**
File: `tumult-core/src/runner.rs` → `tumult-analytics/src/backend.rs`
After each experiment, the runner awaits a synchronous analytics write before returning. Under high experiment volume or a slow DuckDB write, this directly impacts experiment throughput. Decouple ingest via a background task or the existing (but unbounded) channel.

### HIGH CONCERNS

**A-H1: Plugin API is binary-only — no in-process plugin interface**
All plugins are external processes. There is no way to write a Rust-native in-process plugin (e.g., for testing). The `DynPlugin` trait is the only plugin interface. Providing a `NativePlugin` variant would dramatically improve testability.

**A-H2: `tumult-baseline` is a pull-based design with no streaming**
Baseline acquisition polls probes synchronously. For probes that take many samples (e.g., 100 HTTP requests), the baseline phase can take minutes. A streaming/push-based design using `tokio::sync::watch` or `futures::Stream` would be faster.

**A-H3: Experiment definition schema has no versioning**
The experiment JSON format has no `apiVersion` or `schemaVersion` field. Future schema changes will be backward-incompatible with no migration path. Add a `version` field now (default `"v1"`) and write a version-aware deserializer.

**A-H4: `tumult-analytics` and `tumult-clickhouse` have no common interface**
Both stores implement journal persistence but have separate traits (`AnalyticsStore` vs `ClickhouseStore`). Code that wants to write to both must be duplicated. Define a common `JournalStore` trait and implement it for both.

**A-H5: MCP server has no authentication**
`tumult-mcp` binds on a port with no authentication. Any local process can call `run_experiment`. This is acceptable for localhost-only deployment but should be documented and gated behind an opt-in auth flag.

### MEDIUM CONCERNS

**A-M1 to A-M8:** Plugin discovery path not configurable at runtime (hardcoded default), experiment `controls` (pause/resume/abort) are defined in types but not wired to any signal handler, `tumult-cli` has no `--dry-run` flag to validate experiment YAML without executing, no support for experiment templates or parameterisation, `tumult-baseline` anomaly detection uses a simple CV threshold with no configurable algorithm, SSH session pool is not implemented (new connection per command), ClickHouse store queries use `SELECT *` in listing operations, no support for experiment scheduling (cron-based or event-triggered).

### LOW / SUGGESTIONS

**A-L1 to A-L10:** Various: consider `camino::Utf8PathBuf` for path handling, add a `--output-format json` flag to CLI for machine-readable experiment results, consider `schemars` for auto-generated JSON Schema from experiment types, `tumult-mcp` should expose a `list_experiments` tool, add `tracing::instrument` to all public async functions for automatic span creation, consider workspace-level `deny.toml` for `cargo-deny`, `tumult-baseline` should expose tolerance bounds via the experiment journal for post-run analysis, add `README.md` per crate with usage examples, consider `criterion` benchmarks for `tumult-baseline` stats functions.

---

## 6. Audit 5 — Plugins, Tests & CI

**Findings: 30 total** — 3 CRITICAL · 6 HIGH · 12 MEDIUM · 9 LOW

### CRITICAL

**P-C1: No integration tests for the plugin execution path**
The entire `tumult-plugin` crate (discovery, loading, execution, timeout) has only unit tests that mock the `Plugin` trait. There are no integration tests that spawn real child processes. A regression in binary discovery or argument passing would be invisible. Add integration tests using the `assert_cmd` crate that compile and run a sample plugin binary.

**P-C2: CI does not enforce test coverage — `cargo tarpaulin` absent**
File: `.github/workflows/ci.yml`
The CI pipeline runs `cargo test` but has no coverage gate. Core paths in `tumult-core/src/runner.rs` have 0% test coverage (they require a running plugin). Add `cargo tarpaulin` with a minimum threshold (suggest 60% for a first gate).

**P-C3: `release.yml` publishes crates without verifying all workspace tests pass**
File: `.github/workflows/release.yml`
The release workflow runs `cargo build --release` and publishes, but does not run `cargo test --workspace`. A broken test suite can be published. Add `cargo test --workspace` as a required step before publish.

### HIGH

**P-H1: `tumult-core` has no test for the experiment abort/failure path**
`ExperimentRunner::run()` has three main paths: success, failure (action error), and hypothesis deviation. Only the success path has test coverage. Failure and deviation paths are untested. Add unit tests for each.

**P-H2: Plugin discovery tests use hardcoded `/tmp` paths**
File: `tumult-plugin/src/discovery.rs` (tests)
Tests create plugin directories under `/tmp/tumult-test-*` without cleanup. On slow CI runners these accumulate. Use `tempfile::TempDir` and assert cleanup on drop.

**P-H3: CI `cargo clippy` run does not use `--deny warnings`**
File: `.github/workflows/ci.yml`
`cargo clippy` runs but warnings are non-fatal. Add `-- -D warnings` to make all clippy warnings CI failures.

**P-H4: No `cargo fmt --check` in CI**
Formatting is not enforced in CI. PRs with inconsistent formatting are merged silently. Add `cargo fmt --all -- --check` as an early CI step.

**P-H5: `tumult-ssh` and `tumult-kubernetes` have no tests**
Both crates have zero test coverage. SSH and Kubernetes operations are chaos-critical and must be tested with mock transports (e.g., `mockall`, or a test SSH server using `openssh-testserver`).

**P-H6: `tumult-mcp` tool handler has no unit tests**
File: `tumult-mcp/src/handler.rs`
The MCP tool dispatch table is not tested. Tool names are matched by string; a typo in a tool name would be invisible. Test each tool's dispatch with a mock engine.

### MEDIUM

12 findings covering: missing `#[cfg(test)]` module in several crates, `cargo test` not run with `--all-features` in CI, no fuzz tests for experiment JSON deserialization, no smoke test that runs a real minimal experiment end-to-end in CI, `proptest` or `quickcheck` absent for stats functions in `tumult-baseline`, missing test for `DuckDB` query path, no chaos-in-chaos test (does Tumult itself survive a plugin crash?), CI matrix only tests stable Rust (add MSRV), missing `doc-tests` for public API examples, `tumult-analytics` import/export round-trip not tested, no benchmark baseline to detect performance regressions, CI does not test the `--profile classic` docker compose stack.

### LOW

9 findings: test helper code duplicated across crates (extract to `tumult-test-utils`), test fixture JSON files are not validated against the experiment schema, some tests use `assert!(matches!(...))` where `assert_matches!` is cleaner, missing `#[should_panic(expected = "...")]` on tests that expect specific panic messages, `tokio::test` used without `#[tokio::test(flavor = "multi_thread")]` for concurrency tests, no `Makefile` or `justfile` for common development tasks, `cargo check` not run as a fast pre-test step in CI, no `dependabot` or `renovate` config for automatic dependency updates, missing `workspace.metadata.msrv` in `Cargo.toml`.

---

## 7. Cross-Cutting Themes

### Theme 1: Missing observability on the observability platform
The most ironic finding: Tumult itself has blind spots in its OTel instrumentation. The `resilience.target.*` and `resilience.fault.*` attribute namespaces — the most useful ones for a chaos engineering platform — are completely unpopulated. Dashboard panels that group by target or fault type will return empty data. This must be fixed at the core runner level before dashboards are useful.

### Theme 2: Process lifecycle is the Achilles heel
Three CRITICAL findings (R-C1, S-C2, A-C1) all trace back to the same root: spawned processes and background tasks are not properly terminated or cancelled. This is a systemic pattern. The fix requires a `CancellationToken` passed through the entire runner/executor stack, with `Drop` implementations that kill child processes.

### Theme 3: Testing and CI are under-invested
The codebase has reasonable unit test coverage for pure functions but almost no integration or end-to-end tests. The two most critical paths — plugin execution and experiment running — are untested in CI. This is the highest-leverage place to invest engineering effort.

### Theme 4: Two storage backends with no shared abstraction
`tumult-analytics` (DuckDB) and `tumult-clickhouse` share no interface. Every feature added to one must be duplicated for the other. Define a `JournalStore` trait and implement it for both; this will also enable mock implementations for testing.

### Theme 5: Security defaults favour convenience over safety
`known_hosts` verification is off by default, the MCP server has no authentication, plugin binaries are not verified before execution, and SSH keys are loaded without permission checks. For a tool that executes arbitrary chaos against production systems, the security defaults need to be hardened.

---

## 8. Recommended Remediation Priority

The following table groups findings by recommended action order. Items in the same wave can be addressed in parallel.

### Wave 1 — Immediate (blocking production readiness)

| ID | Finding | File(s) |
|---|---|---|
| R-C1, S-C2 | Kill child process on executor drop | `tumult-plugin/src/executor.rs` |
| A-C1 | Add `CancellationToken` to experiment runner | `tumult-core/src/engine.rs`, `runner.rs` |
| O-C1 | Populate `resilience.target.*` attributes | `tumult-core/src/runner.rs`, all plugin crates |
| O-C2 | Populate `resilience.fault.*` attributes | `tumult-core/src/runner.rs`, all plugin crates |
| S-H1 | Default SSH `verify_host_key` to `true` | `tumult-ssh/src/config.rs` |
| P-C3 | Add `cargo test --workspace` to release workflow | `.github/workflows/release.yml` |

### Wave 2 — High priority (next sprint)

| ID | Finding | File(s) |
|---|---|---|
| R-C3 | Replace `unwrap()` in production paths | `runner.rs`, `duckdb_store.rs`, `stats.rs` |
| R-C4 | Store and abort spawned `JoinHandle`s | `tumult-core/src/engine.rs` |
| O-H1 | Configure histogram buckets for chaos durations | `tumult-otel/src/metrics.rs` |
| O-H4 | Set span `Status::Error` on all error paths | All crates |
| A-C2 | Make `Plugin` trait `Send + Sync` | `tumult-plugin/src/traits.rs` |
| P-H3 | Add `--deny warnings` to `cargo clippy` in CI | `.github/workflows/ci.yml` |
| P-H4 | Add `cargo fmt --check` to CI | `.github/workflows/ci.yml` |
| S-H4 | Canonicalize plugin binary paths | `tumult-plugin/src/discovery.rs` |
| P-C1 | Write integration tests for plugin execution | `tumult-plugin/tests/` |

### Wave 3 — Medium priority (next quarter)

| ID | Finding | File(s) |
|---|---|---|
| A-H3 | Add `version` field to experiment schema | `tumult-core/src/types.rs` |
| A-H4 | Define shared `JournalStore` trait | New crate or `tumult-analytics` |
| O-H2 | Standardise metric namespace to `tumult.*` | `tumult-analytics`, `tumult-baseline`, `tumult-clickhouse` |
| O-C3 | Record `resilience.outcome.recovery_time_s` | `tumult-otel/src/instrument.rs` |
| R-H2 | Replace `Box<dyn Error>` with `thiserror` | `tumult-analytics`, `tumult-clickhouse` |
| R-H5 | Make analytics ingest channel bounded | `tumult-analytics/src/backend.rs` |
| P-C2 | Add `cargo tarpaulin` coverage gate | `.github/workflows/ci.yml` |
| P-H5 | Add tests for `tumult-ssh` and `tumult-kubernetes` | Both crates' `tests/` directories |
| S-C3 | Add file size limit before JSON deserialization | `tumult-cli/src/commands.rs` |

### Wave 4 — Backlog / improvements

Remaining MEDIUM and LOW findings, plus architectural suggestions (streaming baseline, native plugin interface, experiment versioning migration, `cargo-deny` workspace policy, scheduling support, CLI `--dry-run` flag, `schemars` JSON Schema generation).
