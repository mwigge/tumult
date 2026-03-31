# Tumult Platform — Full Code Review

**Date:** 2026-03-31  
**Scope:** All 10 active workspace crates (`tumult-core`, `tumult-cli`, `tumult-plugin`, `tumult-otel`, `tumult-baseline`, `tumult-ssh`, `tumult-analytics`, `tumult-kubernetes`, `tumult-mcp`, `tumult-clickhouse`)  
**Methodology:** Live `cargo check` + `cargo clippy --all-targets -- -D warnings -W clippy::pedantic`, full source read, cross-referenced against Rust patterns skill (179 rules / 14 categories), rust-agentic RPI methodology, existing `RUST_PATTERNS_AUDIT.md`, and `SECURITY-AUDIT.md`.  
**Tools:** Rust Patterns skill, Rust Agentic skill, Senior Developer / Code Reviewer perspective, Rust API Guidelines, Rust Performance Book.

---

## Executive Summary

| Dimension | Status |
|-----------|--------|
| Compilation | **FAILING** — 3 crates fail to compile (`tumult-baseline`, `tumult-otel`, `tumult-ssh`) |
| Clippy (pedantic) | **FAILING** — 159 errors across the workspace |
| Security posture | **HIGH RISK** — 1 CRITICAL, 5 HIGH security findings (see SECURITY-AUDIT.md) |
| Async correctness | **IMPORTANT** — blocking `std::thread::sleep` inside async runner, `block_on` inside async context |
| API design | **IMPROVEMENT NEEDED** — missing `#[non_exhaustive]`, stringly-typed enum serialisation, allocating Vec returns from trait methods |
| Documentation | **PARTIAL** — public items mostly documented, missing `# Errors` sections, missing `#[must_use]` annotations |
| Test coverage | **GOOD** — extensive unit tests with arrange/act/assert, property tests on core types, RAII cleanup |
| Memory efficiency | **IMPROVEMENT NEEDED** — unnecessary clones in hot paths, unresized Vec allocations, regex recompilation |

---

## Severity Legend

| Label | Meaning |
|-------|---------|
| **BLOCKING** | Build error or correctness bug — must fix before merge |
| **IMPORTANT** | Async hazard, security risk, or perf regression under load |
| **SUGGESTION** | Idiomatic improvement with meaningful impact |
| **NIT** | Minor style, naming, or micro-optimisation |

---

## 1. Build Failures (BLOCKING)

These three crates **do not compile**. All downstream crates that depend on them are also broken.

### 1.1 `tumult-baseline` — OTel API mismatch

**File:** `tumult-baseline/src/telemetry.rs:3,50`

```
error[E0432]: unresolved import `opentelemetry::trace::StatusCode`
error[E0061]: set_status takes 1 argument (Status enum), not 2
```

**Root cause:** The crate imports `opentelemetry::trace::StatusCode` which no longer exists in `opentelemetry 0.31`. The OTel 0.31 API uses `opentelemetry::trace::Status` (an enum: `Status::Ok`, `Status::Error { description }`, `Status::Unset`) and `Span::set_status` takes a single `Status` value.

**Fix:**
```rust
// telemetry.rs:3 — remove StatusCode import
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};

// telemetry.rs:50 — use Status::error(description)
span.set_status(Status::error(reason.to_string()));
```

The current `telemetry.rs:50` already uses `Status::error(reason.to_string())` — the import line was not updated to remove the old `StatusCode`. **Fix: remove `StatusCode` from the import.**

**Additional:** 34 total errors cascade from this single broken import — all cast precision, `must_use`, and `missing_errors_doc` warnings become errors under `-D warnings`.

---

### 1.2 `tumult-otel` — 12 errors (cascading from pedantic lints as errors)

`tumult-otel` compiles cleanly at `cargo check` level but fails under `-D warnings -W pedantic` with 12 errors. Key issues:

- Missing `#[must_use]` on `is_enabled()`, `service_name()` getters (pedantic `must_use_candidate`)
- `uninlined_format_args` in error messages throughout `config.rs`
- `doc_markdown` — "OTel" in doc comments needs backticks

These are all pedantic lint violations, not logic bugs. The crate is functionally correct.

---

### 1.3 `tumult-ssh` — 26 errors (pedantic + real issues)

`tumult-ssh/src/session.rs` and `tumult-ssh/src/telemetry.rs` produce 26 errors:

- **Real issue:** `cast_possible_wrap` — `file_bytes as i64`, `stdout_bytes as i64`, `stderr_bytes as i64` cast `u64`/`usize` to `i64` which can wrap on 64-bit targets when values exceed `i64::MAX`. The OTel `KeyValue::new` API requires `i64`. **Fix:** use `i64::try_from(n).unwrap_or(i64::MAX)` or `min(n, i64::MAX as u64) as i64`.
- **Real issue:** `match_same_arms` — in `session.rs:220` the `Some(ChannelMsg::Eof)` arm and the wildcard arm both have identical empty bodies. **Fix:** merge into wildcard or handle EOF explicitly.
- **Pedantic:** `uninlined_format_args`, `doc_markdown`, `missing_errors_doc` throughout.

---

## 2. `tumult-core`

### 2.1 BLOCKING — Wrong error variant for decode failures (`journal.rs:32`)

**Rule:** `err-custom-type`  
**File:** `tumult-core/src/journal.rs:32`

`read_journal` maps a TOON decode error to `JournalError::EncodeError`. This is semantically wrong — a read/decode failure is not an encode failure. Error consumers trying to distinguish encode vs decode failures by matching the variant will be misled.

```rust
// Current (WRONG):
toon_format::decode_default(&content).map_err(|e| JournalError::EncodeError(e.to_string()))

// Fix: add a DecodeError variant
#[derive(Error, Debug)]
pub enum JournalError {
    #[error("failed to encode journal to TOON: {0}")]
    EncodeError(String),
    #[error("failed to decode journal from TOON: {0}")]
    DecodeError(String),   // ADD THIS
    #[error("failed to write journal file: {0}")]
    WriteError(#[from] std::io::Error),
}

// Then in read_journal:
toon_format::decode_default(&content).map_err(|e| JournalError::DecodeError(e.to_string()))
```

---

### 2.2 IMPORTANT — Blocking sleep in async runner (`runner.rs:441,491`)

**Rule:** `async-spawn-blocking`  
**File:** `tumult-core/src/runner.rs:441,491`

`execute_activities` calls `std::thread::sleep` directly inside what is called from an async context (the CLI runner is tokio-based). This blocks the Tokio worker thread entirely for the pause duration, preventing other tasks from running. Under the default `tokio::main` multi-thread runtime this starves the thread pool; under a single-thread runtime it deadlocks.

```rust
// Current (DANGEROUS in async context):
std::thread::sleep(std::time::Duration::from_secs_f64(pause));

// Fix: make execute_activities async and await the sleep
tokio::time::sleep(std::time::Duration::from_secs_f64(pause)).await;
```

Note: `runner.rs` is currently `fn` (synchronous). The fix requires making `run_experiment`, `execute_activities`, and related functions `async`. Alternatively, if keeping synchronous, use `spawn_blocking` at the call site in the CLI layer instead.

---

### 2.3 IMPORTANT — Stringly-typed enum debug serialisation (`runner.rs:458, arrow_convert.rs:56,88,89`)

**Rule:** `anti-stringly-typed`  
**Files:** `tumult-core/src/runner.rs:458`, `tumult-analytics/src/arrow_convert.rs:56,88,89`

Both OTel span attributes and DuckDB/Arrow column values use `format!("{:?}", activity.activity_type)` to convert enums to strings. This means:

1. Renaming a variant (e.g. `Action` → `FaultAction`) silently changes stored database values, breaking historical queries.
2. The Debug representation may include struct syntax for future variants (`Action { target: "k8s" }`), producing malformed strings.
3. OTel attribute names in traces become unstable across refactors.

**Fix:** Implement `Display` (or a stable `as_str()` method) on `ActivityType`, `ActivityStatus`, and `ExperimentStatus`:

```rust
impl fmt::Display for ActivityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Action => "action",
            Self::Probe  => "probe",
        })
    }
}
```

Then use `activity.activity_type.to_string()` everywhere instead of `format!("{:?}", ...)`.

---

### 2.4 IMPORTANT — `#[non_exhaustive]` missing on all public enums

**Rule:** `api-non-exhaustive`  
**File:** `tumult-core/src/types.rs` (all enums), `tumult-baseline/src/tolerance.rs`

Public enums without `#[non_exhaustive]` make adding a new variant a **breaking change** for any downstream match. The following types are expected to grow as the platform matures:

| Type | Location | Risk |
|------|----------|------|
| `ActivityType` | `types.rs:16` | New activity categories |
| `ExperimentStatus` | `types.rs:23` | New terminal states |
| `ActivityStatus` | `types.rs:32` | New execution outcomes |
| `HttpMethod` | `types.rs:41` | CONNECT, OPTIONS, HEAD |
| `ControlFlow` | `controls.rs` | Conditional skip, retry |
| `LifecycleEvent` | `controls.rs` | New lifecycle hooks |
| `BaselineMethod` | `tolerance.rs` | New statistical methods |

**Fix:** Add `#[non_exhaustive]` to each. Downstream `match` expressions will need a wildcard arm — which is the desired behaviour for forward compatibility.

---

### 2.5 SUGGESTION — Regex recompilation per probe invocation (`engine.rs:~200`)

**Rule:** `anti-format-hot-path` / `perf-profile-first`  
**File:** `tumult-core/src/engine.rs` (`evaluate_tolerance` Regex arm)

`Regex::new(pattern)` is called on every probe invocation. For experiments with many probes or tight polling loops, this compiles the same pattern thousands of times.

**Fix:** Cache compiled regexes at experiment validation time in the `Experiment` struct, or use `once_cell::sync::Lazy` with a pattern-keyed cache:

```rust
// In validate_experiment, pre-compile all regex tolerances
// Store as an auxiliary map passed to execute_activities
// Or use a thread-local LRU cache keyed by pattern string
```

---

### 2.6 SUGGESTION — Newtype IDs for `trace_id` / `span_id` / `experiment_id`

**Rule:** `type-newtype-ids`  
**File:** `tumult-core/src/types.rs:492`

`ActivityResult.trace_id`, `ActivityResult.span_id`, and `Journal.experiment_id` are all raw `String` fields. There is no type-level distinction preventing `trace_id` from being passed where `span_id` is expected. This becomes a real risk as the codebase grows.

**Fix:**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TraceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[repr(transparent)]
pub struct SpanId(pub String);
```

---

### 2.7 NIT — Unnecessary `.to_string()` on string literal (`runner.rs:358,464`)

**Rule:** `mem-avoid-format`  
**File:** `tumult-core/src/runner.rs:358,464`

```rust
// Current:
tracer.span_builder("tumult.probe".to_string())
tracer.span_builder(span_name.to_string())

// Fix (if builder accepts impl Into<String> — it does):
// The .to_string() is required here since span_name is &'static str and
// span_builder takes impl Into<String>. This is unavoidable but harmless.
// Add a comment noting the allocation is required by the OTel API.
```

---

### 2.8 NIT — `count() as u32` truncation in runner (`runner.rs:202,312`)

**Rule:** `err-no-unwrap-prod`  
**File:** `tumult-core/src/runner.rs:202,312`

```rust
.count() as u32
```

If there are more than `u32::MAX` rollback failures (extremely unlikely), this silently wraps. Use `u32::try_from(...).unwrap_or(u32::MAX)` with a comment, or `saturating_cast` (nightly).

---

## 3. `tumult-cli`

### 3.1 IMPORTANT — `block_on` inside async context (`commands.rs:~249`)

**Rule:** `async-no-lock-await`  
**File:** `tumult-cli/src/commands.rs:~249`

`auto_ingest_journal` (or its callers) uses `tokio::runtime::Handle::current().block_on(...)` to call async code from within an async function. This is a nested runtime call that **deadlocks on single-threaded Tokio runtimes** and is a code smell on multi-threaded ones.

**Fix:** Make `auto_ingest_journal` `async` and call the inner async function with `.await` directly. Remove `block_on` entirely.

---

### 3.2 IMPORTANT — Silent plugin discovery error (`commands.rs:~405`)

**Rule:** `err-no-unwrap-prod`  
**File:** `tumult-cli/src/commands.rs:~405`

```rust
// Current:
discover_all_plugins().unwrap_or_default()
```

A misconfigured plugin directory, permissions error, or malformed manifest is silently swallowed. The user gets an empty plugin list with no diagnostic.

**Fix:**
```rust
let plugins = discover_all_plugins().unwrap_or_else(|e| {
    tracing::warn!(error = %e, "plugin discovery failed; continuing with no plugins");
    vec![]
});
```

---

### 3.3 IMPORTANT — Busy-poll with `std::thread::sleep` in process executor (`commands.rs:~123`)

**Rule:** `async-spawn-blocking`  
**File:** `tumult-cli/src/commands.rs:~123`

`execute_process` polls a child process in a loop with `std::thread::sleep`. This blocks the Tokio thread.

**Fix:** Use `tokio::process::Command` which provides async `.wait()` and `.wait_with_output()`, eliminating the polling loop entirely:

```rust
use tokio::process::Command;

let output = tokio::time::timeout(
    std::time::Duration::from_secs_f64(*timeout_s),
    Command::new(path).args(arguments).envs(env).output(),
)
.await??;
```

---

### 3.4 NIT — `file_stem().unwrap_or_default()` empty string fallback (`commands.rs:~549`)

**Rule:** `err-no-unwrap-prod`

Silent empty-string filename when path has no stem. Replace with a named default:

```rust
.unwrap_or_else(|| std::ffi::OsStr::new("output"))
```

---

### 3.5 NIT — HTTP provider not yet implemented silently fails

**File:** `tumult-cli/src/commands.rs:44–54`

The `ProviderExecutor::execute` for `Provider::Http` returns `success: false` with an error message. This means HTTP probes in experiments silently fail at runtime without any compile-time indication. This is acceptable for Phase 0 but should be tracked as a TODO with a `tracing::error!` message rather than a silent failure returned as an `ActivityOutcome`.

---

## 4. `tumult-plugin`

### 4.1 IMPORTANT — Allocating `Vec` returned from trait hot path

**Rule:** `own-slice-over-vec`  
**File:** `tumult-plugin/src/traits.rs:48,49`

```rust
// Current:
fn actions(&self) -> Vec<ActionDescriptor>;
fn probes()  -> Vec<ProbeDescriptor>;
```

Every lookup call (`has_action`, `has_probe`, `list_all_actions`) allocates a new `Vec`. In the registry hot path (called per experiment step), this is constant allocation pressure.

**Fix:**
```rust
fn actions(&self) -> &[ActionDescriptor];
fn probes(&self)  -> &[ProbeDescriptor];
```

Implementors store the descriptors as a `Vec<ActionDescriptor>` field and return a slice. This is a breaking trait change but the trait is sealed (`private::Sealed`), so only crate-internal implementations need updating.

---

### 4.2 IMPORTANT — Cascading allocation in registry lookup (`registry.rs:44,53`)

**Rule:** `anti-clone-excessive`  
**File:** `tumult-plugin/src/registry.rs:44,53`

`has_action` / `has_probe` call `p.actions()` / `p.probes()` which (before fix 4.1) allocate new `Vec`s on every call. Fix 4.1 resolves this.

---

### 4.3 SUGGESTION — `DiscoveryError::ManifestParse` uses `Box<dyn Error>`

**Rule:** `err-custom-type`  
**File:** `tumult-plugin/src/discovery.rs`

Inconsistent with the rest of the workspace (which uses `thiserror` with concrete error types). Replace the boxed trait object with a concrete source:

```rust
#[error("failed to parse plugin manifest at {path}: {source}")]
ManifestParse {
    path: PathBuf,
    #[source]
    source: serde_json::Error,
},
```

---

### 4.4 NIT — `String::from_utf8_lossy(...).to_string()` double allocation (`executor.rs:86`)

**Rule:** `mem-avoid-format`  
**File:** `tumult-plugin/src/executor.rs:86`

```rust
// Current — two allocations:
String::from_utf8_lossy(&output.stdout).to_string()

// Fix — one allocation (no-op if already Owned):
String::from_utf8_lossy(&output.stdout).into_owned()
```

---

### 4.5 NIT — `#[must_use]` missing on `PluginOutput` (`traits.rs:22`)

**Rule:** `api-must-use`  
**File:** `tumult-plugin/src/traits.rs:22`

```rust
#[must_use]
pub struct PluginOutput { ... }
```

---

## 5. `tumult-otel`

### 5.1 NIT — Duplicate `"tumult".to_string()` (`config.rs:16,37`)

**Rule:** `mem-avoid-format`  
**File:** `tumult-otel/src/config.rs:16,37`

```rust
// Fix:
const DEFAULT_SERVICE_NAME: &str = "tumult";

// In Default impl:
service_name: DEFAULT_SERVICE_NAME.to_string()
```

### 5.2 SUGGESTION — `TumultTelemetry` stores full config unnecessarily (`telemetry.rs:28`)

**Rule:** `mem-with-capacity`  
**File:** `tumult-otel/src/telemetry.rs:28`

`TumultTelemetry` stores the entire `TelemetryConfig` at runtime but only ever reads `enabled`, `service_name`, and (for shutdown) nothing. Store only what is used:

```rust
pub struct TumultTelemetry {
    enabled: bool,
    service_name: String,
    tracer_provider: Option<SdkTracerProvider>,
}
```

### 5.3 NIT — `is_enabled()` and `service_name()` missing `#[must_use]` (`telemetry.rs:115,118`)

**Rule:** `api-must-use`

```rust
#[must_use]
pub fn is_enabled(&self) -> bool { ... }

#[must_use]
pub fn service_name(&self) -> &str { ... }
```

---

## 6. `tumult-baseline`

### 6.1 BLOCKING — Import of removed `StatusCode` type (see §1.1)

Already documented above.

### 6.2 IMPORTANT — `cast_precision_loss` in percentile calculation (`acquisition.rs:80`, `stats.rs:24,34,58`)

**Rule:** Clippy `cast_precision_loss`  
**Files:** `tumult-baseline/src/acquisition.rs:80`, `tumult-baseline/src/stats.rs:24,34,58`

`(sorted.len() - 1) as f64` loses precision for collections larger than 2^52 elements (unrealistic in practice but a lint violation). More practically, `data.len() as f64` in `mean()` and `stddev()` is also flagged.

Since these collections are bounded by experiment sample counts (typically < 10,000), this is safe in practice. The correct fix is to acknowledge with `#[allow(clippy::cast_precision_loss)]` and a comment, or use `f64::from` where the cast is from `u32` (infallible):

```rust
// For u32 -> f64 (lossless up to 2^24):
f64::from(ps.errors) / f64::from(ps.total_attempts)

// For usize -> f64 (potentially lossy but bounded in practice):
#[allow(clippy::cast_precision_loss)]  // sample counts bounded < 100_000
data.len() as f64
```

### 6.3 IMPORTANT — `#[non_exhaustive]` on `Method` enum (`tolerance.rs:13`)

**Rule:** `api-non-exhaustive`  
**File:** `tumult-baseline/src/tolerance.rs:13`

`Method::Percentile`, `Method::MeanStddev`, `Method::Iqr` are defined but `Sigma3`, `Iqr2`, and future methods would be breaking additions without `#[non_exhaustive]`.

### 6.4 SUGGESTION — `check_baseline_anomaly` missing `#[must_use]` (`anomaly.rs:22`)

**Rule:** `api-must-use`  
**File:** `tumult-baseline/src/anomaly.rs:22`

```rust
#[must_use]
pub fn check_baseline_anomaly(data: &[f64], min_samples: usize) -> AnomalyCheck { ... }
```

### 6.5 NIT — `ps.values.clone()` to sort a local copy (`acquisition.rs:159`)

**Rule:** `own-borrow-over-clone`  
**File:** `tumult-baseline/src/acquisition.rs:159`

If only the sorted order is needed (not the full cloned `Vec`), collect indices and sort by value, or use a `BTreeMap` at collection time. For small sample sizes this is inconsequential but idiomatic Rust avoids unnecessary clones.

---

## 7. `tumult-ssh`

### 7.1 CRITICAL SECURITY — Host key verification disabled (`session.rs:344–365`)

**Rule:** Security / `err-expect-bugs-only`  
**File:** `tumult-ssh/src/session.rs:344–365`

`ClientHandler::check_server_key` unconditionally returns `Ok(true)`. **Every SSH connection is trivially MITM-able.** The code itself contains a `// SECURITY WARNING` comment acknowledging this.

**Immediate fix (minimum viable):** Add a config knob `allow_unknown_hosts: bool` (default `false`) that panics or returns an error when `false`:

```rust
async fn check_server_key(&mut self, _addr: &SocketAddr, _key: &PublicKey)
    -> Result<bool, SshError>
{
    if self.allow_unknown_hosts {
        tracing::warn!("SSH host key verification disabled — INSECURE");
        return Ok(true);
    }
    Err(SshError::HostKeyVerificationFailed)
}
```

**Full fix:** Implement TOFU (Trust On First Use) with `~/.config/tumult/known_hosts`.

### 7.2 NIT — `cast_possible_wrap` on OTel attributes (`telemetry.rs:58,73,74,84`)

**Rule:** Clippy `cast_possible_wrap`  
**File:** `tumult-ssh/src/telemetry.rs:58,73,74,84`

Casting `u64`/`usize` → `i64` can wrap for values > `i64::MAX`. The OTel API requires `i64`. Use:

```rust
KeyValue::new("ssh.file_bytes",
    i64::try_from(file_bytes).unwrap_or(i64::MAX))
```

### 7.3 NIT — Duplicate match arms (`session.rs:220`)

```rust
// Current:
Some(russh::ChannelMsg::Eof) => {}
None => break,
_ => {}

// Fix: merge Eof into wildcard since both bodies are empty
_ => {}  // includes Eof
```

### 7.4 SUGGESTION — Inconsistent builder pattern in `SshConfig` (`config.rs`)

**Rule:** `api-builder-pattern`  
**File:** `tumult-ssh/src/config.rs`

Constructor functions (`with_key`, `with_agent`) use chainable setters but without a terminal `build()` method or `#[must_use]` on the builder type. Either commit to a full builder (`SshConfigBuilder`) or document that the chainable setters are a deliberate shortcut.

---

## 8. `tumult-analytics`

### 8.1 IMPORTANT — `default_path()` panics on headless systems (`duckdb_store.rs:47`)

**Rule:** `err-expect-bugs-only`  
**File:** `tumult-analytics/src/duckdb_store.rs:47`

```rust
// Current (panics on CI / containers):
dirs_next::home_dir().expect("cannot determine home directory")
```

On headless Linux systems (`/etc/passwd` with no home, Docker containers without a user home), `dirs_next::home_dir()` returns `None` and this panics at startup.

**Fix:**
```rust
pub fn default_path() -> Result<PathBuf, AnalyticsError> {
    let home = dirs_next::home_dir()
        .ok_or(AnalyticsError::HomeDirectoryNotFound)?;
    Ok(home.join(".tumult").join("analytics.duckdb"))
}
```

Add `HomeDirectoryNotFound` to `AnalyticsError`.

### 8.2 IMPORTANT — Stringly-typed enum serialisation into Arrow/DuckDB (`arrow_convert.rs:56,88,89`)

Already documented in §2.3. This is the most impactful cross-cutting issue — database integrity depends on stable serialised enum strings.

### 8.3 IMPORTANT — Timestamp nanosecond `.expect()` in store (`duckdb_store.rs:267`)

**Rule:** `err-expect-bugs-only`  
**File:** `tumult-analytics/src/duckdb_store.rs:267`

```rust
// Current:
chrono::Utc::now().timestamp_nanos_opt().expect("system time before year 2262")
```

While the assumption is reasonable (year 2262 is far off), `.expect()` still terminates the process. Use a documented fallback:

```rust
chrono::Utc::now()
    .timestamp_nanos_opt()
    .unwrap_or(i64::MAX)  // fallback: post-2262; recorded as max timestamp
```

### 8.4 SUGGESTION — Pre-size Arrow column Vecs (`arrow_convert.rs:74–82`)

**Rule:** `mem-with-capacity`  
**File:** `tumult-analytics/src/arrow_convert.rs:74–82`

The total activity count across all phases is known before the loop begins. Pre-allocate:

```rust
let total = journal.method_results.len()
    + journal.rollback_results.len()
    + journal.steady_state_before.as_ref().map_or(0, |h| h.probe_results.len())
    + journal.steady_state_after.as_ref().map_or(0, |h| h.probe_results.len());

let mut names: Vec<String> = Vec::with_capacity(total);
// ... other column vecs with same capacity
```

### 8.5 SUGGESTION — Seal or document `AnalyticsBackend` trait (`backend.rs:16`)

**Rule:** `api-sealed-trait`  
**File:** `tumult-analytics/src/backend.rs:16`

`AnalyticsBackend` is `pub` with no stability promise. External crates implementing it would be broken by any API change. Either seal it (private supertrait) or document it as `#[doc(hidden)]` pending stabilisation.

### 8.6 NIT — `unwrap()` after checked `len() == 1` (`duckdb_store.rs:390`)

**Rule:** `err-no-unwrap-prod`

```rust
// Current:
batches.into_iter().next().unwrap()

// Fix with clear context:
batches.into_iter().next().expect("len==1 asserted above")
```

---

## 9. `tumult-kubernetes`

### 9.1 NIT — Missing `#[non_exhaustive]` on status structs (`probes.rs:16,27,37,45`)

**Rule:** `api-non-exhaustive`  
**File:** `tumult-kubernetes/src/probes.rs`

`PodStatus`, `DeploymentStatus`, `NodeStatus`, `NodeCondition` will grow as the Kubernetes API surface expands. Add `#[non_exhaustive]` to all four.

### 9.2 SUGGESTION — Wrong span name for `uncordon_node` (`actions.rs:77`)

**Rule:** OTel semantic conventions  
**File:** `tumult-kubernetes/src/actions.rs:77`

`uncordon_node` calls `begin_cordon_node` for its OTel span, so the span is named `k8s.node.cordon` even for uncordon operations. This produces misleading telemetry.

**Fix:** Add `begin_uncordon_node` to `tumult-kubernetes/src/telemetry.rs`:
```rust
pub(crate) fn begin_uncordon_node(node: &str) -> SpanGuard {
    k8s_span("k8s.node.uncordon", node)
}
```

### 9.3 NIT — `#[non_exhaustive]` on `DrainResult` (`actions.rs:94`)

**Rule:** `api-non_exhaustive`  
**File:** `tumult-kubernetes/src/actions.rs:94`

`DrainResult` is `pub` — adding a field like `drain_duration_ms` later is breaking. Add `#[non_exhaustive]`.

---

## 10. `tumult-mcp`

### 10.1 HIGH SECURITY — Raw SQL passed to DuckDB from MCP tools (`tools.rs:56,295`)

**Rule:** Security / SQL injection  
**File:** `tumult-mcp/src/tools.rs:56,295`

The `analyze` and `analyze_persistent` MCP tools accept a raw `query` parameter from the AI agent and pass it directly to `DuckDB::query()`. DuckDB supports `COPY`, `ATTACH`, `httpfs`, and arbitrary extension installation. A compromised agent can read `/etc/passwd`, exfiltrate data to S3, or install malicious extensions.

**Fix:**
```rust
fn validate_select_only(query: &str) -> Result<(), String> {
    let normalized = query.trim().to_uppercase();
    if !normalized.starts_with("SELECT") && !normalized.starts_with("WITH") {
        return Err("only SELECT/WITH queries are allowed".into());
    }
    Ok(())
}
```

Apply before every DuckDB query in MCP tools.

### 10.2 HIGH SECURITY — Path traversal in all MCP file tools (`tools.rs`)

**Rule:** Security / path traversal  
**File:** `tumult-mcp/src/tools.rs`

All seven MCP tools that accept file paths pass them to `std::fs` without canonicalization or directory containment checks. An agent can supply `../../etc/shadow`.

**Fix:** Implement and use `safe_resolve_path`:
```rust
fn safe_resolve_path(base: &Path, user_path: &str) -> Result<PathBuf, String> {
    let resolved = base.join(user_path).canonicalize()
        .map_err(|e| format!("invalid path: {e}"))?;
    if !resolved.starts_with(base) {
        return Err(format!("path traversal rejected: {user_path}"));
    }
    Ok(resolved)
}
```

### 10.3 MEDIUM SECURITY — No execution timeout in `ProcessExecutor` (`handler.rs:132–135`)

**Rule:** Security / DoS  
**File:** `tumult-mcp/src/handler.rs:132–135`

`std::process::Command::output()` blocks indefinitely. A hanging script causes the MCP handler thread to block forever — denial of service.

**Fix:** Wrap with `tokio::time::timeout`:
```rust
let output = tokio::time::timeout(
    std::time::Duration::from_secs(PROCESS_TIMEOUT_SECS),
    tokio::process::Command::new(path).args(args).envs(env).output()
).await
    .map_err(|_| McpError::Timeout)?
    .map_err(McpError::Io)?;
```

---

## 11. `tumult-clickhouse`

### 11.1 MEDIUM SECURITY — Raw SQL API surface (`store.rs`)

**Rule:** Security / SQL injection  
**File:** `tumult-clickhouse/src/store.rs`

The public `query()` method accepts raw SQL strings. While current call sites are internal and controlled, the public API surface allows callers to pass user-supplied strings in future.

**Fix:** Make the raw-SQL method `pub(crate)` and provide parameterized query methods for all public-facing operations.

### 11.2 LOW SECURITY — HTTP default URL risks cleartext password (`config.rs:28`)

**Rule:** Security / TLS  
**File:** `tumult-clickhouse/src/config.rs:28`

Default URL is `http://localhost:8123`. When `TUMULT_CLICKHOUSE_PASSWORD` is set with an HTTP URL, the credential is transmitted in cleartext.

**Fix:** Add a warning log when HTTP is used with a non-empty password:
```rust
if url.starts_with("http://") && !password.is_empty() {
    tracing::warn!("ClickHouse password is being sent over HTTP (plaintext)");
}
```

---

## 12. Cross-Cutting Findings

### 12.1 SUGGESTION — Duplicate telemetry guard boilerplate

**Rule:** `proj-pub-crate-internal`  
**Files:** All `*/src/telemetry.rs` modules

Every crate defines a structurally identical guard type:
```rust
pub(crate) struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}
```

This is repeated across `tumult-core`, `tumult-baseline`, `tumult-otel`, `tumult-ssh`, `tumult-kubernetes`, `tumult-analytics`, `tumult-plugin`, `tumult-mcp`. Consider extracting a shared `pub(crate) struct TelemetryGuard` in `tumult-otel` and re-exporting it, or declaring a macro. This reduces ~50 lines of identical boilerplate.

### 12.2 SUGGESTION — OTel `KeyValue::new` always requires `String`

**Rule:** `mem-avoid-format` (documented exemption)  
**Files:** All `telemetry.rs` files

`.to_string()` on `&str` arguments to `KeyValue::new` is unavoidable since the OTel SDK takes `impl Into<Value>` which resolves to `String` for string inputs. This is documented as intentional in `RUST_PATTERNS_AUDIT.md` (finding X-3). No change needed; add a crate-level comment once.

### 12.3 NIT — `"tumult-engine"` tracer name hardcoded in `runner.rs`

**File:** `tumult-core/src/runner.rs:346,427,512`

The tracer name `"tumult-engine"` is hardcoded three times in the same file. Extract as a module-level constant:

```rust
const TRACER_NAME: &str = "tumult-engine";
```

### 12.4 SUGGESTION — CI `cargo audit` gate disabled

**Rule:** Security / Supply chain  
**File:** `.github/workflows/ci.yml:66`

`continue-on-error: true` on the `cargo audit` step means known CVEs never block a PR. The DuckDB bundled C++ dependency and `rust-mcp-sdk` (early-stage) are high-value supply chain targets.

**Fix:** Remove `continue-on-error: true`. If specific advisories must be ignored, use an `audit.toml` with an explicit `ignore = ["RUSTSEC-XXXX-YYYY"]` list.

### 12.5 NIT — GitHub Actions pinned to mutable tags

**File:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`

Actions are pinned to mutable version tags (`actions/checkout@v4`), not commit SHAs. Pin to full SHA for supply-chain integrity:
```yaml
uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683  # v4.2.2
```

---

## 13. Workspace-Level Configuration

### 13.1 Release profile missing LTO and codegen-units settings

**Rule:** `opt-lto-release`, `opt-codegen-units`  
**File:** `Cargo.toml` (workspace root)

The workspace `Cargo.toml` does not define release profile settings. For a production CLI binary:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true

[profile.dev.package."*"]
opt-level = 3  # fast dep compilation in dev
```

### 13.2 `rust-version` is 1.75 but workspace uses 2021 edition features

**File:** `Cargo.toml:33`

`rust-version = "1.75"` is fine for the declared features (2021 edition, `let...else` is stabilised in 1.65). However, Clippy 1.75 does not have all the pedantic lints being triggered. Bumping to `rust-version = "1.80"` aligns better with the actual lint set and provides `LazyLock` (stable replacement for `once_cell::sync::Lazy`).

### 13.3 `tokio = { version = "1", features = ["full"] }` for library crates

**Rule:** `async-tokio-runtime`  
**File:** `Cargo.toml:47`

Library crates (`tumult-core`, `tumult-plugin`, etc.) that inherit the workspace `tokio` dependency get the `"full"` feature set, which includes `tokio::net`, `tokio::fs`, `tokio::signal`, etc., most of which are unused. Library crates should declare minimal tokio features:

```toml
# In tumult-core/Cargo.toml:
tokio = { version = "1", features = ["rt", "macros", "time", "sync"] }
```

The workspace alias `tokio-minimal` exists for this purpose but is underused.

---

## 14. Documentation Quality

| Crate | Module docs `//!` | Public fn `# Errors` | Public fn `# Examples` | `#[must_use]` completeness |
|-------|-------------------|-----------------------|------------------------|----------------------------|
| `tumult-core` | Good | Partial (runner has example, engine missing errors) | Good (runner, engine) | Missing on most query helpers |
| `tumult-cli` | Minimal | Absent | Absent | N/A (binary) |
| `tumult-plugin` | Good | Absent | Absent | Missing on `PluginOutput` |
| `tumult-otel` | Good | Absent | Absent | Missing on getters |
| `tumult-baseline` | Good | Absent on `derive_baseline` | Absent | Missing on `check_baseline_anomaly`, stats functions |
| `tumult-ssh` | Good | Missing on `close()` | Absent | Missing on `config()` getter |
| `tumult-analytics` | Excellent | Absent on most | Absent | Missing throughout |
| `tumult-kubernetes` | Good | Absent | Absent | Missing on probe functions |
| `tumult-mcp` | Minimal | Absent | Absent | Missing on all tools |

**Recommended standard:** Every public `fn` returning `Result` must have `# Errors` section. Every pure function or getter returning a non-trivial value should have `#[must_use]`. See `doc-errors-section`, `doc-examples-section`, `api-must-use` rules.

---

## 15. Testing Assessment

### Strengths

- `tumult-core/src/types.rs` has comprehensive TOON round-trip tests for every public type — excellent coverage.
- `tumult-core/src/runner.rs` has full lifecycle tests including cancellation token, rollback strategies, and hypothesis alternation — good async correctness coverage.
- `tumult-core/tests/audit_findings.rs` and `tests/experiment_integration.rs` provide integration-level validation.
- Test modules use `#[cfg(test)] mod tests {}` and `use super::*` consistently (`test-cfg-test-module`, `test-use-super`).

### Gaps

- No property-based tests (`proptest`) for statistical functions in `tumult-baseline/src/stats.rs`. Edge cases (empty slice, single element, NaN, infinity) should be covered.
- `tumult-cli` tests are integration-level only (e2e_docker, e2e_analytics) — no unit tests for `commands.rs` helper functions. The HTTP provider stub is not tested at all.
- `tumult-mcp/src/tools.rs` has zero test coverage.
- `tumult-analytics/src/arrow_convert.rs` tests are absent — Arrow schema mismatches would only be caught at runtime.
- `tumult-ssh` integration tests require a real SSH server — no mock/in-memory alternative is provided.

---

## 16. Priority-Ordered Action Plan

### Must Fix Before Production Use

1. **Fix `tumult-baseline` build failure** — remove `StatusCode` import, keep `Status::error(...)` call. One-line fix.
2. **Fix SSH host key verification** — add `allow_unknown_hosts` config knob defaulting to `false`. Without this, every SSH session is MITM-vulnerable.
3. **Fix MCP SQL injection** — add `SELECT`-only validation before every DuckDB query call in `tumult-mcp/src/tools.rs`.
4. **Fix MCP path traversal** — implement `safe_resolve_path` and apply to all seven file-operating MCP tools.
5. **Replace blocking sleeps with `tokio::time::sleep`** — `runner.rs:441,491` and `commands.rs:~123` block Tokio worker threads.
6. **Fix `block_on` in async context** — `commands.rs:~249` can deadlock.

### High Priority (Before Load Testing)

7. **Stable enum serialisation** — implement `Display` on `ActivityType`, `ActivityStatus`, `ExperimentStatus`. Affects both OTel traces and DuckDB data integrity.
8. **Fix `default_path()` panic** — return `Result` from `AnalyticsStore::default_path()`.
9. **Add `#[non_exhaustive]`** to all public enums and structs listed in §2.4.
10. **Fix plugin trait allocation** — change `TumultPlugin::actions()/probes()` to return `&[T]`.
11. **Enable `cargo audit` gate** — remove `continue-on-error: true` from CI.
12. **Fix `DecodeError` variant** — add `JournalError::DecodeError` and use it in `read_journal`.

### Improvement Backlog

13. Cache compiled regexes in tolerance evaluation.
14. Implement `safe_resolve_path` for plugin script path validation (`PATH-02` from SECURITY-AUDIT.md).
15. Fix `cast_possible_wrap` in `tumult-ssh/src/telemetry.rs`.
16. Add `# Errors` sections to all public fallible functions.
17. Add `#[must_use]` to all pure getters and query functions.
18. Add `proptest` coverage for `tumult-baseline/src/stats.rs`.
19. Add unit tests for `tumult-mcp/src/tools.rs`.
20. Pin GitHub Actions to full commit SHAs.

---

## Appendix A — Clippy Error Summary (pedantic, `-D warnings`)

Total errors under `cargo clippy --all-targets -- -D warnings -W clippy::pedantic`: **159**

| Category | Count | Top Files |
|----------|-------|-----------|
| `must_use_candidate` | 23 | `tumult-baseline/stats.rs`, `tumult-otel/telemetry.rs`, `tumult-ssh/session.rs` |
| `cast_precision_loss` | 8 | `tumult-baseline/acquisition.rs`, `tumult-baseline/stats.rs` |
| `cast_possible_truncation` | 4 | `tumult-baseline/acquisition.rs`, `tumult-baseline/stats.rs` |
| `cast_possible_wrap` | 4 | `tumult-ssh/telemetry.rs` |
| `uninlined_format_args` | 8 | `tumult-ssh/session.rs`, `tumult-mcp/tools.rs` |
| `doc_markdown` | 11 | Various telemetry modules |
| `missing_errors_doc` | 5 | `tumult-baseline/acquisition.rs`, `tumult-ssh/session.rs` |
| `cast_lossless` | 7 | `tumult-baseline/acquisition.rs`, `tumult-baseline/stats.rs` |
| `match_same_arms` | 2 | `tumult-ssh/session.rs` |
| `strict_comparison_f32_f64` | 7 | `tumult-core/engine.rs` (tolerance range checks) |
| Other | 80 | Distributed across workspace |

**Crates failing to compile under pedantic:** `tumult-baseline` (34 errors), `tumult-otel` (12 errors), `tumult-ssh` (26 errors + 3 real issues).

---

## Appendix B — Compliance Against Rust Patterns Skill (179 Rules)

| Category | Compliance | Key Gaps |
|----------|-----------|---------|
| Ownership & Borrowing | 85% | `own-slice-over-vec` in plugin trait, clone in acquisition sort |
| Error Handling | 75% | `err-custom-type` (decode variant), `err-no-unwrap-prod` (duckdb_store), `err-doc-errors` |
| Memory Optimization | 80% | `mem-with-capacity` in arrow_convert, `mem-avoid-format` in executor |
| API Design | 65% | `api-non-exhaustive` (sweep needed), `api-must-use` (many missing), `api-builder-pattern` (ssh) |
| Async/Await | 60% | `async-spawn-blocking` (2 locations), `async-no-lock-await` (1 location) |
| Compiler Optimization | 70% | Release profile lacks LTO/codegen-units |
| Naming Conventions | 95% | Minor: `name-no-get-prefix` on one getter |
| Type Safety | 70% | `type-newtype-ids` for trace/span/experiment IDs |
| Testing | 80% | Missing proptest, missing MCP tests, missing Arrow conversion tests |
| Documentation | 60% | Missing `# Errors`, `#[must_use]`, inconsistent coverage |
| Performance Patterns | 85% | Regex recompilation is the key hot-path issue |
| Project Structure | 90% | tokio "full" features in library crates |
| Clippy & Linting | 40% | 159 pedantic errors; `lint-rustfmt-check` passes |
| Anti-patterns | 75% | Debug serialisation for DB storage is the critical anti-pattern |

**Overall compliance: ~74%** against the 179-rule Rust Patterns skill.

---

## Appendix C — Files Reviewed

```
tumult-core/src/{controls,engine,execution,journal,lib,runner,types}.rs
tumult-cli/src/{commands,lib,main}.rs
tumult-plugin/src/{discovery,executor,lib,manifest,registry,telemetry,traits}.rs
tumult-otel/src/{attributes,config,instrument,lib,metrics,telemetry}.rs
tumult-baseline/src/{acquisition,anomaly,lib,stats,telemetry,tolerance}.rs
tumult-ssh/src/{config,error,lib,session,telemetry}.rs
tumult-analytics/src/{arrow_convert,backend,duckdb_store,error,export,lib,telemetry}.rs
tumult-kubernetes/src/{actions,error,lib,probes,telemetry}.rs
tumult-mcp/src/{handler,lib,main,telemetry,tools}.rs
tumult-clickhouse/src/{config,error,lib,store,telemetry}.rs
Cargo.toml (workspace)
RUST_PATTERNS_AUDIT.md (prior audit, incorporated)
SECURITY-AUDIT.md (security audit, incorporated)
```
