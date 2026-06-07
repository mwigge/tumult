# Changelog

All notable changes to the Tumult project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [1.2.0] — Live agentic fault injection + real fault-execution engine

### Added

- **`tumult agentic proxy`**: a fault-injecting HTTP reverse proxy that puts a
  scenario pack's faults into the live model traffic of any base-URL-configurable
  agent. Point Claude Code (`ANTHROPIC_BASE_URL`), the Codex CLI / OpenCode
  (`OPENAI_BASE_URL`), or GitHub Copilot (`HTTPS_PROXY`) at the proxy and watch
  how the real agent copes with injected latency, rate limits, provider errors,
  timeouts, malformed/truncated output, tool failures, and retrieval poisoning.
  Each request is logged and optionally appended to a metadata-only JSONL journal
  with live contract verdicts. New module `tumult-agentic::proxy`.
- **Shared fault-execution engine** (`tumult-agentic::engine`): a single
  `execute` entry point that gates every fault through the seeded `FaultEngine`,
  applies it via `apply_fault`, and evaluates every contract against the
  resulting response.
- **Live-client guide**: `docs/guides/agentic-live-clients.md` documents wiring
  each agent to the proxy and how faults map onto HTTP behaviour.

### Changed

- **Scenario packs now run through the real engine.** `tumult agentic run
  --scenario` previously reported hand-scripted, pre-computed "post-fault"
  responses. Every pack now applies its declared faults through `FaultEngine` +
  `apply_fault` against a per-pack baseline and evaluates its real contracts, so
  the reported pass/fail genuinely reflects what the faults do.
- **`tumult agentic replay --fixture` now replays the supplied fixture.** It
  previously validated the caller's fixture and then discarded it, always
  reporting a built-in smoke fixture. It now runs the caller's fixture end to
  end through the replay adapter; the report's session, source, and step count
  echo that fixture.

### Fixed

- **`retrieval_poisoning` now contaminates the evaluated response body**, not
  just the `retrieved_documents` list, so the body-based safety contracts
  (citation, PII, secret leakage) actually observe the poisoning.

## [1.1.2] — Docker image build fix

### Fixed

- **Docker image**: add `tumult-agentic` and `tumult-intelligence` to the
  `COPY` list in `docker/Dockerfile.tumult`. The 1.1.1 release build's
  "Publish tumult image" job failed because these two workspace crates
  (added in 1.1.0) were never copied into the Docker build context, so
  `cargo build` could not load their manifests as workspace members.

## [1.1.1] — Release build fix

### Fixed

- **Cross-compilation**: pin the workspace `reqwest` dependency to
  `default-features = false` with `rustls-tls` instead of the default
  `native-tls`/OpenSSL backend. The 1.1.0 release build failed for the
  `*-unknown-linux-musl` targets because `tumult-agentic`'s direct `reqwest`
  dependency activated `default-tls`, pulling in `openssl-sys`, which has no
  OpenSSL development headers in the musl cross-compilation containers.
  Switching to `rustls-tls` matches the TLS stack already used everywhere
  else in the workspace and removes ~155 lines of `native-tls`/OpenSSL
  transitive dependencies from `Cargo.lock`.

## [1.1.0] — Agentic Fault Injection

### Added

- **tumult-agentic**: new crate that treats AI agents as systems under test —
  fault injection, behavioral contracts, replay fixtures, scoring, and
  OpenTelemetry correlation for agent workflows that call models, tools, MCP
  servers, or retrieval systems.
- **Agent target model**: run against HTTP agents and MCP tools/servers, with
  framework adapters (LangChain, AutoGen, CrewAI, Google ADK) planned.
- **Fault types**: model latency/timeouts/rate limits, malformed or truncated
  output, hallucinated tool calls, tool latency/failure, retrieval poisoning,
  context truncation, token budget exhaustion, and retry-loop pressure.
- **Behavioral contracts**: validity, safety, fallback behavior, citation
  presence, schema conformance, retry budget, task success, latency, and cost
  controls.
- **Deterministic replay**: turn captured agent traces or production sessions
  into regression experiments.
- **Scenario packs**: concurrency storm, hallucination under timeout, cost
  explosion detector, malformed JSON recovery, tool timeout fallback, and
  retrieval poisoning — all runnable locally without an external LLM via
  `tumult agentic list-packs` and `tumult agentic smoke`.
- **CLI surfaces**: `tumult agentic smoke`, `tumult agentic run --scenario`,
  and `tumult agentic replay --fixture`, all writing metadata-only journals
  with trace correlation and contract evidence.
- **MCP integration**: new tool support for discovering and running agentic
  fault scenarios through the MCP server.
- **Analytics**: new ingestion tables for agent runs, contract checks, fault
  injections, replay outcomes, and resilience scores in `tumult-analytics`.
- **Telemetry**: agentic spans keep Tumult's `resilience.*` experiment
  attributes and add GenAI-aligned `gen_ai.*` attributes for operation, model,
  tool, provider, and evaluation correlation. Raw prompts, completions, tool
  payloads, and retrieved documents default to metadata-only capture.
- **Docs**: Agentic Quickstart, Agentic Observability, and Agentic Scenarios
  guides, plus runnable examples under `examples/agentic/`.

## [1.0.3] — Release workflow fix

### Fixed

- **Release workflow**: filter `download-artifact` step to `tumult-*` pattern so internal `.dockerbuild` layer-cache artifacts are no longer mixed in with release binaries. Fixes the "Create Release" failure that blocked every release since v0.5.0.

### Notes

- No runtime or API changes. This release exists to land the CI fix and produce a working GitHub Release with binary assets.
- All changes accumulated under tags v1.0.0, v1.0.1, and v1.0.2 (none of which published successfully) are included. v1.0.2 was blocked by GitHub's immutable-releases feature locking the tag after a partial release creation.

## [1.0.2] — Unreleased

Tag exists but no GitHub Release published — the immutable-releases feature locked the tag after a partial release creation. See 1.0.3 for the fix.

## [1.0.1] — Unreleased

Tag exists but no GitHub Release was ever published — the release workflow failed at the "Download all artifacts" step. See 1.0.2 for the fix.

## [1.0.0] — Unreleased

Tag exists but no GitHub Release was ever published — same root cause. See 1.0.2 for the fix.

## [0.3.0] — Phase 2: Platform Plugins + Analytics

### Added

- **tumult-analytics**: Embedded analytics crate
  - TOON Journal → Arrow RecordBatch conversion
  - DuckDB embedded SQL engine with zero-copy Arrow ingestion
  - `tumult analyze` command with default summary + custom SQL queries
  - `tumult export` command for Parquet and CSV export
  - ADR-008: Arrow + DuckDB as embedded analytics engine

- **tumult-kubernetes**: Native Kubernetes chaos plugin (kube-rs 3.1)
  - Actions: pod delete, deployment scale, node cordon/uncordon/drain, network policy
  - Probes: pod ready, pods by label, deployment status, node status, service endpoints
  - Label selector targeting (LitmusChaos/Chaos Mesh patterns)
  - ADR-007: Native vs script plugin for Kubernetes

- **tumult-db-postgres**: PostgreSQL chaos plugin
  - Actions: kill connections, lock tables, inject latency, exhaust connection pool
  - Probes: connection count, replication lag, pool utilization

- **tumult-db-mysql**: MySQL chaos plugin
  - Actions: kill connections, lock tables

- **tumult-db-redis**: Redis chaos plugin
  - Actions: FLUSHALL, CLIENT PAUSE, DEBUG SLEEP
  - Probes: redis ping, redis info (connection/memory stats)

- **tumult-kafka**: Kafka broker chaos plugin
  - Actions: kill broker, partition broker, add broker latency
  - Probes: consumer lag, under-replicated partitions, broker count

- **tumult-network**: Network chaos plugin
  - Actions: tc netem latency/loss/corruption, DNS block, host partition
  - Probes: ping latency, DNS resolve

- **tumult-loadtest**: Load testing integration
  - k6 driver with OTLP trace correlation
  - JMeter driver with JTL metrics parsing
  - Example k6 scripts for HTTP and gRPC

### Security

- Input validation library (plugins/lib/validate.sh)
- SQL injection prevention: identifier validation, dollar-quoting
- Credential protection: MYSQL_PWD and REDISCLI_AUTH env vars
- Container runtime allowlist validation

## [0.2.0] — Phase 1: Essential Plugins

### Added

- **tumult-ssh**: SSH remote execution crate
  - Connection manager with russh 0.58 (pure Rust, no C dependencies)
  - Key-based (Ed25519, RSA, ECDSA) and SSH agent authentication
  - Remote command execution with stdout/stderr capture
  - File upload via SSH channel with timeout enforcement
  - Passphrase redaction in Debug output
  - ADR-006: SSH as universal remote transport

- **tumult-stress**: Script plugin for stress-ng
  - Actions: cpu-stress, memory-stress, io-stress, combined-stress
  - Probes: cpu-utilization, memory-utilization, io-utilization
  - Works on both Linux (/proc) and macOS (sysctl/vm_stat)

- **tumult-containers**: Script plugin for Docker/Podman
  - Actions: kill, stop, pause, unpause, limit-cpu, limit-memory
  - Probes: container-status, container-health
  - Supports Docker and Podman via TUMULT_RUNTIME

- **tumult-process**: Script plugin for process chaos
  - Actions: kill (by PID/name/pattern), suspend (SIGSTOP), resume (SIGCONT)
  - Probes: process-exists, process-resources (JSON output)

- Cross-compile release workflow for 6 targets (Linux + macOS)
- serde defaults on all optional fields — minimal experiment files work
- Plugin script test suite (14 tests validating manifests, probes, error handling)

### Fixed

- Init template uses /proc/cpuinfo + /proc/meminfo probes (works out of the box)
- Process timeout enforcement in CLI executor
- Hypothesis probe with tolerance but no output now fails correctly

### Security

- RSA timing side-channel (RUSTSEC-2023-0071) documented with Ed25519 mitigation

## [0.1.0] — Phase 0: Foundation

### Added

- **tumult-core**: Experiment data model with serde/TOON round-trip support
  - All types: Experiment, Activity, Provider, Tolerance, Hypothesis, Journal
  - Five-phase data model: Estimate, Baseline, During, Post, Analysis
  - Execution targets: Local, SSH, Container, KubeExec
  - Config/secret resolution from environment variables and files

- **tumult-core**: Five-phase experiment runner (`runner::run_experiment`)
  - Phase 0 (Estimate): record predictions before execution
  - Phase 1 (Baseline): statistical baseline acquisition
  - Phase 2 (During): method execution with degradation sampling
  - Phase 3 (Post): recovery measurement
  - Phase 4 (Analysis): estimate vs actual accuracy scoring
  - Hypothesis evaluation (before/after) with tolerance matching
  - Rollback strategies: always, on-deviation, never
  - Controls lifecycle: BeforeExperiment, BeforeMethod, BeforeActivity, etc.

- **tumult-baseline**: Statistical baseline derivation
  - Methods: mean +/- N sigma, percentile, IQR, static
  - Anomaly detection (coefficient of variation, extreme range)
  - Tolerance derivation from baseline samples
  - Recovery point detection and compliance ratio

- **tumult-plugin**: Plugin system
  - `TumultPlugin` trait for native Rust plugins
  - Script plugin manifest parser (TOON format)
  - Script execution with TUMULT_* environment variables
  - Plugin discovery from ./plugins/, ~/.tumult/plugins/, $TUMULT_PLUGIN_PATH

- **tumult-otel**: OpenTelemetry instrumentation
  - TracerProvider, MeterProvider, LoggerProvider setup with OTLP
  - tracing-opentelemetry bridge for #[instrument] spans
  - Standard resilience.* namespace attributes
  - Standard metrics: experiments, actions, probes, deviations

- **tumult-cli**: Command-line interface
  - `tumult run` — execute experiments with journal output
  - `tumult validate` — check experiment syntax and references
  - `tumult discover` — list discovered plugins and actions
  - `tumult init` — scaffold new experiments from templates
  - `--dry-run` mode — show execution plan without running
  - Process provider execution (shell scripts)

- **collector/**: Reference OTel Collector configurations
  - Default (stdout), SigNoz, Grafana (Tempo+Mimir+Loki)
  - docker-compose.yaml for local development with Jaeger

- **Documentation**
  - ADR-001 through ADR-009: architectural decisions
  - Experiment format guide
  - Baseline guide
  - Execution flow guide
  - CLI reference
  - Plugin authoring guide
  - Observability setup guide
  - Resilience metadata standard
  - Data lifecycle specification
