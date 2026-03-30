# Changelog

All notable changes to the Tumult project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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
