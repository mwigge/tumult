---
title: Home
layout: home
nav_order: 1
---

# Tumult

**Rust-native chaos engineering for modern distributed systems.**

Tumult is a single statically-linked binary that runs chaos experiments, emits OpenTelemetry traces automatically, and produces structured journals designed for SQL analytics and AI analysis — no runtime dependencies, no configuration required to get started.

[Get Started]({% link guides/experiment-format.md %}){: .btn .btn-primary .fs-5 .mb-4 .mb-md-0 .mr-2 }
[View on GitHub](https://github.com/mwigge/tumult){: .btn .fs-5 .mb-4 .mb-md-0 }

---

## Why Tumult?

| | Tumult | Chaos Toolkit |
|---|---|---|
| **Runtime** | Single static binary | Python + pip |
| **Observability** | OTel built-in, always on | Plugin, opt-in |
| **Output format** | TOON (40–50% fewer tokens) | JSON |
| **Analytics** | Embedded DuckDB + Parquet | External |
| **Remote execution** | SSH native (pure Rust) | External driver |

---

## Key Features

**Single Binary**
Install with `cargo install tumult` or download a pre-built release. No interpreter, no dependencies, cross-compiled for macOS (Intel + Apple Silicon), Linux (x86_64 + aarch64 + musl), and Windows.

**OTel by Default**
Every experiment, action, and probe emits an OpenTelemetry span with structured `resilience.*` attributes. Point `OTEL_EXPORTER_OTLP_ENDPOINT` at any collector — Jaeger, Grafana Tempo, SigNoz — and traces appear automatically.

**TOON Format**
Experiments and journals use TOON (Token-Oriented Object Notation): human-readable, serde-compatible, and 40–50% fewer tokens than equivalent JSON. Journals feed directly into LLM analysis pipelines.

**Embedded Analytics**
`tumult analyze journal.toon --query "SELECT ..."` runs SQL over your experiment history via embedded DuckDB. Export to Parquet for long-term retention and compliance evidence.

**Plugin System**
Write a plugin in any language — bash, Python, Go — by placing scripts in a directory with a `plugin.toon` manifest. No Rust required. Native Rust plugins (Kubernetes, SSH) are compiled in via feature flags.

**Regulatory Evidence**
Map experiments to DORA, NIS2, PCI-DSS 4.0, ISO 22301, and Basel III requirements. Journals are the audit artifact. `tumult compliance` generates reports.

---

## Quick Start

```bash
cargo install tumult
```

Or download a pre-built binary from [GitHub Releases](https://github.com/mwigge/tumult/releases).

```bash
tumult init my-experiment
tumult run my-experiment.toon
tumult analyze my-experiment.journal.toon
```

---

## Blog

{% assign posts = site.pages | where_exp: "p", "p.parent == 'Blog'" | sort: "nav_order" %}
{% for post in posts %}
- [{{ post.title }}]({{ post.url | relative_url }})
{% endfor %}

---

## Documentation

- [Experiment Format]({% link guides/experiment-format.md %})
- [CLI Reference]({% link guides/cli-reference.md %})
- [Plugin System]({% link plugins/authoring-guide.md %})
- [Observability Setup]({% link guides/observability-setup.md %})
- [Analytics Guide]({% link guides/analytics-guide.md %})
