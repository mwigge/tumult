# Tumult Technical Design Specification
**Version:** 1.1.0  
**Status:** Draft / Technical Foundation  

## 1. Workspace Structure (Rust Cargo Workspace)
Tumult is built as a highly modular workspace to allow for lean builds and feature-gated plugins.

```text
tumult/                          # Cargo workspace root
├── tumult-cli/                  # Binary: The `tumult` command
├── tumult-core/                 # Engine: Runner, Hypothesis, Controls
├── tumult-plugin/               # Traits, Registry, Manifest Loader
├── tumult-otel/                 # OTel setup, Spans, OTLP export
├── tumult-baseline/             # Statistical methods (μ ± Nσ, IQR)
├── tumult-analytics/            # DuckDB + Arrow + Parquet export
├── tumult-regulatory/           # DORA/NIS2 mapping & Reporting
├── tumult-ssh/                  # Remote execution (russh)
├── tumult-stress/               # stress-ng wrapper
├── tumult-kubernetes/           # kube-rs based chaos
├── tumult-loadtest/             # k6/JMeter background drivers
├── tumult-mcp/                  # MCP server adapter (Phase 3)
└── plugins/                     # Community script plugins