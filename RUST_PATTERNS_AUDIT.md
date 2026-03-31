# Rust Patterns Audit — Tumult Workspace

**Date:** 2026-03-31  
**Scope:** All 8 crates — `tumult-core`, `tumult-cli`, `tumult-plugin`, `tumult-otel`, `tumult-baseline`, `tumult-ssh`, `tumult-analytics`, `tumult-kubernetes`  
**Methodology:** Full source read against the rust-patterns skill (179 rules across 14 categories)

---

## Severity Legend

| Label | Meaning |
|-------|---------|
| **BLOCKING** | Correctness bug or semantic error — must fix |
| **IMPORTANT** | Perf regression, API contract violation, or async hazard |
| **SUGGESTION** | Idiomatic improvement with meaningful impact |
| **NIT** | Minor style, naming, or micro-optimisation |

---

## 1. `tumult-core`

### `src/journal.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| C-1 | **BLOCKING** | `err-custom-type` | `journal.rs:32` | `read_journal` maps a decode error to `JournalError::EncodeError`. Wrong variant name — `DecodeError` should be used for read failures. | Add a `DecodeError` variant to `JournalError` and use it in the `from_slice` error arm. |

### `src/types.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| C-2 | **IMPORTANT** | `api-non-exhaustive` | `types.rs` (all public enums) | `ActivityType`, `ExperimentStatus`, `ActivityStatus`, `HttpMethod`, `ControlFlow`, `LifecycleEvent` are all `pub` enums with no `#[non_exhaustive]`. Adding a new variant is a breaking change for any downstream match. | Add `#[non_exhaustive]` to every public enum exposed in this crate. |
| C-3 | **SUGGESTION** | `type-newtype-ids` | `types.rs` (multiple structs) | `ActivityResult.trace_id`, `ActivityResult.span_id`, `Journal.experiment_id` are raw `String`. No type safety prevents passing a `trace_id` where a `span_id` is expected. | Introduce `TraceId(String)`, `SpanId(String)`, `ExperimentId(String)` newtypes with `#[repr(transparent)]`. |
| C-4 | **NIT** | `api-common-traits` | `types.rs` | `HypothesisResult`, `ActivityResult`, `Journal` do not derive `PartialEq`. Makes unit testing with `assert_eq!` impossible without manual comparison. | Add `#[derive(PartialEq)]` to these structs. |

### `src/engine.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| C-5 | **IMPORTANT** | `perf-profile-first` / `anti-format-hot-path` | `engine.rs:204–208` | `evaluate_tolerance` (Regex arm) compiles a `Regex` on every invocation via `Regex::new(pattern)`. Called per-probe per-experiment. | Cache the compiled `Regex` in the config/probe struct, or use `once_cell::sync::Lazy`. |
| C-6 | **NIT** | `own-borrow-over-clone` | `engine.rs:44–45` | `resolve_config` clones `key` and `env_key` before an error branch that may not be taken. | Move the `clone()` calls inside the `None` branch or use `to_owned()` only where needed. |
| C-7 | **NIT** | `err-custom-type` | `engine.rs` | `EngineError::SecretFileNotFound` is reused for both "file not found" and "read failed". Error consumers cannot distinguish the two cases. | Split into `SecretFileNotFound { path }` and `SecretReadFailed { path, source }`. |

### `src/runner.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| C-8 | **IMPORTANT** | `async-spawn-blocking` | `runner.rs:367,414` | `std::thread::sleep` is called inside an async function (the wait-loop for cooldown/background activities). This blocks the Tokio thread. | Replace with `tokio::time::sleep(duration).await`. |
| C-9 | **SUGGESTION** | `anti-stringly-typed` | `runner.rs:386` | `format!("{:?}", activity.activity_type)` is used to produce an OTel attribute value. Debug formatting is fragile; adding a new enum variant changes the attribute string in traces. | Implement `Display` (or a custom `as_str()`) on `ActivityType` so the OTel key is stable and controlled. |
| C-10 | **NIT** | `mem-avoid-format` | `runner.rs` | `span_builder("tumult.probe".to_string())` — `to_string()` on a string literal. | Use `.into()` or accept `&'static str` if the span builder allows it. |
| C-11 | **NIT** | `own-borrow-over-clone` | `runner.rs:452,463` | `current_trace_id()` / `current_span_id()` each call `.clone()` on the `SpanContext`. | Confirm whether clones are truly required; if the value is only formatted/logged, borrow instead. |

### `src/controls.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| C-12 | **SUGGESTION** | `perf-iter-lazy` | `controls.rs` | `ControlRegistry::handler_names()` returns `Vec<&str>` — allocates a `Vec` every call, often used just for display or lookup. | Change return type to `impl Iterator<Item = &str>` to avoid the allocation. |

---

## 2. `tumult-cli`

### `src/commands.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| CL-1 | **IMPORTANT** | `async-spawn-blocking` | `commands.rs:123` | `execute_process` busy-polls with `std::thread::sleep` inside a timeout loop. Blocks the Tokio worker thread. | Replace with `tokio::time::sleep().await` or use `tokio::process::Command` and its async `.wait()`. |
| CL-2 | **IMPORTANT** | `async-no-lock-await` | `commands.rs:249` | `auto_ingest_journal` calls `tokio::runtime::Handle::current().block_on(...)` — synchronous blocking inside an async context. | Make `auto_ingest_journal` async and call the inner async function directly with `.await`. |
| CL-3 | **IMPORTANT** | `err-no-unwrap-prod` | `commands.rs:405` | `discover_all_plugins().unwrap_or_default()` silently swallows plugin discovery errors. A misconfigured plugin path would be silently ignored. | Return the error (propagate via `?`) or log it explicitly with `tracing::warn!` before defaulting. |
| CL-4 | **NIT** | `err-no-unwrap-prod` | `commands.rs:549` | `file_stem().unwrap_or_default()` silently uses an empty string as the output filename if the path has no stem. | Use `unwrap_or_else(|| OsStr::new("output"))` or return an error for this degenerate case. |

---

## 3. `tumult-plugin`

### `src/traits.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| P-1 | **IMPORTANT** | `own-slice-over-vec` | `traits.rs` | `TumultPlugin::actions()` and `probes()` return `Vec<ActionDescriptor>` (allocating on every call). The registry calls these in lookup hot paths. | Change the trait to return `&[ActionDescriptor]` or `impl Iterator<Item = &ActionDescriptor>`, backed by a stored `Vec` in implementors. |
| P-2 | **NIT** | `api-must-use` | `traits.rs` | `PluginOutput` struct is returned from plugin executions but missing `#[must_use]`. | Add `#[must_use]` to the struct. |

### `src/registry.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| P-3 | **IMPORTANT** | `anti-clone-excessive` | `registry.rs:44,53` | `has_action` / `has_probe` each call `p.actions()` / `p.probes()`, allocating a new `Vec` per lookup. In the current design this is called per-execution. | Fix P-1 first (return `&[T]`), then these become zero-allocation slice scans. |
| P-4 | **SUGGESTION** | `own-slice-over-vec` | `registry.rs` | `list_plugins()` returns `Vec<String>` by cloning all plugin names. | Return `Vec<&str>` (borrowing from the stored Arc'd plugins) or `impl Iterator<Item = &str>`. |
| P-5 | **NIT** | `anti-clone-excessive` | `registry.rs:64–81` | `list_all_actions()` clones `String` fields unnecessarily when the caller only needs to read them. | Return borrowed `&ActionDescriptor` slices if ownership is not needed. |

### `src/discovery.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| P-6 | **SUGGESTION** | `err-custom-type` | `discovery.rs` | `DiscoveryError::ManifestParse` uses `Box<dyn std::error::Error + Send + Sync>` as the source, inconsistent with the rest of the workspace which uses `thiserror` with concrete types. | Change to a concrete source type (e.g., `serde_json::Error`) or a named wrapper variant. |

### `src/executor.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| P-7 | **NIT** | `mem-avoid-format` | `executor.rs:86–87` | `String::from_utf8_lossy(&output.stdout).to_string()` — double allocation: `Cow` is created then `.to_string()` allocates again. | Use `String::from_utf8_lossy(&output.stdout).into_owned()` which only allocates once (a no-op when already `Owned`). |

---

## 4. `tumult-otel`

### `src/config.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| O-1 | **NIT** | `mem-avoid-format` | `config.rs:16,37` | `"tumult".to_string()` appears twice. | Extract as `const DEFAULT_SERVICE_NAME: &str = "tumult"` and use `.to_string()` once in the default method. |

### `src/telemetry.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| O-2 | **SUGGESTION** | `mem-with-capacity` | `telemetry.rs` | `TumultTelemetry` stores the entire `TelemetryConfig` at runtime but only needs `enabled`, `service_name`, and `endpoint`. | Store only what's needed at runtime rather than the full config struct. |

---

## 5. `tumult-baseline`

### `src/acquisition.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| B-1 | **NIT** | `own-borrow-over-clone` | `acquisition.rs:159` | `ps.values.clone()` to sort a local copy — clones the entire `Vec<f64>`. | Clone only the slice when needed, or compute order on indices without cloning. |
| B-2 | **NIT** | `err-no-unwrap-prod` | `acquisition.rs` | `u32::try_from(ps.values.len()).unwrap_or(u32::MAX)` in non-test code. | Use `u32::try_from(...).unwrap_or(u32::MAX)` is acceptable (well-documented fallback), but prefer explicit `saturating_cast` or document the intent. |

### `src/tolerance.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| B-3 | **IMPORTANT** | `api-non-exhaustive` | `tolerance.rs:13` | `Method` enum is `pub` without `#[non_exhaustive]`. Adding `Sigma3`, `Iqr2`, etc. would be breaking. | Add `#[non_exhaustive]`. |

### `src/telemetry.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| B-4 | **NIT** | `mem-avoid-format` | `baseline/telemetry.rs:19,45` | `method.to_string()` and `reason.to_string()` called on `&str` arguments just to satisfy `KeyValue::new`. Unavoidable given the OTel API, but worth noting as a systematic pattern. | No change needed; the OTel API requires `String`. Document as intentional. |

---

## 6. `tumult-ssh`

### `src/session.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| S-1 | **IMPORTANT** | Security | `session.rs:356–364` | `ClientHandler::check_server_key` unconditionally returns `Ok(true)`, accepting any host key. Vulnerable to MITM. A TODO is present but untracked. | Implement known-hosts file verification. Until then, ensure the security warning in the doc comment is sufficient for the deployment context and add a feature flag to enforce it. |
| S-2 | **NIT** | `mem-avoid-format` | `session.rs:89` | `channel.exec(true, command.to_string())` — `command` is already `&str`; `.to_string()` allocates. | Pass `command` directly if the russh API accepts `impl Into<String>`; if not, the allocation is unavoidable — add a comment. |
| S-3 | **NIT** | `mem-avoid-format` | `session.rs:144,154` | `String::from_utf8_lossy(&stderr).trim().to_string()` — same double-allocation pattern as P-7. | Use `.into_owned()` after `.trim()`, or trim the `Cow` before calling `to_string()`. |
| S-4 | **NIT** | `api-must-use` | `session.rs:254–256` | `SshSession::config()` getter has no `#[must_use]`. Minor, but consistent with the API guidelines. | Add `#[must_use]`. |

### `src/config.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| S-5 | **SUGGESTION** | `api-builder-pattern` | `config.rs` | `SshConfig` uses a partial builder (chainable setters), but constructor functions take `&str`/`PathBuf` differently. The pattern is inconsistent. | Commit fully to builder: add `SshConfigBuilder` with `#[must_use]` on the builder type and a terminal `build()` method, or leave as-is (current design is workable). |
| S-6 | **NIT** | `api-impl-into` | `config.rs:56,71` | `with_key(host: &str, user: &str, ...)` and `with_agent(host: &str, user: &str)` accept `&str` — good — but then immediately call `.to_string()`. | This is correct; the allocation is intentional (stored as `String`). No change needed. |

---

## 7. `tumult-analytics`

### `src/duckdb_store.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| A-1 | **IMPORTANT** | `err-expect-bugs-only` | `duckdb_store.rs:41` | `dirs_next::home_dir().expect("cannot determine home directory")` in `default_path()`. On some headless Linux systems (CI, containers) this returns `None` and panics. | Return `Result<PathBuf, AnalyticsError>` from `default_path()` and propagate the error to callers. |
| A-2 | **IMPORTANT** | `err-expect-bugs-only` | `duckdb_store.rs:267` | `chrono::Utc::now().timestamp_nanos_opt().expect("system time before year 2262")` — reasonable assumption for nanoseconds but `.expect()` still panics in production. | Use `.unwrap_or(i64::MAX)` or document why this is a programmer error. |
| A-3 | **SUGGESTION** | `mem-with-capacity` | `duckdb_store.rs:215–225` | `Vec::new()` in `query()` result collection; row count is unknown upfront but could use a reasonable initial capacity. | Minor; only matters for large result sets. |
| A-4 | **NIT** | `own-borrow-over-clone` | `duckdb_store.rs:390` | `batches.into_iter().next().unwrap()` after `.len() == 1` check — the `.unwrap()` is guaranteed safe but still triggers `unwrap-in-prod` lints. | Use `batches.into_iter().next().expect("len==1 checked above")` with clear context. |
| A-5 | **NIT** | `err-no-unwrap-prod` | `duckdb_store.rs:399` | `appender.append_record_batch(batch.clone())?` — `batch.clone()` here is needed by the DuckDB API. | Add a comment `// DuckDB appender takes ownership, clone required` for clarity. |

### `src/arrow_convert.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| A-6 | **IMPORTANT** | `anti-stringly-typed` | `arrow_convert.rs:56,88,89` | `format!("{:?}", journal.status)`, `format!("{:?}", r.activity_type)`, `format!("{:?}", r.status)` use Rust `Debug` formatting to produce database-stored strings. If a variant's debug repr changes (e.g. during rename), stored data becomes inconsistent with the enum. | Implement `Display` or a stable `as_str()` on `ExperimentStatus`, `ActivityType`, `ActivityStatus` and use those instead. |
| A-7 | **SUGGESTION** | `mem-with-capacity` | `arrow_convert.rs:74–82` | All `Vec` fields in `journal_to_activity_batch` are allocated with `Vec::new()`. The total count (sum of all phases) is known before the loop. | Compute total activity count upfront and use `Vec::with_capacity(total)` for all column vecs. |
| A-8 | **NIT** | `own-borrow-over-clone` | `arrow_convert.rs:86` | `exp_ids.push(journal.experiment_id.clone())` inside a closure called once per activity — necessary since `experiment_id` is borrowed from `journal`, but the closure also captures `journal`. This is correct. | No change needed; annotate the intent. |

### `src/export.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| A-9 | **NIT** | `async-tokio-fs` | `export.rs:3,18,35,48` | `std::fs::File` used in what could be an async pipeline. The DuckDB/Arrow writers are sync, so this is correct. | Annotate with a comment that file I/O here is intentionally sync due to sync writer APIs. |

### `src/backend.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| A-10 | **SUGGESTION** | `api-sealed-trait` | `backend.rs:16` | `AnalyticsBackend` is a `pub` trait. External crates could implement it without any guidance or stability promise. | Consider sealing the trait with a private supertrait, or explicitly document it as open for extension. |

---

## 8. `tumult-kubernetes`

### `src/actions.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| K-1 | **NIT** | `mem-avoid-format` | `actions.rs:28,52,54,72,90` | Every action returns `format!("pod {}/{} deleted", ...)` etc. These `format!` allocations are on already-rare success paths, so no perf concern. | No change needed. Cosmetic. |
| K-2 | **NIT** | `api-non-exhaustive` | `actions.rs:94` | `DrainResult` is `pub` and `#[derive(Clone)]` but not `#[non_exhaustive]`. Adding a field (e.g. `drain_duration_ms`) is a breaking change. | Add `#[non_exhaustive]` to `DrainResult`. |
| K-3 | **SUGGESTION** | `async-tokio-fs` | `actions.rs:77` | `uncordon_node` calls `begin_cordon_node` for its span — the span name is `k8s.node.cordon` even for uncordon operations. Telemetry will be misleading. | Add `begin_uncordon_node` to `telemetry.rs` mirroring `begin_cordon_node`. |

### `src/probes.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| K-4 | **SUGGESTION** | `api-non-exhaustive` | `probes.rs:16,27,37,45` | `PodStatus`, `DeploymentStatus`, `NodeStatus`, `NodeCondition` are all `pub` structs without `#[non_exhaustive]`. Adding Kubernetes-side fields later is a breaking change. | Add `#[non_exhaustive]` to all four status structs. |
| K-5 | **NIT** | `own-borrow-over-clone` | `probes.rs:212,225` | `pod.status.as_ref().and_then(|s| s.phase.clone())` — `phase` is `Option<String>`; the clone is required to move out of a reference. Pattern is correct and unavoidable with the k8s-openapi types. | No change needed. |

### `src/telemetry.rs`

| # | Severity | Rule | File:Line | Finding | Fix |
|---|----------|------|-----------|---------|-----|
| K-6 | **NIT** | `mem-avoid-format` | `kubernetes/telemetry.rs:15` | `span_builder(name.to_string())` — `name` is `&str` passed to `k8s_span`; allocates on every span creation. | If the `span_builder` API accepts `impl Into<String>`, this is unavoidable. Confirm and document. |
| K-7 | **NIT** | `proj-pub-crate-internal` | `kubernetes/telemetry.rs:12` | `k8s_span` helper is `fn` (implicitly `pub(crate)` within the module but declared without `pub`). All callers are within the same crate. | Add `pub(crate)` explicitly for clarity, or keep private — currently correct since it's module-private. No change strictly needed. |

---

## Cross-Cutting Findings

### OTel telemetry modules (`tumult-*`)

| # | Severity | Rule | Finding | Fix |
|---|----------|------|---------|-----|
| X-1 | **SUGGESTION** | `proj-pub-crate-internal` | All `telemetry.rs` modules expose `SpanGuard` / `IngestGuard` / `QueryGuard` etc. as `pub` structs with private fields (`_guard`). These are only ever used within the crate. | Change to `pub(crate)` visibility since none of these are exported from the crate's `lib.rs` public API. |
| X-2 | **SUGGESTION** | `api-non-exhaustive` | Each `telemetry.rs` guard type (`SpanGuard`, `IngestGuard`, etc.) is structurally identical: `struct { _guard: ContextGuard }`. | Extract a single `pub(crate) struct TelemetryGuard { _guard: opentelemetry::ContextGuard }` in a shared internal module, or declare a macro to reduce boilerplate. |
| X-3 | **NIT** | `mem-avoid-format` | Every `telemetry.rs` file calls `.to_string()` on `&str` arguments to satisfy `KeyValue::new`. This is required by the OTel API and unavoidable without a newtype wrapper. | Document once in a crate-level comment that `KeyValue::new` requires owned strings; this is not a bug. |

### `#[non_exhaustive]` sweep

The following public enums and structs are missing `#[non_exhaustive]` and are likely to grow:

| Crate | Type | Kind |
|-------|------|------|
| `tumult-core` | `ActivityType` | enum |
| `tumult-core` | `ExperimentStatus` | enum |
| `tumult-core` | `ActivityStatus` | enum |
| `tumult-core` | `HttpMethod` | enum |
| `tumult-core` | `ControlFlow` | enum |
| `tumult-core` | `LifecycleEvent` | enum |
| `tumult-baseline` | `Method` | enum |
| `tumult-kubernetes` | `DrainResult` | struct |
| `tumult-kubernetes` | `PodStatus` | struct |
| `tumult-kubernetes` | `DeploymentStatus` | struct |
| `tumult-kubernetes` | `NodeStatus` | struct |
| `tumult-kubernetes` | `NodeCondition` | struct |

### Stringly-typed debug serialisation

Findings A-6 and C-9 are the same root cause: `format!("{:?}", variant)` is used to produce both OTel attribute values and Arrow/DuckDB database strings. This means:

1. A variant rename silently changes stored data.
2. The debug output includes field names for struct-like variants (`Action { .. }`), which is not the intended database value.

**Recommended fix:** Implement a stable `Display` (or `as_str()`) on `ActivityType`, `ActivityStatus`, and `ExperimentStatus` — once — and use that everywhere.

---

## Summary by Priority

| Priority | Count | Findings |
|----------|-------|---------|
| BLOCKING | 1 | C-1 |
| IMPORTANT | 10 | C-5, C-8, CL-1, CL-2, CL-3, P-1, P-3, S-1, A-1, A-6 |
| SUGGESTION | 12 | C-2, C-3, C-9, C-12, P-6, O-2, B-3, S-5, A-7, A-10, K-3, K-4, X-1, X-2 |
| NIT | 18 | C-4, C-6, C-7, C-10, C-11, CL-4, P-2, P-4, P-5, P-7, O-1, B-1, B-2, B-4, S-2, S-3, S-4, S-6, A-2, A-3, A-4, A-5, A-8, A-9, K-1, K-2, K-5, K-6, K-7, X-3 |

---

## Recommended Action Order

1. **Fix C-1 first** — the `EncodeError`/`DecodeError` naming is a semantic bug that can cause misdiagnosis in production.
2. **Fix A-6 + C-9** together — stable serialisation of enum variants affects both database integrity and OTel trace fidelity.
3. **Fix C-8 and CL-1** together — blocking `std::thread::sleep` in async code starves the Tokio runtime under load.
4. **Fix CL-2** — `block_on` inside async context can deadlock on single-threaded runtimes.
5. **Fix CL-3** — silent plugin discovery errors are an operational hazard.
6. **Fix P-1/P-3** together — the allocating `Vec` return from trait methods is the root cause of the registry hot-path allocations.
7. **Fix A-1** — `default_path()` panics on headless containers; change to `Result`.
8. **Apply `#[non_exhaustive]`** sweep (C-2, B-3, K-2, K-4) — one PR, mechanical.
9. **Fix C-5** — regex recompilation per probe invocation will matter at scale.
