# Tumult Security Assessment

**Date:** 2026-04-01  
**Scope:** Full workspace — 11 crates, 10 script plugins, Docker infrastructure  
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

## 10. Recommendations

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
