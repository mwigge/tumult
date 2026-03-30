# Changelog

All notable changes to the Tumult project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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
