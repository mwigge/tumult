# Tumult Security Assessment

**Date:** 2026-06-08 (tumult 1.3.0)  
**Scope:** Full workspace — 13 crates (incl. `tumult-agentic`, `tumult-otel`, `tumult-intelligence`), 10 script plugins, the agentic fault-injection proxy, and Docker infrastructure  
**Tools:** cargo-audit, cargo-geiger, manual source review  
**References:** [Rust Foundation Security Initiative](https://rustfoundation.org/security-initiative/), [RustSec Advisory Database](https://rustsec.org/), [OpenCVE Rust](https://app.opencve.io/cve/?vendor=rust-lang)

---

## Executive Summary

The Tumult codebase has **zero unsafe code in production**, **zero known HIGH/CRITICAL vulnerabilities**, and **zero hardcoded credentials**. The primary risk surface is transitive dependency maintenance (5 unmaintained crates via `toon-format`) and the inherent trust boundary at script plugin execution.

| Category | Finding | Severity |
|----------|---------|----------|
| Unsafe code (our crates) | 0 `unsafe` blocks in production code | None |
| `.unwrap()` in production | 0 (all 492 occurrences are in test code or doc examples) | None |
| SQL injection | 0 string-formatted SQL queries | None |
| Command injection | Null-byte validation on arguments; process path from experiment definition, not user input | Low |
| Hardcoded secrets | 0 | None |
| Credential files in repo | 0 | None |
| TLS verification bypass | 0 | None |
| Path traversal | 0 | None |
| cargo-audit advisories | 5 warnings (all transitive, all unmaintained/unsound — no active exploits) | Low |

---

## 1. cargo-audit Results

### Advisory Summary

| Crate | Version | Advisory | Type | Severity | Direct? |
|-------|---------|----------|------|----------|---------|
| `bincode` | 1.3.3 | RUSTSEC-2025-0141 | Unmaintained | Low | No — via `syntect` -> `toon-format` |
| `paste` | 1.0.15 | RUSTSEC-2024-0436 | Unmaintained | Low | No — via `ratatui` -> `toon-format` |
| `yaml-rust` | 0.4.5 | RUSTSEC-2024-0320 | Unmaintained | Low | No — via `syntect` -> `toon-format` |
| `rustls-pemfile` | 2.2.0 | RUSTSEC-2025-0134 | Unmaintained | Low | No — via `axum-server` -> `rust-mcp-sdk` |
| `lru` | 0.12.5 | RUSTSEC-2026-0002 | Unsound (`IterMut` Stacked Borrows) | Medium | No — via `ratatui` -> `toon-format` |

### Risk Assessment

- **No HIGH or CRITICAL advisories.** All 5 are transitive dependencies.
- **4 of 5 trace through `toon-format`** (the TOON parser). Upstream `toon-format` owns these dependency choices.
- **`lru` unsoundness (RUSTSEC-2026-0002)** is the most notable: a Stacked Borrows violation in `IterMut`. Tumult does not use `lru` directly — it's pulled in by `ratatui` (TUI rendering in `toon-format`). The `IterMut` API is not exercised in our usage path. Risk: **theoretical, not exploitable in Tumult's context.**
- **`rustls-pemfile`** is pulled by the MCP SDK's HTTP server. It handles PEM certificate parsing. While unmaintained, no active CVEs exist against it.

### Remediation

- Monitor `toon-format` for dependency updates (primary vector for 4 of 5 advisories)
- Consider `Cargo.toml` `[patch]` overrides if upstream is slow to update
- Track `lru` for a fixed release addressing RUSTSEC-2026-0002

---

## 2. Unsafe Code Analysis

### Our Crates: Zero Unsafe

```
tumult-core:       0 unsafe blocks
tumult-cli:        0 unsafe blocks
tumult-analytics:  0 unsafe blocks
tumult-otel:       0 unsafe blocks
tumult-plugin:     0 unsafe blocks
tumult-ssh:        0 unsafe blocks
tumult-clickhouse: 0 unsafe blocks
tumult-mcp:        0 unsafe blocks
tumult-baseline:   0 unsafe blocks
tumult-kubernetes: 0 unsafe blocks
tumult-test-utils: 0 unsafe blocks
```

The single reference to `unsafe` in the codebase is a comment in `tumult-core/src/runner.rs:563` explaining why a safe pattern was chosen over an unsafe alternative.

### Dependencies with Unsafe

Unsafe code exists in transitive dependencies (expected for systems crates):
- `libduckdb-sys` — FFI bindings to DuckDB C library (required for embedded analytics)
- `russh` — SSH protocol implementation (uses unsafe for crypto primitives)
- `opentelemetry` SDK internals
- `tokio` runtime internals

These are well-audited, widely-used crates. The unsafe usage is appropriate for their function (FFI, crypto, async runtime).

---

## 3. `.unwrap()` Analysis

| Location | Count | Assessment |
|----------|-------|-----------|
| Test code (`#[cfg(test)]`, `tests/`) | 487 | Acceptable — panics in tests are expected |
| Doc examples (`///`) | 5 | Acceptable — illustrative code |
| Production code | 0 | Clean |

All `.expect()` calls (12 total) are in test code.

---

## 4. Injection Surface Analysis

### Command Execution

Tumult executes external processes in three places:

1. **`tumult-plugin/src/executor.rs:112`** — Script plugin execution via `/bin/sh`
   - **Input:** Script path from plugin manifest (`plugin.toon`), arguments from experiment definition
   - **Validation:** Null-byte check on all arguments (`validate_arguments`)
   - **Mitigation:** Scripts are pre-registered via discovery, not user-supplied at runtime. Arguments pass through environment variables (`TUMULT_*` prefix), not command-line interpolation.

2. **`tumult-cli/src/commands.rs:109`** — Process provider execution
   - **Input:** `path` and `arguments` from experiment `.toon` file
   - **Validation:** Null-byte check inherited from core types
   - **Mitigation:** The experiment file is authored by the operator, not external input.

3. **`tumult-mcp/src/handler.rs:169`** — MCP tool execution
   - **Input:** Experiment path from MCP client
   - **Risk:** MCP clients can specify arbitrary experiment paths
   - **Mitigation:** MCP server runs locally; authentication required via MCP protocol

### SQL Queries

- **Zero string-formatted SQL.** All DuckDB queries use the `tumult analyze --query` CLI flag, which passes the query string directly to DuckDB without interpolation.
- The DuckDB store uses parameterized inserts via Arrow record batches, not SQL string construction.

### Deserialization

- **2 `serde_json::from_str` calls** in production:
  1. `tumult-core/src/runner.rs:450` — Parses probe output for tolerance evaluation. Input is stdout from a subprocess we spawned.
  2. `tumult-plugin/src/lib.rs:103` — Parses plugin manifest. Input is a file on disk authored by the plugin developer.
- Both deserialize into `serde_json::Value` (generic), not into types with custom `Deserialize` implementations that could trigger logic bugs.

---

## 5. Integer Cast Analysis

30 `as` casts in production code. All are in safe contexts:

| Pattern | Count | Risk |
|---------|-------|------|
| `count as usize` (DuckDB row counts) | 4 | None — DuckDB returns positive integers |
| `elapsed().as_millis() as u64` (timing) | 2 | None — durations are always positive |
| `count as u64` (OTel gauge values) | 6 | None — counters are positive |
| `float.floor() as usize` (percentile index) | 4 | Low — bounded by input array length |
| `samples as usize` (statistics) | 2 | None — sample counts are positive |
| Other metric/gauge casts | 12 | None — all positive bounded values |

No truncation risk. No user-controlled values in cast expressions.

---

## 6. Credential and Secret Handling

- **Zero hardcoded credentials** in source code
- **`resolve_secrets()` in `tumult-cli`** reads secrets from environment variables at runtime, not from files
- **SSH keys** are handled by `tumult-ssh` via `russh` — keys are loaded from paths specified in experiment definitions, never embedded
- **ClickHouse** connection strings use environment variables (`CLICKHOUSE_ENDPOINT`, `TUMULT_CLICKHOUSE_URL`)
- **Docker test infrastructure** uses non-production credentials (`tumult_test` / `tumult_test`) in `docker-compose.yml` — appropriate for test fixtures
- **No `.env` files** in the repository

---

## 7. Supply Chain Assessment

### Direct Dependencies (Cargo.toml)

| Crate | Purpose | Maintenance | Last Updated |
|-------|---------|-------------|--------------|
| `tokio` | Async runtime | Active | 2026 |
| `opentelemetry` | Telemetry | Active | 2026 |
| `duckdb` | Embedded analytics | Active | 2026 |
| `arrow` | Columnar data | Active (Apache) | 2026 |
| `russh` | SSH protocol | Active | 2026 |
| `kube` | Kubernetes client | Active | 2026 |
| `clap` | CLI parsing | Active | 2026 |
| `thiserror` | Error types | Active | 2026 |
| `anyhow` | Error handling | Active | 2026 |
| `serde` / `serde_json` | Serialization | Active | 2026 |
| `toon-format` | TOON parser | Active | 2026 |
| `rust-mcp-sdk` | MCP server | Active | 2026 |
| `clickhouse` | ClickHouse client | Active | 2026 |

### Total Dependency Tree

- **675 crate dependencies** in `Cargo.lock`
- **5 advisories** (all Low/Medium, all transitive)
- **0 actively exploited vulnerabilities**

---

## 8. Script Plugin Security

Script plugins execute shell scripts as subprocesses. Security boundaries:

| Control | Implementation |
|---------|---------------|
| Null-byte injection | Validated in `validate_arguments()` |
| Argument passing | Via `TUMULT_*` env vars, not shell interpolation |
| Timeout enforcement | `tokio::time::timeout` with `kill_on_drop(true)` |
| Output capture | stdout/stderr captured, not re-executed |
| W3C trace context | Injected as `TRACEPARENT`/`TRACESTATE` env vars (read-only) |
| Plugin discovery | Scripts must be in registered plugin directories with `plugin.toon` manifest |

### Risk: Script Content

Tumult trusts the content of plugin scripts. A malicious `plugin.toon` + script could execute arbitrary commands. This is by design — script plugins are the extensibility mechanism, similar to how `kubectl` plugins or Git hooks work.

**Mitigations:**
- Plugin directories are configured, not auto-discovered from arbitrary paths
- Scripts require execute permission
- All script output is logged and captured in journals

---

## 9. Docker Infrastructure Security

| Item | Status |
|------|--------|
| Docker socket exposure | Mounted read-only (`:ro`) where needed |
| Container networking | Isolated `tumult-e2e` network |
| ClickHouse auth | Default user, no password (test only — not for production) |
| SSH test container | Key-based auth only, `PasswordAuthentication no` |
| Image pinning | All images pinned to specific versions |
| Resource limits | ClickHouse: 4 CPU / 4GB, Collector: 2 CPU / 1GB |

---

## 10. Agentic Fault Injection Security

The `tumult-agentic` proxy (`tumult agentic proxy`) and orchestrator (`tumult
agentic run-live`) introduce a new, deliberately privileged surface: they sit
in the path between a real coding agent and its model provider. This section
documents that trust boundary and its mitigations.

### 10.1 Trust boundary — credential and payload forwarding

| Concern | Behavior | Mitigation |
|---------|----------|------------|
| **API credentials** | The proxy is a transparent forwarder: it does **not** strip `Authorization` / `x-api-key`, so the agent's real provider credentials flow *through* tumult to the configured `--upstream`. | The proxy is operator-run and binds to `127.0.0.1` by default; it forwards only to the single `--upstream` you configure (no open relay). Treat it like any local TLS-terminating dev proxy. |
| **Prompts / completions** | Request and response bodies pass through tumult in memory (some are mutated for fault injection). | **Metadata-only capture is the default** — no prompt, completion, tool payload, or body content is written to the journal or telemetry. Only counts, durations, fault types, and contract verdicts are recorded. |
| **Egress** | The proxy forwards to `--upstream` only; it never fans out to other hosts. | Operator controls the upstream. `accept-encoding`, `host`, and `content-length` are stripped/managed; inbound `traceparent`/`tracestate` are replaced with tumult's own. |

**Operator guidance:** point a client at the proxy only for throwaway/test work,
keep the listener on `127.0.0.1`, and never expose the proxy port to a network.
Pointing a production client at it means trusting the local tumult process with
that session's credentials and traffic.

### 10.2 `run-live` subprocess

`tumult agentic run-live` spawns `claude -p` with a constructed environment
(`TRACEPARENT`, `CLAUDE_CODE_*` telemetry flags, `ANTHROPIC_BASE_URL`,
`OTEL_EXPORTER_OTLP_ENDPOINT`). It sets only these well-known variables, inherits
the operator's environment, and passes the prompt via stdin — never as an
argument, since argv is visible in `ps` to other local users — with no shell
interpolation. The agent binary itself (`claude`) is trusted operator-installed
tooling, run with the operator's own credentials.

### 10.3 Telemetry privacy

All agentic spans/metrics follow the `metadata_only` capture policy by default
(`resilience.agent.payload.capture_policy = metadata_only`). W3C trace context
is propagated, but the W3C spec carries no payload. The OTLP collector config
(`collector/otel-agentic-normalization.yaml`) only tags and renames attributes;
it does not capture content. Operators who opt into content capture (client-side
`OTEL_LOG_*` flags) must ensure their backend is approved for that data.

### 10.4 Surface unchanged

The proxy adds no `unsafe`, no new credential storage, and no new network
listeners beyond the operator-launched proxy port. The MCP tool-surface span
wrapping added in 1.3.0 does not change MCP authentication.

---

## 11. Prompt injection and agent-driven operation

The recommendation and autopilot features close a loop that deserves its own
threat model: content from experiment journals influences a prompt, the prompt
drives a model, and the model's output can become an executable experiment.

### 11.1 The indirect-injection chain

1. **Journals become prompt material.** `tumult_recommend --agent` assembles
   its prompt from analytics-store content: journal titles, experiment
   metadata, failure reasons, and coverage gaps. Anything that previously ran
   — including an experiment whose definition contained attacker-influenced
   strings — contributes to that prompt.
2. **The prompt drives an external agent.** The configured agent CLI
   (`claude`, `codex`) answers with an experiment definition. Model output is
   not trustworthy by construction: a crafted journal entry can steer the
   generated target, plugin action, or arguments (classic indirect prompt
   injection).
3. **The output is executable.** A generated experiment file can be run by
   `tumult_run_experiment` directly, or enter the autopilot queue and later
   be enacted by `tumult_autopilot_run` / approved via
   `tumult_autopilot_respond`.

### 11.2 What validation does and does not cover

Generated experiments pass **structural validation only**: the TOON parser,
the experiment validator (steady-state hypothesis present, exactly one
fault, rollback declared), and — on the autopilot path — the deterministic
policy gate (enrollment, blast-radius bounds, concurrency, cooldown, guard
telemetry pre-flight). These checks establish that an experiment is
well-formed and within the operator's declared policy. They do **not**
establish that its target or arguments are semantically appropriate: a
well-formed experiment can still be the wrong experiment.

### 11.3 Required operational controls

- **Review before execution.** An operator must read any agent-generated
  experiment before it runs. Generation and execution are separate tools
  precisely so this review has a place to happen.
- **Human-in-the-loop by name, not by annotation.** MCP clients must gate
  the destructive tools on explicit human approval, matching on tool name:
  `tumult_run_experiment`, `tumult_gameday_run`, `tumult_autopilot_run`
  (when `execute=true`), and `tumult_autopilot_respond` (when
  `approve=true`). Annotations are advisory metadata for the client UI; they
  are not an enforcement mechanism, and a compromised or confused client
  cannot be assumed to honor them.
- **Server-side backstops.** Independent of client behavior, the server
  requires the operator role for every destructive tool, re-evaluates the
  full autopilot gate against current state before any approval executes
  (a stale approval is refused and the refusal is recorded), and holds a
  server-wide enactment lock so at most one fault-injection enactment runs
  at a time.

None of this removes the prompt-injection risk at the model boundary; it
bounds what a successful injection can reach. Treat journal content as
untrusted input to the model, and model output as untrusted input to the
runner.

---

## 12. Recommendations

### Immediate (P0)

None required. No active vulnerabilities.

### Short-term (P1)

1. **Pin `toon-format` to a version that updates `lru`** when RUSTSEC-2026-0002 is fixed upstream
2. **Add `cargo-deny`** to CI for license compliance and duplicate dependency detection
3. **Add `SECURITY.md`** to the repository with responsible disclosure instructions

### Medium-term (P2)

4. **Run `cargo-geiger`** in CI to track unsafe usage in the dependency tree over time
5. **Add input validation for MCP experiment paths** — reject paths outside the workspace
6. **Consider `seccomp` profiles** for Docker containers in production deployments
7. **Add `Cargo.toml` `[lints]` section** to enforce `clippy::undocumented_unsafe_blocks` workspace-wide

### Long-term (P3)

8. **Fuzz testing** for TOON parser and tolerance evaluation (untrusted input paths)
9. **Miri CI job** for detecting undefined behavior in test suite
10. **SBOM generation** (CycloneDX or SPDX) for supply chain transparency
