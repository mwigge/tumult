---
title: Agentic Cross-Client Observability
parent: Guides
nav_order: 11
---

# Agentic Cross-Client Observability

The [live clients guide](agentic-live-clients.md) injects faults into a real
agent's traffic. This guide explains how Tumult makes the resulting telemetry
**comparable across clients** — Claude Code, the Codex CLI, OpenCode, and GitHub
Copilot — so one resilience experiment reads the same way regardless of which
agent was under test.

The four clients do not emit identical native telemetry. They differ in schema,
in whether they speak OTel at all, and in how far they propagate trace context.
Tumult's job is to normalize what they do emit onto one canonical schema and to
guarantee a uniform floor of evidence even when a client emits nothing.

## Canonical Schema

All agentic telemetry — whatever its origin — normalizes onto a single schema:

- **`gen_ai.*`** — OpenTelemetry GenAI semantic conventions for the model and
  tool surface (`gen_ai.operation.name`, `gen_ai.system`, `gen_ai.request.model`,
  `gen_ai.tool.name`, `gen_ai.evaluation.result`, token counts). See the
  [observability guide](agentic-observability.md) for the full attribute table.
- **`resilience.agent.*`** — Tumult's experiment-side attributes (scenario, fault
  type, contract, score, capture policy, byte counts, payload hash).
- **`tumult.client`** — a resource attribute identifying which agent produced the
  telemetry. Allowed values: `claude-code`, `codex`, `copilot`, `opencode`,
  `unknown`. This is the single dimension you slice on to compare clients.

The capture policy is **metadata-only by default**: scenario names, fault labels,
contract outcomes, scores, byte counts, hashes, and trace IDs are recorded; raw
prompts, completions, tool inputs/outputs, retrieved documents, PII, secrets, and
credentials are not. Content is included only when you explicitly opt a client in
(see per-client privacy notes below), and only in controlled environments.

Normalization onto this schema is performed by the collector. Point your client
exporters at the collector configured by
[`collector/otel-agentic-normalization.yaml`](../../collector/otel-agentic-normalization.yaml),
which maps each client's native attributes onto `gen_ai.*` and stamps the
`tumult.client` resource attribute.

## Two-Sided Observability

Every agentic experiment produces telemetry from two independent sides. They are
designed to share one trace when context propagates, but each stands on its own
when it does not.

### Experiment side — what Tumult did

Tumult emits a `resilience.agentic.experiment` span tree through `tumult-otel`.
It records the experiment itself, independent of the agent:

```text
resilience.agentic.experiment
├── resilience.agent.scenario          (scenario / pack entry)
├── resilience.agent.fault.decision    (one per fault evaluated: selected or not)
├── resilience.agent.fault.application  (one per fault actually applied)
├── resilience.agent.contract          (one per contract outcome)
└── resilience.agent.score             (resilience score for the run)
```

This side is **always present**, including for fully offline runs
(`tumult agentic run` / `replay` / `smoke`) where no live client is involved. It
answers: which scenario ran, which faults were decided and applied, which
contracts passed or failed, and what score resulted.

### Target side — how the agent reacted

The target side is the agent's own behaviour under the injected faults. It has
two sources:

- **The proxy span floor.** The `agentic proxy` emits a span per proxied request
  on the model surface — method, path, status, latency, faults applied, contract
  verdicts, byte counts — for *every* client, with no client-side configuration.
  This is the guaranteed-uniform layer.
- **The client's native OTel.** When a client exports its own spans (model calls,
  tool/MCP calls, internal retries), those are normalized onto `gen_ai.*` and
  added to the target side, giving richer detail than the proxy floor alone.

When trace context propagates from Tumult through the proxy to the agent and back
into the agent's own exporter, **both sides share one trace** and the experiment
and the agent's reaction sit in a single nested tree. When it does not propagate,
the two sides remain separate traces correlated by attribute
(`tumult.client`, `resilience.experiment.id`, `resilience.run.id`).

## Trace-Nesting Tiers

One caveat matters here: the clients do not nest equally. There are three tiers.

| Tier | Behaviour | Clients |
|---|---|---|
| **Full nesting** | Provider-side `traceparent` honoured on both the model and MCP/tool surfaces; the agent's spans nest under the experiment trace. | Claude Code |
| **Partial nesting** | `traceparent` honoured on the MCP/tool surface only; model-call spans correlate by attribute. | OpenCode |
| **Correlate-by-attribute** | No provider-side `traceparent`; spans are joined to the experiment by `tumult.client` + experiment/run IDs, not by trace ID. | Codex, Copilot |

- **Claude Code** nests on both surfaces, but only with
  `CLAUDE_CODE_PROPAGATE_TRACEPARENT=1` set.
- **OpenCode** nests on the MCP/tool surface only, and only with
  `experimental.openTelemetry: true` enabled.
- **Codex** and **Copilot** do not propagate a provider-side `traceparent`; their
  telemetry is correlated to the experiment by shared attributes.

Regardless of tier, the **proxy span is the uniform floor**: it gives every
client a comparable target-side record even when no provider-side trace context
flows. Treat full/partial nesting as an enrichment over that floor, not a
prerequisite.

## Per-Client Wiring

Run the [fault-injecting proxy](agentic-live-clients.md#start-the-proxy) on
`127.0.0.1:8080`, point the client's model base URL at it, and point the client's
OTel exporter at the normalization collector. The snippets below are
copy-pasteable; set `OTEL_EXPORTER_OTLP_ENDPOINT` to your collector's OTLP
endpoint (for example `http://127.0.0.1:4318`).

### Claude Code

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318
export CLAUDE_CODE_PROPAGATE_TRACEPARENT=1
export ANTHROPIC_BASE_URL=http://127.0.0.1:8080
claude
```

`CLAUDE_CODE_PROPAGATE_TRACEPARENT=1` is what lifts Claude Code into the
full-nesting tier on both the model and MCP surfaces.

**Privacy.** Claude Code spans redact content by default — they carry metadata,
not prompt or completion text. Opt into content only when you need it, in a
controlled environment, via the `OTEL_LOG_*` controls.

### Codex CLI

Codex is configured through `config.toml`. Point its model base URL at the proxy,
add an `[otel]` exporter, keep prompt logging off, and disable analytics so the
CLI does not phone home:

```toml
openai_base_url = "http://127.0.0.1:8080/v1"

[otel]
exporter = "otlp-http"
endpoint = "http://127.0.0.1:4318"
log_user_prompt = false

[analytics]
enabled = false
```

Codex does not propagate a provider-side `traceparent`, so its telemetry
correlates to the experiment by attribute. The proxy span is its uniform floor.

### OpenCode

Configure OpenCode with an OpenAI-compatible provider whose base URL is the
proxy, export OTLP to the collector, and enable experimental OTel so the MCP/tool
surface nests:

```bash
export OPENCODE_OTLP_ENDPOINT=http://127.0.0.1:4318
export OPENCODE_OTLP_HEADERS=""   # e.g. "authorization=Bearer <token>" if your collector needs it
OPENAI_BASE_URL=http://127.0.0.1:8080/v1 opencode
```

In OpenCode's config, set the provider base URL to the proxy and enable:

```json
{
  "experimental": {
    "openTelemetry": true
  }
}
```

`experimental.openTelemetry: true` puts OpenCode in the partial-nesting tier:
`traceparent` is honoured on the MCP/tool surface, while model calls correlate by
attribute.

### GitHub Copilot SDK

Configure the Copilot SDK's `TelemetryConfig` with an OTLP endpoint:

```ts
const telemetry: TelemetryConfig = {
  otlpEndpoint: "http://127.0.0.1:4318",
};
```

Copilot's support for a custom model base URL is limited, so route its model
traffic through the proxy at the transport layer (for example
`HTTPS_PROXY=http://127.0.0.1:8080`) rather than via a base-URL setting. Copilot
does not propagate a provider-side `traceparent`; its telemetry correlates to the
experiment by attribute, over the proxy span floor.

## Client Capability Summary

| Client | Proxiable | Native OTel | Trace-nesting tier |
|---|---|---|---|
| Claude Code | Yes (`ANTHROPIC_BASE_URL`) | Yes (`CLAUDE_CODE_ENABLE_TELEMETRY`) | Full (model + MCP, needs `CLAUDE_CODE_PROPAGATE_TRACEPARENT=1`) |
| Codex CLI | Yes (`openai_base_url`) | Yes (`[otel]` in `config.toml`) | Correlate-by-attribute |
| OpenCode | Yes (OpenAI-compatible base URL) | Yes (`experimental.openTelemetry`) | Partial (MCP/tool surface only) |
| Copilot SDK | Limited (transport proxy) | Yes (`TelemetryConfig.otlpEndpoint`) | Correlate-by-attribute |

## What You Get

Whichever client you test, the experiment side is identical and always present,
the proxy span floor is identical, and the `tumult.client` attribute lets you
compare runs across clients on one canonical schema. Native client OTel and
trace-nesting add detail where the client supports it — but no client is required
to support them for the experiment to be observable and comparable.

For the underlying attribute reference and privacy defaults, see
[Agentic Observability](agentic-observability.md). For fault-to-HTTP mapping and
proxy safety notes, see [Agentic Live Clients](agentic-live-clients.md).
