# Tumult Platform — Security Audit Report

**Date:** 2026-03-31  
**Scope:** All Rust crates, all shell plugin scripts (50 files), CI/CD workflows  
**Auditor:** Security-engineer skill (OpenCode)

---

## Executive Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 1 |
| HIGH     | 5 |
| MEDIUM   | 7 |
| LOW      | 8 |

The most severe finding is that SSH host key verification is unconditionally disabled, turning every SSH connection into a trivially exploitable man-in-the-middle target. The second tier of HIGH findings centers on unsandboxed SQL (DuckDB and ClickHouse both accept raw user-supplied queries), unsandboxed file paths in the MCP layer, and a `cargo audit` gate that is configured to never block. These four areas should be addressed before this tool is used against production infrastructure.

The shell plugin layer is generally well-written — validate.sh is used consistently across database plugins, and credentials are handled via .pgpass/REDISCLI_AUTH/MYSQL_PWD rather than command-line flags. The process, network, Redis, and stress plugins have some gaps detailed below.

---

## Category 1 — SSH Host Key Verification

### SSH-01 · **CRITICAL** — Host key verification unconditionally disabled

**File:** `tumult-ssh/src/session.rs:344–365`

`ClientHandler::check_server_key` always returns `Ok(true)`, accepting any host key from any server. The code itself contains a self-labelled `// SECURITY WARNING` comment acknowledging this is "NOT acceptable for production use." Any SSH connection made by Tumult is trivially intercepted: an attacker with network access between the operator and the target host can impersonate the target, observe all commands sent, capture credentials, and inject arbitrary output.

```rust
// Current (DANGEROUS):
async fn check_server_key(…) -> Result<bool, …> {
    Ok(true)   // accepts ANYTHING
}
```

**Recommended fix:**
1. Add a `known_hosts_path: Option<PathBuf>` field to `SshConfig`.
2. Implement TOFU (Trust On First Use): on first connection, record the host key fingerprint under `~/.config/tumult/known_hosts`; on subsequent connections, verify and reject changes.
3. Expose `--known-hosts` CLI flag and document it.
4. At minimum, add a hard config knob `allow_unknown_hosts: bool` (default `false`) so users who need TOFU can opt in explicitly rather than silently.

---

## Category 2 — SQL Injection / Arbitrary Query Execution

### SQL-01 · **HIGH** — MCP tools pass raw SQL to DuckDB without sandboxing

**File:** `tumult-mcp/src/tools.rs:56` (`analyze`), `tumult-mcp/src/tools.rs:295` (`analyze_persistent`)

Both MCP tool handlers accept a `query` parameter from the MCP caller (an AI agent acting on behalf of a user) and pass it directly to `DuckDB::query()` / `DuckDB::query_columns()`. DuckDB supports `COPY`, `ATTACH`, `httpfs` file-reading, and `INSTALL`/`LOAD` extension installation. A malicious or compromised MCP caller can:

- Read arbitrary files: `SELECT * FROM read_csv('/etc/passwd')`
- Write files: `COPY (SELECT ...) TO '/tmp/exfil.txt'`
- Attach external databases: `ATTACH '/sensitive/path.db'`
- Install extensions: `INSTALL httpfs; LOAD httpfs; SELECT * FROM read_parquet('s3://...')`

**Recommended fix:**
- Restrict queries to `SELECT` only (reject anything that doesn't start with `SELECT` after whitespace stripping, case-insensitive).
- Alternatively, use DuckDB's `readonly` connection mode.
- Document the limitation in the tool description so MCP callers know only read queries are supported.

### SQL-02 · **HIGH** — CLI `--query` passes raw SQL to DuckDB

**File:** `tumult-cli/src/commands.rs:499–501`

The `analyze --query` subcommand passes the user-supplied string directly to DuckDB with no restrictions. Same attack surface as SQL-01, but from the local CLI rather than MCP. Local CLI users are somewhat more trusted, but this is still an injection risk if experiment files or configuration contain user-controlled content that ends up in a query string.

**Recommended fix:** Apply the same `SELECT`-only restriction as SQL-01, or document that this is a power-user feature with full DuckDB access.

### SQL-03 · **MEDIUM** — ClickHouse `query()` accepts raw SQL

**File:** `tumult-clickhouse/src/store.rs` (all query callsites via the ClickHouse HTTP client)

The ClickHouse store's public API accepts raw SQL strings. While current call sites are controlled (no user input flows in), the public API surface accepts arbitrary strings. Future contributors could inadvertently pass user input.

**Recommended fix:** Parameterize all queries and make the raw-SQL method `pub(crate)` rather than `pub`.

---

## Category 3 — Path Traversal

### PATH-01 · **HIGH** — MCP tools accept arbitrary file paths with no containment checks

**File:** `tumult-mcp/src/tools.rs` — `validate_experiment`, `run_experiment`, `read_journal`, `list_journals`, `store_stats`, `analyze_persistent`, `create_experiment`

All seven MCP tools that operate on file paths accept raw string paths from the MCP caller and pass them directly to file system operations (`std::fs::read`, `write`, etc.) or `Path::new(path)`. No canonicalization, no directory containment check. An MCP caller (e.g., a compromised AI agent) can supply `../../etc/passwd`, `/etc/shadow`, or absolute paths to any file on the system.

**Recommended fix:** Implement a `safe_resolve_path` helper that canonicalizes and checks containment within a configured base directory (e.g., `~/.config/tumult/journals/`). Apply it at every MCP path entry point.

```rust
fn safe_resolve_path(base: &Path, user_path: &str) -> Result<PathBuf, McpError> {
    let resolved = base.join(user_path).canonicalize()
        .map_err(|_| McpError::InvalidPath(user_path.to_string()))?;
    if !resolved.starts_with(base) {
        return Err(McpError::PathTraversal(user_path.to_string()));
    }
    Ok(resolved)
}
```

### PATH-02 · **MEDIUM** — Plugin manifest `script` paths not validated against plugin directory

**File:** `tumult-plugin/src/manifest.rs`, `tumult-plugin/src/discovery.rs`

`ScriptAction.script` and `ScriptProbe.script` are `PathBuf` fields parsed from the manifest with no validation. `discovery.rs` joins them to the plugin directory but does not check that the canonical result stays within the plugin directory. A malicious plugin manifest can set `script: ../../bin/bash` to execute arbitrary binaries.

**Recommended fix:** After joining the plugin base directory with the script path, canonicalize and assert containment (same pattern as PATH-01 fix).

---

## Category 4 — Unsafe Code

### UNSAFE-01 · **Low** (informational) — No `unsafe` blocks found

The entire Rust codebase is `unsafe`-free. Zero `unsafe` blocks, zero `transmute`, zero `unsafe impl Send/Sync`. This is a positive finding.

---

## Category 5 — Credential Handling

### CRED-01 · **MEDIUM** — SSH command content recorded in OTel spans

**File:** `tumult-ssh/src/telemetry.rs:29–48`

`begin_execute()` records the full command string (first 256 chars) as the `ssh.command` span attribute:

```rust
attributes: vec![KeyValue::new("ssh.command", cmd_preview)],
```

If commands contain credentials, tokens, or secrets (e.g., `curl -H 'Authorization: Bearer <token>' ...`), they are exported to the configured OTLP endpoint, potentially persisted in Jaeger/Tempo/etc., and visible to anyone with read access to the telemetry backend.

**Recommended fix:** Remove `ssh.command` from span attributes, or replace with a command category/type label (`ssh.command.type = "curl"`) that contains no argument values.

### CRED-02 · **LOW** — ClickHouse URL defaults to HTTP (plaintext credentials)

**File:** `tumult-clickhouse/src/config.rs:28`

Default URL is `http://localhost:8123`. If `TUMULT_CLICKHOUSE_PASSWORD` is set and the URL is HTTP, the password is transmitted in plaintext. Most ClickHouse deployments are local/internal, but documentation should warn about this.

**Recommended fix:** Document that production deployments should configure `TUMULT_CLICKHOUSE_URL=https://...`. Optionally emit a warning log if the URL is HTTP and a password is set.

### CRED-03 · **LOW** — `exhaust-connections.sh` missing trap before PGPASSFILE creation

**File:** `plugins/tumult-db-postgres/actions/exhaust-connections.sh:32–34`

The `PGPASS_FILE` is created with `mktemp`, written to, and `chmod 600`'d — but the `trap cleanup EXIT INT TERM` is set *after* the file is created. If the process is killed between `mktemp` and the trap registration, the credentials file is orphaned on disk.

**Recommended fix:** Register the trap immediately after `mktemp`, before writing credentials:

```sh
PGPASS_FILE=$(mktemp)
trap "rm -f ${PGPASS_FILE}" EXIT INT TERM   # move here
echo "*:*:*:*:${TUMULT_PG_PASSWORD:-}" > "${PGPASS_FILE}"
chmod 600 "${PGPASS_FILE}"
```

---

## Category 6 — Command / Shell Injection

### INJECT-01 · **LOW** — `partition-host.sh` does not validate `TUMULT_TARGET_IP`

**File:** `plugins/tumult-network/actions/partition-host.sh:10,27`

`TARGET_IP` is used directly in `iptables -A INPUT -s "${TARGET_IP}" -j DROP`. No format validation is performed. If the value contains shell metacharacters (e.g., `; rm -rf /`) they could be injected into the iptables invocation if the shell expands them.

In practice `set -eu` and double-quoting `"${TARGET_IP}"` prevent word-splitting and globbing, so the immediate risk is low — iptables will reject a malformed IP address at its own argument parsing level. However, adding explicit IP validation aligns with the principle of defense in depth.

**Recommended fix:** Add a `validate_ip_address` function to `validate.sh`:
```sh
validate_ip_address() {
    case "$2" in
        *[!0-9.]*) echo "error: $1 must be a valid IP address, got: $2" >&2; exit 1;;
    esac
}
```
Apply to `TUMULT_TARGET_IP` in `partition-host.sh` and `partition-broker.sh`.

### INJECT-02 · **LOW** — `block-dns.sh` does not validate `TUMULT_DNS_PORT`

**File:** `plugins/tumult-network/actions/block-dns.sh:9,22`

`DNS_PORT` is used in `iptables --dport "${DNS_PORT}"` with no validation. Same risk as INJECT-01.

**Recommended fix:** Add `validate_integer "TUMULT_DNS_PORT" "${DNS_PORT}"` (validate.sh already provides this).

### INJECT-03 · **LOW** — `kill-broker.sh` does not validate `TUMULT_SIGNAL`

**File:** `plugins/tumult-kafka/actions/kill-broker.sh:11,33`

`SIGNAL` is interpolated directly into `kill -s "${SIGNAL}"` with no allowlist check, unlike the well-implemented signal validation in `kill-process.sh`. A malicious value could pass unexpected arguments to `kill`.

**Recommended fix:** Add the same allowlist check used in `kill-process.sh:16–19`.

### INJECT-04 · **LOW** — `combined-stress.sh` does not source `validate.sh`

**File:** `plugins/tumult-stress/actions/combined-stress.sh`

This script uses `TUMULT_CPU_WORKERS`, `TUMULT_CPU_LOAD`, `TUMULT_VM_WORKERS`, `TUMULT_HDD_WORKERS`, and `TUMULT_TIMEOUT` as arguments to `stress-ng` without validation. The other stress scripts (`cpu-stress.sh`, `memory-stress.sh`, `io-stress.sh`) all validate their integer parameters.

**Recommended fix:** Add `. "$(dirname "$0")/../../lib/validate.sh"` and validate all integer parameters.

### INJECT-05 · **LOW** — `fill-disk.sh` does not validate `TUMULT_TOPIC` or `TUMULT_RETENTION_MS`

**File:** `plugins/tumult-kafka/actions/fill-disk.sh:13,14`

`TOPIC` and `RETENTION_MS` are used as CLI arguments to `kafka-configs` without validation. Not a direct injection risk (double-quoting prevents splitting), but `TOPIC` with metacharacters would silently pass to Kafka rather than being rejected early.

**Recommended fix:** Add `validate.sh` sourcing and `validate_identifier "TUMULT_TOPIC"` + `validate_integer "TUMULT_RETENTION_MS"`.

---

## Category 7 — Input Validation

### VALID-01 · **MEDIUM** — Process action scripts do not validate `TUMULT_PID`

**Files:** `plugins/tumult-process/actions/kill-process.sh:23`, `suspend-process.sh:12`, `resume-process.sh:12`

`TUMULT_PID` is passed directly to `kill -s SIGNAL "${TUMULT_PID}"` without verifying it is a valid positive integer. Providing a non-numeric PID causes a shell error but also exposes the error message; providing `1` kills the init process if the script runs as root.

**Recommended fix:** Add validation: `echo "${TUMULT_PID}" | grep -qE '^[0-9]+$' || { echo "error: PID must be a positive integer" >&2; exit 1; }`  
Also consider an upper-bound sanity check (e.g., reject PID 1 explicitly, reject PIDs outside system range).

### VALID-02 · **MEDIUM** — Redis scripts do not source `validate.sh`; `TUMULT_DURATION` not validated

**Files:** `plugins/tumult-db-redis/actions/block-clients.sh:13`, `simulate-failover.sh:13`

`TUMULT_DURATION` is used as a direct argument to `redis-cli CLIENT PAUSE "${DURATION}"` without integer validation. A non-numeric value produces an unhelpful Redis error. All Redis scripts also skip sourcing `validate.sh` entirely — this inconsistency should be resolved.

**Recommended fix:** Source `validate.sh` in all Redis scripts and add `validate_integer "TUMULT_DURATION" "${DURATION}"`.

### VALID-03 · **MEDIUM** — `add-broker-latency.sh` does not validate `TUMULT_KAFKA_PORT`

**File:** `plugins/tumult-kafka/actions/add-broker-latency.sh:15,25`

`KAFKA_PORT` is used in a `tc filter` match without validation.

**Recommended fix:** Add `validate_integer "TUMULT_KAFKA_PORT" "${KAFKA_PORT}"`.

### VALID-04 · **LOW** — `consumer-lag.sh` / `under-replicated.sh` / `broker-count.sh` do not validate `TUMULT_CONSUMER_GROUP`

**File:** `plugins/tumult-kafka/probes/consumer-lag.sh:12`

`GROUP` is passed to `kafka-consumer-groups --group "${GROUP}"` with no validation. The other kafka scripts also lack validate.sh. Not a critical risk since they are probes (read-only), but consistency matters.

---

## Category 8 — TLS / Transport Security

### TLS-01 · **MEDIUM** — OTLP endpoint TLS is not enforced

**File:** `tumult-otel/src/config.rs:41`

`OTEL_EXPORTER_OTLP_ENDPOINT` is accepted as-is, with no check that it uses `https://`. An operator who accidentally sets `http://` will silently transmit telemetry (including `ssh.command` span attributes) in plaintext.

**Recommended fix:** Log a `tracing::warn!` if the configured endpoint starts with `http://` and TLS is not explicitly disabled. Consider rejecting HTTP endpoints unless `TUMULT_OTEL_ALLOW_INSECURE=true` is set.

### TLS-02 · **LOW** — ClickHouse HTTP default (see CRED-02)

Already documented under credential handling. No additional action needed.

---

## Category 9 — File Permissions

### PERM-01 · **LOW** — Journal files are created with default process umask

**File:** `tumult-core/src/journal.rs:25`

`std::fs::write(path, toon)` creates files with the process's umask permissions (typically `0o644` or `0o664`). Journal files may contain sensitive experiment outputs, error messages, or infrastructure details that are world-readable in default configurations.

**Recommended fix:** Use `OpenOptions` with explicit mode:
```rust
use std::os::unix::fs::OpenOptionsExt;
OpenOptions::new()
    .write(true).create(true).truncate(true)
    .mode(0o600)
    .open(path)?
    .write_all(toon.as_bytes())?;
```

### PERM-02 · **LOW** — DuckDB analytics database created with default permissions

**File:** `tumult-analytics/src/duckdb_store.rs` (database file creation)

Same issue as PERM-01 for the DuckDB database file.

---

## Category 10 — Dependency Supply Chain

### DEP-01 · **HIGH** — `cargo audit` gated with `continue-on-error: true`

**File:** `.github/workflows/ci.yml:66`

```yaml
- name: cargo audit
  uses: rustsec/audit-check@v2.0.0
  continue-on-error: true
```

This means PRs with known CVEs in dependencies will pass CI without warning. The security gate provides no protection. This is particularly important because the platform depends on DuckDB (bundled C++ library), Tokio, and an MCP SDK — all high-value supply chain targets.

**Recommended fix:** Remove `continue-on-error: true`. If specific advisories need to be ignored, use `rustsec/audit-check` with an explicit `ignore:` list in `audit.toml`.

### DEP-02 · **MEDIUM** — GitHub Actions not pinned to commit SHA

**Files:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`

Actions are pinned to mutable tags:
- `actions/checkout@v4`
- `dtolnay/rust-toolchain@stable`
- `Swatinem/rust-cache@v2`
- `rustsec/audit-check@v2.0.0`
- `softprops/action-gh-release@v2`
- `actions/upload-artifact@v4`
- `actions/download-artifact@v4`

A compromised action maintainer can push malicious code to the tag. In CI, this has access to `GITHUB_TOKEN` and any build secrets.

**Recommended fix:** Pin all actions to full commit SHAs:
```yaml
uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683  # v4.2.2
```

### DEP-03 · **LOW** — `GITHUB_TOKEN` permissions not explicitly scoped in `ci.yml`

**File:** `.github/workflows/ci.yml`

No `permissions:` block is declared, meaning the workflow runs with GitHub's default token permissions (typically `read` for most scopes, `write` for `contents` in some contexts). Explicitly scoping permissions is a defense-in-depth measure.

**Recommended fix:**
```yaml
permissions:
  contents: read
```

### DEP-04 · **LOW** — `duckdb` bundled feature compiles large C++ codebase at build time

**File:** `Cargo.toml`

`duckdb = { version = "1.10501", features = ["bundled"] }` compiles the full DuckDB C++ library. The version string `"1.10501"` looks unusual (likely `1.1.5.0.1`) and should be verified. Bundled C++ code executed at compile time via `build.rs` has full system access.

**Recommended fix:** Verify the DuckDB crate version string against the published crate on crates.io. Run `cargo audit` after any DuckDB update to catch newly disclosed CVEs quickly.

### DEP-05 · **LOW** — `rust-mcp-sdk` is an early-stage crate

**File:** `Cargo.toml`

`rust-mcp-sdk = "0.9"` is a very new crate for an emerging protocol. Limited audit history, no RustSec advisories yet but also no track record. The MCP layer is a significant attack surface (it exposes file system and SQL query access to AI agents).

**Recommended fix:** Monitor the crate's advisory status. Consider vendoring it with `cargo vendor` for supply-chain stability.

---

## Category 11 — Script Plugin Security

### SCRIPT-01 — `validate.sh` usage summary

`validate.sh` is sourced correctly in:

| Plugin family | validate.sh sourced? | Fields validated |
|---|---|---|
| tumult-db-postgres (all scripts) | Yes | DATABASE, TABLE, DURATION, connection counts |
| tumult-db-mysql (actions) | Yes | DATABASE, TABLE, DURATION, PIDs |
| tumult-network (add-latency, add-packet-loss, add-corruption) | Yes | numeric params |
| tumult-stress (cpu, memory, io) | Yes | integer params |
| tumult-process (kill-process) | Signal allowlist only — validate.sh not sourced |
| tumult-db-redis (all) | **No** |
| tumult-kafka (all) | **No** |
| tumult-containers (all) | **No** |
| tumult-network (partition-host, block-dns, reset-tc) | **No** |
| tumult-stress (combined-stress) | **No** |

Three full plugin families (Redis, Kafka, containers) and several individual scripts skip validate.sh entirely. This is an inconsistency gap, not an immediate injection risk in most cases (arguments are double-quoted), but it means the defensive layer is absent.

### SCRIPT-02 · **MEDIUM** — `partition-host.sh` and `partition-broker.sh` have no rollback scripts

**Files:** `plugins/tumult-network/actions/partition-host.sh`, `plugins/tumult-kafka/actions/partition-broker.sh`

Both scripts add iptables rules with `--comment "tumult-*"` tags, but there are no corresponding cleanup/rollback scripts that remove these rules. If an experiment is interrupted after partitioning and before a manual rollback, the target host remains partitioned. `reset-tc.sh` exists for tc rules but no equivalent `unpartition-host.sh` exists for iptables.

**Recommended fix:** Add `unpartition-host.sh` and `unpartition-broker.sh` that remove the tagged iptables rules:
```sh
iptables -D INPUT -s "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null || true
iptables -D OUTPUT -d "${TARGET_IP}" -j DROP -m comment --comment "tumult-partition" 2>/dev/null || true
```

### SCRIPT-03 — No execution timeout on MCP `ProcessExecutor`

**File:** `tumult-mcp/src/handler.rs:132–135`

```rust
std::process::Command::new(path).args(arguments).envs(env).output()
```

`Command::output()` blocks indefinitely. A hanging script (network timeout, deadlocked psql, etc.) will block the MCP handler thread forever. This is a denial-of-service risk in the MCP server.

**Recommended fix:** Implement a timeout wrapper using `tokio::time::timeout` or spawn a thread with a signal-based kill after a configurable deadline.

---

## Summary Table

| ID | Severity | Category | File | Issue |
|----|----------|----------|------|-------|
| SSH-01 | **CRITICAL** | SSH | tumult-ssh/src/session.rs:344 | Host key verification disabled |
| SQL-01 | **HIGH** | SQL Injection | tumult-mcp/src/tools.rs:56,295 | Raw SQL passed to DuckDB from MCP |
| SQL-02 | **HIGH** | SQL Injection | tumult-cli/src/commands.rs:499 | Raw SQL from `--query` flag |
| PATH-01 | **HIGH** | Path Traversal | tumult-mcp/src/tools.rs | No path containment in MCP tools |
| DEP-01 | **HIGH** | Supply Chain | .github/workflows/ci.yml:66 | `cargo audit` never blocks PRs |
| CRED-01 | **MEDIUM** | Credentials | tumult-ssh/src/telemetry.rs:29 | SSH command in OTel span attributes |
| SQL-03 | **MEDIUM** | SQL Injection | tumult-clickhouse/src/store.rs | Raw SQL API surface |
| PATH-02 | **MEDIUM** | Path Traversal | tumult-plugin/src/discovery.rs | Script paths not contained to plugin dir |
| TLS-01 | **MEDIUM** | TLS | tumult-otel/src/config.rs:41 | OTLP endpoint allows HTTP silently |
| VALID-01 | **MEDIUM** | Input Validation | tumult-process/actions/kill-process.sh | PID not validated as integer |
| VALID-02 | **MEDIUM** | Input Validation | tumult-db-redis/actions/*.sh | DURATION not validated; no validate.sh |
| VALID-03 | **MEDIUM** | Input Validation | tumult-kafka/actions/add-broker-latency.sh | KAFKA_PORT not validated |
| DEP-02 | **MEDIUM** | Supply Chain | .github/workflows/*.yml | Actions not pinned to SHA |
| SCRIPT-02 | **MEDIUM** | Shell | partition-host.sh, partition-broker.sh | No iptables rollback scripts |
| SCRIPT-03 | **MEDIUM** | Shell | tumult-mcp/src/handler.rs:132 | MCP ProcessExecutor has no timeout |
| CRED-02 | **LOW** | Credentials | tumult-clickhouse/src/config.rs:28 | Default HTTP URL transmits password in cleartext |
| CRED-03 | **LOW** | Credentials | exhaust-connections.sh:32 | PGPASSFILE trap registered after file write |
| INJECT-01 | **LOW** | Shell Injection | partition-host.sh:10 | TARGET_IP not format-validated |
| INJECT-02 | **LOW** | Shell Injection | block-dns.sh:9 | DNS_PORT not validated |
| INJECT-03 | **LOW** | Shell Injection | kill-broker.sh:11 | SIGNAL not allowlisted |
| INJECT-04 | **LOW** | Shell Injection | combined-stress.sh | No validate.sh, unvalidated parameters |
| INJECT-05 | **LOW** | Shell Injection | fill-disk.sh:13 | TOPIC and RETENTION_MS not validated |
| VALID-04 | **LOW** | Input Validation | tumult-kafka/probes/ | CONSUMER_GROUP not validated |
| PERM-01 | **LOW** | File Permissions | tumult-core/src/journal.rs:25 | Journal files use default umask |
| PERM-02 | **LOW** | File Permissions | tumult-analytics/src/duckdb_store.rs | DuckDB file uses default umask |
| DEP-03 | **LOW** | Supply Chain | .github/workflows/ci.yml | GITHUB_TOKEN permissions unscoped |
| DEP-04 | **LOW** | Supply Chain | Cargo.toml | DuckDB bundled C++ version string unusual |
| DEP-05 | **LOW** | Supply Chain | Cargo.toml | rust-mcp-sdk is early-stage |
