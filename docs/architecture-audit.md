# Tumult Architecture Audit Report

**Date:** 2026-03-31  
**Scope:** Full workspace — 9 crates, 5 ADRs, technical design spec  
**Auditor:** Automated Rust architecture audit  

---

## Summary

The Tumult codebase shows solid foundations: clean error types via `thiserror`, good telemetry instrumentation, a working analytics layer, and a well-structured plugin manifest system. However, several **critical gaps** exist between the ADR specifications and the actual implementation — notably around feature flags, background activity execution, and the five-phase model. A number of async correctness issues could cause runtime panics or subtle thread-pool stalls in production.

Findings are rated: `[CRITICAL]`, `[HIGH]`, `[MEDIUM]`, `[LOW]`, `[INFO]`.

---

## 1. Crate Boundary Violations

### 1.1 `tumult-core` depends on `tumult-baseline` `[HIGH]`

`tumult-core/Cargo.toml:11` lists `tumult-baseline` as a dependency. `tumult-core` is the platform's foundational crate and should not depend on higher-level analysis crates. This creates a circular conceptual dependency: `baseline` is defined in terms of `core` types (`ProbeSamples`, `AcquisitionResult`), yet `core` imports `baseline`.

The correct layering is:
```
tumult-baseline → tumult-core (baseline uses core types)
```
Not:
```
tumult-core → tumult-baseline (core uses baseline stats)
```

**Fix:** Move baseline-aware logic (the `derive_baseline` call in `runner.rs`) into a higher-level crate such as `tumult-cli` or a new `tumult-runner` crate that can depend on both.

### 1.2 `tumult-clickhouse` leaks `tumult-analytics` internals `[MEDIUM]`

`tumult-clickhouse/src/store.rs:10` imports `tumult_analytics::duckdb_store::StoreStats`. The `StoreStats` struct is defined inside `duckdb_store`, a storage-implementation detail of the DuckDB backend. `tumult-clickhouse` should not depend on an internal module of `tumult-analytics`; it should depend only on the `AnalyticsBackend` trait and any shared types defined in `tumult-analytics/src/lib.rs` or `backend.rs`.

**Fix:** Move `StoreStats` to `tumult-analytics/src/backend.rs` or re-export it from `tumult-analytics/src/lib.rs`.

### 1.3 `AnalyticsBackend` synchronous wrapper uses `block_on` inside async context `[CRITICAL]`

`tumult-clickhouse/src/store.rs:388–414` implements the synchronous `AnalyticsBackend` trait by calling `tokio::runtime::Handle::current().block_on(...)` on every method. If these methods are called from within a Tokio async task (e.g., from a `#[tokio::main]` context or any `.await` chain), this will **panic at runtime** with "Cannot start a runtime from within a runtime."

The same pattern exists in `tumult-cli/src/commands.rs:249–253` (`rt.block_on()` inside an async command handler).

**Fix:** Either make `AnalyticsBackend` async (using `async_trait`) or ensure synchronous impls are only called from `spawn_blocking`. The `ClickHouseStore` already has full `_async` variants — the sync wrappers are unnecessary if callers are already async.

---

## 2. Trait Design

### 2.1 `TumultPlugin` seal cannot enforce in-tree-only constraint across workspace `[HIGH]`

`tumult-plugin/src/traits.rs:44` seals `TumultPlugin` via `private::Sealed` where `private` is `pub(crate)`. This prevents *external* crates (outside the workspace) from implementing `TumultPlugin`, but workspace-internal crates — `tumult-ssh`, `tumult-kubernetes` — must implement `Sealed` themselves, meaning they each have their own `private` module with `impl Sealed for ...`. The seal therefore provides no protection against arbitrary workspace crates implementing the trait; it only prevents crates published separately.

ADR-004 states native plugins are "in-tree only" — the current mechanism does not enforce this at the type system level.

**Fix:** Document the limitation explicitly. If true enforcement is needed, move the seal impl to `tumult-plugin` and provide a registration macro.

### 2.2 `actions()` and `probes()` return owned `Vec` on every call `[MEDIUM]`

`tumult-plugin/src/traits.rs` defines `fn actions(&self) -> Vec<ActionDescriptor>` and `fn probes(&self) -> Vec<ProbeDescriptor>`. These are called repeatedly during plugin discovery and dispatch. Returning owned `Vec`s on each call requires allocation on every invocation.

**Fix:** Change to `fn actions(&self) -> &[ActionDescriptor]` (backed by a `Vec` field) or return a `Cow<[ActionDescriptor]>`. Since descriptors are static metadata, `fn actions() -> &'static [ActionDescriptor]` would be even better.

### 2.3 `TumultPlugin` has no execution method `[HIGH]`

The `TumultPlugin` trait describes plugin capabilities (`actions`, `probes`, `name`, `version`) but has no `execute_action` or `execute_probe` method. The `executor.rs` module dispatches to plugins via a separate mechanism. This means the trait alone cannot guarantee that a plugin can actually execute the capabilities it declares — it is purely a metadata contract.

**Fix:** Add `fn execute_action(&self, action: &str, params: &ActionParams) -> Result<ActionOutput, PluginError>` and `fn execute_probe(&self, probe: &str, params: &ProbeParams) -> Result<ProbeOutput, PluginError>` to the trait, or document clearly that the trait is intentionally metadata-only and reference the execution contract elsewhere.

### 2.4 `ActionOutput`/`ProbeOutput` are opaque type aliases `[LOW]`

`tumult-plugin/src/traits.rs:30–33` defines `ActionOutput` and `ProbeOutput` as type aliases for `PluginOutput`. This makes the API surface confusing — callers see `ActionOutput` and `ProbeOutput` as distinct types but they are identical at the type level. Newtypes would allow future divergence.

---

## 3. Type Design

### 3.1 No `#[non_exhaustive]` on public enums `[HIGH]`

Key public enums in `tumult-core/src/types.rs` have no `#[non_exhaustive]` attribute:
- `ExperimentStatus` (line ~85)
- `ActivityStatus` (line ~100)
- `ActivityType` (line ~115)
- `Provider` (line ~130)

Adding a new variant to any of these in a future release will be a **breaking change** for downstream code that matches on them (including the analytics layer, CLI, and any external tool). Since Tumult is intended to be extensible, these enums will likely grow.

**Fix:** Add `#[non_exhaustive]` to all public enums today.

### 3.2 `Experiment` and `Journal` lack builder patterns `[MEDIUM]`

`tumult-core/src/types.rs`: `Experiment` has 13 public fields (lines ~282–310) and `Journal` has 17 public fields (lines ~467–485). Both are constructed with struct literal syntax throughout the codebase. This is fragile — adding a new required field is a breaking change at every construction site. The test fixtures in `duckdb_store.rs:426–461` already demonstrate the pain: constructing `Journal` requires spelling out all 17 fields including `None` for every optional one.

**Fix:** Implement builders (or at minimum provide constructors with sensible defaults and `..Default::default()` support via `#[derive(Default)]`).

### 3.3 `Activity::default()` uses a hardcoded `echo` process `[MEDIUM]`

`tumult-core/src/types.rs:194–211` — the `Default` impl for `Activity` creates an activity with `process: Some(Process { path: "echo".into(), ... })`. This is a surprising default that will pass validation silently in tests but produce unexpected behaviour in production. `Default` should either be semantically empty or not be implemented at all.

### 3.4 `Provider` and `ExecutionTarget` encode infrastructure knowledge in `core` `[MEDIUM]`

`Provider::Ssh` and `Provider::Kubernetes` variants live in `tumult-core/src/types.rs`. This means adding a new provider (e.g. `Provider::Nomad`) requires modifying the core crate, even though `tumult-core` should be infrastructure-agnostic. The provider concept belongs in the plugin registry layer.

---

## 4. Feature Flag Design

### 4.1 No Cargo feature flags — ADR-004 violated `[CRITICAL]`

ADR-004 states: *"Native plugins MUST be enabled via Cargo feature flags."* No crate in the workspace defines `[features]` sections. `tumult-cli/Cargo.toml` hardwires `tumult-ssh`, `tumult-kubernetes`, `tumult-analytics`, and `tumult-clickhouse` as unconditional dependencies:

```toml
# tumult-cli/Cargo.toml (lines ~10–30, approximate)
tumult-ssh = { path = "../tumult-ssh" }
tumult-kubernetes = { path = "../tumult-kubernetes" }
tumult-analytics = { path = "../tumult-analytics" }
tumult-clickhouse = { path = "../tumult-clickhouse" }
```

This means every build of `tumult-cli` must compile all plugins, all analytics backends, and all their transitive dependencies (kube-rs, russh, duckdb, clickhouse, arrow, parquet). A minimal build is not possible.

**Fix:** Gate each plugin behind a Cargo feature in `tumult-cli`:
```toml
[features]
default = ["ssh", "kubernetes", "analytics"]
ssh = ["dep:tumult-ssh"]
kubernetes = ["dep:tumult-kubernetes"]
analytics = ["dep:tumult-analytics"]
clickhouse = ["dep:tumult-clickhouse"]
```

### 4.2 `tokio` feature resolution is inconsistent `[MEDIUM]`

The workspace `Cargo.toml` defines a `tokio-minimal` alias using `package = "tokio"` with only `rt` and `macros` features. However `tumult-ssh/Cargo.toml` and `tumult-kubernetes/Cargo.toml` both use `tokio = { workspace = true }` which resolves to the `tokio` package with whatever features the workspace `[workspace.dependencies]` entry provides. If the workspace entry includes `"full"`, all crates get `tokio::full` even if they only need `rt`. Verify the workspace tokio entry and ensure per-crate feature sets are correct.

### 4.3 `duckdb` version string is malformed `[LOW]`

`Cargo.toml:78` (workspace root) specifies `duckdb = "1.10501"`. This is not a valid semver string. The likely intended version is `"1.1.0"` or similar. While `cargo` may accept non-standard version strings, this will cause confusion and may break tooling.

---

## 5. Error Hierarchy

### 5.1 `RunnerError::EmptyMethod` duplicates `EngineError::EmptyMethod` `[MEDIUM]`

`tumult-core/src/runner.rs:29–31` defines `RunnerError::EmptyMethod` and `tumult-core/src/engine.rs:11–12` defines `EngineError::EmptyMethod`. The same validation is represented in two error types, causing two code paths for the same invariant. This will lead to inconsistent error messages and makes it unclear which layer is responsible for the check.

**Fix:** Consolidate validation in one layer. If the engine validates this, the runner should not need to. Prefer `EngineError` as the canonical source since the engine is the validator.

### 5.2 `JournalError::EncodeError` is used for decode failures `[HIGH]`

`tumult-core/src/journal.rs:10–15` defines `JournalError::EncodeError`. Line 32 uses this same variant when a decode operation fails — the variant name is semantically incorrect. A reader of the error will assume encoding failed when in fact deserialization failed.

**Fix:** Add `JournalError::DecodeError` for decode failures.

### 5.3 `ExecutionError` is unused — dead code `[LOW]`

`tumult-core/src/execution.rs:8–13` defines `ExecutionError` with `#[allow(dead_code)]`. A suppressed dead-code warning on an error type is a signal that the abstraction was defined but never wired up. Either use it or remove it to keep the error hierarchy clean.

### 5.4 `AnalyticsError` has no variant for ClickHouse-specific failures `[MEDIUM]`

`tumult-analytics/src/error.rs` defines `AnalyticsError`. The `tumult-clickhouse` crate uses `AnalyticsError::Io(std::io::Error::other(e.to_string()))` to wrap ClickHouse errors (e.g. `store.rs:164`). This loses the original error type and makes it impossible to distinguish ClickHouse network errors from local I/O errors programmatically.

**Fix:** Either add `AnalyticsError::ClickHouse(clickhouse::error::Error)` or define a separate `ClickHouseError` in `tumult-clickhouse` and have `ClickHouseStore` return that type instead.

---

## 6. Async Architecture

### 6.1 `std::thread::sleep` used in async runner `[CRITICAL]`

`tumult-core/src/runner.rs:367` and `runner.rs:412` use `std::thread::sleep` for `pause_before`/`pause_after` delays on activities. This blocks the OS thread that the Tokio executor is running on, stalling all tasks that share the thread. In a multi-activity experiment, this can cause head-of-line blocking across the entire runtime.

**Fix:** Replace with `tokio::time::sleep(Duration::from_millis(...)).await`.

### 6.2 `commands.rs` creates a nested Tokio runtime `[CRITICAL]`

`tumult-cli/src/commands.rs:249–253` constructs a new `tokio::runtime::Runtime` with `rt.block_on()` inside a function that is itself called from an async command handler. This will panic: *"Cannot start a runtime from within a runtime."*

**Fix:** The function should be `async` and use `.await` directly.

### 6.3 `ActivityExecutor` trait is synchronous `[HIGH]`

The `ActivityExecutor` trait (referenced from executor.rs) defines `fn execute` as a synchronous method, which forces blocking I/O inside the trait body. Process-based activities (`execute_process`) implement this with a busy-polling loop that calls `std::thread::sleep(50ms)` to check for timeout (commands.rs:113–128). This wastes CPU and blocks the thread.

**Fix:** Make `ActivityExecutor::execute` async (`async fn execute(...) -> Result<...>`), then use `tokio::process::Command` and `tokio::time::timeout` inside implementations.

### 6.4 `ClickHouseStore::AnalyticsBackend` sync wrappers via `block_on` `[CRITICAL]`

Already noted in §1.3. The `AnalyticsBackend` sync impl for `ClickHouseStore` calls `Handle::current().block_on(...)` from within an async context, which will panic. This is a separate occurrence from the `commands.rs` case.

---

## 7. Plugin System

### 7.1 Discovery path mismatch with ADR-004 `[HIGH]`

ADR-004 specifies the first plugin discovery path is `./tumult-plugins/`. The implementation in `tumult-plugin/src/discovery.rs:70` uses `./plugins/`. These are different directories. Any operator following the ADR to place plugins in `./tumult-plugins/` will see them silently ignored.

**Fix:** Update `discovery.rs` to use `./tumult-plugins/` or update ADR-004 to reflect the actual path, ensuring documentation and code agree.

### 7.2 `discover_all_plugins` fails fast on first bad directory `[MEDIUM]`

`tumult-plugin/src/discovery.rs:93` — if discovery of the first path directory returns an error, the whole discovery chain aborts. A single corrupt manifest or permission error on the first search path prevents plugins in all subsequent paths from loading.

**Fix:** Collect errors per-directory and continue discovery, returning a `(Vec<PluginManifest>, Vec<DiscoveryError>)` so callers can log warnings for failed paths while still loading everything that succeeded.

### 7.3 TOCTOU race in manifest loading `[MEDIUM]`

`tumult-plugin/src/discovery.rs:43–57` checks `manifest_path.exists()` and then reads the file in a separate step. Between the check and the read, the file could be deleted or replaced. While this is unlikely in practice, it is a correctness issue that `std::fs::read_to_string` returning `Err` would handle more gracefully than the explicit exists-check pattern.

**Fix:** Remove the `exists()` check; just attempt the read and pattern-match on `ErrorKind::NotFound`.

### 7.4 Plugin trait seal cannot prevent arbitrary workspace crates from implementing `[MEDIUM]`

Already covered in §2.1. Re-stated here as a plugin system concern: any future workspace crate can add `impl private::Sealed for MyPlugin {}` without any gatekeeping. The "in-tree only" requirement in ADR-004 is not mechanically enforced.

---

## 8. Journal Design

### 8.1 No schema version on `Journal` struct `[HIGH]`

The `Journal` struct in `tumult-core/src/types.rs:467–485` has no `schema_version` field and no versioning mechanism. TOON journal files written today will be read by future versions of Tumult without any migration path. If a field is added, removed, or renamed, old journal files will deserialize incorrectly or fail silently.

**Fix:** Add `schema_version: u32` to `Journal` and handle version mismatches in `read_journal` (journal.rs).

### 8.2 `read_journal` uses wrong error variant for decode failure `[HIGH]`

Already noted in §5.2. `tumult-core/src/journal.rs:32` emits `JournalError::EncodeError` when decoding fails. This is a misnaming that will confuse operators and log analysis pipelines.

### 8.3 `timestamp_nanos_opt().expect()` can panic for far-future timestamps `[MEDIUM]`

`tumult-core/src/runner.rs:475` calls `.timestamp_nanos_opt().expect("timestamp out of range")`. The `chrono` docs note that `timestamp_nanos_opt()` returns `None` for timestamps outside the `i64` nanosecond range (roughly year 2262 for the upper bound, 1677 for the lower). While hitting this bound is unlikely today, using `expect` will panic rather than returning an error.

**Fix:** Propagate the `None` case as a `RunnerError` rather than panicking.

### 8.4 `baseline_result`, `during_result`, `post_result` always `None` in emitted journals `[CRITICAL]`

`tumult-core/src/runner.rs:267–271` constructs the `Journal` with `baseline_result: None`, `during_result: None`, `post_result: None` unconditionally — regardless of whether phases 1–3 actually ran. The `Journal` struct has these fields specifically to record five-phase results, but the runner never populates them. Any analytics query or report that reads these fields will always see `None`.

---

## 9. Five-Phase Model

### 9.1 Background activities are not executed asynchronously `[CRITICAL]`

ADR-003 describes background activities (e.g. load generators) as running concurrently during fault injection. `tumult-core/src/runner.rs` calls `partition_background` to separate background from foreground activities, but then `execute_activities` (lines ~354–422) processes **all activities sequentially** regardless of the `background` flag. Load generation activities block the main execution loop.

**Fix:** Spawn background activities with `tokio::spawn` before the method phase begins, capture their `JoinHandle`s, and `.await` them after the method phase completes.

### 9.2 `resilience_score` is binary, not continuous `[HIGH]`

`tumult-core/src/runner.rs:441–445` computes `resilience_score` as either `1.0` (all probes within tolerance) or `0.0` (any probe outside tolerance). ADR-003 describes a 0–1 continuous score reflecting partial resilience. A binary score loses all nuance — an experiment where 9/10 probes pass scores identically to one where 0/10 pass.

**Fix:** Compute `resilience_score` as the ratio of passing probes to total probes: `passing_count as f64 / total_count as f64`. Alternatively, weight probes by severity.

### 9.3 Phase 1–3 (Baseline/During/Post) data never stored in journal `[CRITICAL]`

Already noted in §8.4 — the five-phase ADR model explicitly includes `baseline_result`, `during_result`, and `post_result` as journal fields. These are always `None` in the emitted journal. Phases 1–3 of the chaos model are effectively dark: their results are computed but not recorded.

### 9.4 No `load_result` population `[MEDIUM]`

`Journal.load_result` is always `None`. There is no code path that sets it. If `tumult-loadtest` is ever integrated, this field exists to receive its result — but there is currently no wiring.

---

## 10. Workspace Structure

### 10.1 Planned crates from technical design are absent `[MEDIUM]`

`docs/technical_design.md` lists the following crates as part of the workspace:
- `tumult-regulatory` — DORA/NIS2 mapping & reporting
- `tumult-stress` — stress-ng wrapper
- `tumult-loadtest` — k6/JMeter background drivers

None of these exist in the workspace. `Cargo.toml` workspace members do not include them. The `Journal.load_result` field references `LoadResult` which implies `tumult-loadtest` integration, but the crate does not exist. ADR-003 relies on load generation for background activity during fault injection.

### 10.2 Inconsistent dependency pinning `[LOW]`

Several dependencies bypass the workspace `[workspace.dependencies]` table:
- `dirs-next = "2"` in `tumult-analytics/Cargo.toml:19` — not in workspace deps
- `regex-lite = "0.1"` in `tumult-core/Cargo.toml:22` — not in workspace deps

**Fix:** Add all shared dependencies to `[workspace.dependencies]` and reference them with `{ workspace = true }`.

### 10.3 `StoreStats` defined in `duckdb_store` but used cross-crate `[LOW]`

`tumult-analytics::duckdb_store::StoreStats` is a public struct defined inside a storage-implementation module. It is used by `tumult-clickhouse`. Shared types used across crates should live in a common module, not inside implementation modules.

---

## 11. Missing Crates / Abstractions

### 11.1 No `tumult-runner` crate — runner logic tightly coupled to core `[HIGH]`

The five-phase experiment runner is implemented directly in `tumult-core/src/runner.rs`. This creates a dependency from `tumult-core` on `tumult-baseline` (for stats) and forces the core crate to grow as the runner gains complexity. A dedicated `tumult-runner` crate would:
- Accept `tumult-core` types
- Use `tumult-baseline` for statistical operations
- Use `tumult-plugin` for execution
- Expose only `run_experiment(experiment: &Experiment) -> Result<Journal, RunnerError>`

### 11.2 No `tumult-loadtest` crate `[HIGH]`

ADR-003 and the five-phase model depend on background load generation. The `Journal` struct has a `load_result` field. There is no crate implementing this. Without load generation, the experiment model is incomplete for fault injection scenarios that require steady background load.

### 11.3 No `tumult-regulatory` crate `[MEDIUM]`

`Journal` has a `regulatory: Option<RegulatoryResult>` field (types.rs). The type is defined but there is no crate that populates it. DORA/NIS2 compliance reporting is mentioned in the technical design as a first-class feature.

### 11.4 No async `AnalyticsBackend` trait `[MEDIUM]`

The `AnalyticsBackend` trait in `tumult-analytics/src/backend.rs` is fully synchronous. `tumult-clickhouse` implements it with blocking `Handle::current().block_on()` wrappers (§1.3, §6.4). An `AsyncAnalyticsBackend` trait with `async fn` methods would allow proper async implementations without runtime panics.

---

## 12. ADR Compliance

### ADR-001 (Platform Runtime: Tokio + Async)

| Requirement | Status | Notes |
|---|---|---|
| Tokio as async runtime | ✅ Compliant | Used throughout |
| No blocking calls on Tokio threads | ❌ **Violated** | `std::thread::sleep` in runner.rs:367,412; busy-poll loop in commands.rs:113–128 |
| Async-first I/O | ❌ **Violated** | `ActivityExecutor` is synchronous |

### ADR-002 (Data Observability: Nanosecond timestamps, `resilience.*` namespace)

| Requirement | Status | Notes |
|---|---|---|
| Nanosecond timestamps as `i64` | ✅ Compliant | Used consistently |
| `resilience.*` attribute namespace | ✅ Largely compliant | runner.rs uses `resilience.probe.name`, `resilience.action.name` |
| Non-panicking timestamp handling | ⚠️ **Partial** | `expect()` on `timestamp_nanos_opt()` in runner.rs:475 and duckdb_store.rs:267 |

### ADR-003 (Experiment Model: Five-Phase, Background Activities, Continuous Score)

| Requirement | Status | Notes |
|---|---|---|
| Five-phase model (Baseline/During/Post/Load/Analysis) | ❌ **Violated** | Phases 1–3 results always `None` in journal |
| Background activities run concurrently | ❌ **Violated** | All activities run sequentially in runner.rs:354–422 |
| Continuous `resilience_score` (0–1) | ❌ **Violated** | Binary 0.0/1.0 only in runner.rs:441–445 |
| Load generation integration | ❌ **Not implemented** | No `tumult-loadtest` crate |

### ADR-004 (Extensibility: Feature Flags, Plugin Discovery)

| Requirement | Status | Notes |
|---|---|---|
| Native plugins via Cargo feature flags | ❌ **Violated** | No `[features]` in any crate |
| Plugin discovery path `./tumult-plugins/` | ❌ **Violated** | Implementation uses `./plugins/` (discovery.rs:70) |
| Plugin trait sealed in-tree | ⚠️ **Partial** | Seal exists but does not enforce workspace boundaries |

### ADR-005 (Analytics: DuckDB, schema versioning, Parquet export)

| Requirement | Status | Notes |
|---|---|---|
| DuckDB embedded store | ✅ Compliant | Fully implemented in `duckdb_store.rs` |
| `schema_meta` table for versioning | ✅ Compliant | `duckdb_store.rs:94–114` implements this |
| Parquet export/import | ✅ Compliant | `export_tables`/`import_tables` implemented |
| WAL mode for crash safety | ✅ Compliant | DuckDB uses WAL by default for file-backed stores |
| Incremental ingestion / dedup | ✅ Compliant | `experiment_exists` check + unique index |

---

## Prioritised Fix List

| Priority | Issue | Location |
|---|---|---|
| 1 | `std::thread::sleep` blocking Tokio threads | `runner.rs:367,412` |
| 2 | Nested Tokio runtime panic risk | `commands.rs:249–253` |
| 3 | `block_on` in async context (ClickHouse backend) | `store.rs:388–414` |
| 4 | Phase 1–3 journal fields always `None` | `runner.rs:267–271` |
| 5 | Background activities not async | `runner.rs:354–422` |
| 6 | No Cargo feature flags (ADR-004 violation) | `tumult-cli/Cargo.toml` |
| 7 | Discovery path mismatch (`plugins/` vs `tumult-plugins/`) | `discovery.rs:70` |
| 8 | `JournalError::EncodeError` used for decode failures | `journal.rs:32` |
| 9 | Binary `resilience_score` (should be 0–1 continuous) | `runner.rs:441–445` |
| 10 | `#[non_exhaustive]` missing on public enums | `types.rs` |
| 11 | `tumult-core` depends on `tumult-baseline` | `core/Cargo.toml:11` |
| 12 | No schema version on `Journal` struct | `types.rs:467–485` |
| 13 | `discover_all_plugins` fails fast on first bad path | `discovery.rs:93` |
| 14 | `StoreStats` defined in implementation module, used cross-crate | `duckdb_store.rs:23–26` |
| 15 | Missing `tumult-loadtest`, `tumult-regulatory`, `tumult-stress` crates | workspace |
