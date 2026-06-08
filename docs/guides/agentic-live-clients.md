---
title: Agentic Fault Injection Against Live Clients
parent: Guides
nav_order: 9
---

# Agentic Fault Injection Against Live Clients

The [quickstart](agentic-quickstart.md) runs faults against deterministic local
fixtures. This guide runs the *same* faults against a **real** coding agent —
Claude Code, the Codex CLI, OpenCode, or GitHub Copilot — by putting Tumult
between the agent and its model provider.

Every mainstream agent can be pointed at a custom model base URL (or an HTTP
proxy). Tumult's `agentic proxy` listens on a local port, forwards traffic to
the real provider, and injects a scenario pack's faults into the live stream.

```
  ┌────────────┐   model API    ┌──────────────────┐   model API   ┌──────────────┐
  │  agent     │ ─────────────► │ tumult agentic   │ ────────────► │  provider    │
  │ (claude    │                │ proxy            │               │ (anthropic,  │
  │  code,     │ ◄───────────── │  · injects faults│ ◄──────────── │  openai,...) │
  │  codex,...)│   faulted resp │  · scores traffic│   real resp   │              │
  └────────────┘                └──────────────────┘               └──────────────┘
```

## Start the Proxy

```bash
# Faults from the malformed-json-recovery pack, forwarded to Anthropic.
tumult agentic proxy \
  --listen 127.0.0.1:8080 \
  --upstream https://api.anthropic.com \
  --scenario malformed-json-recovery \
  --journal target/agentic/proxy.jsonl
```

The proxy prints the exact wiring for each client on startup:

```text
Tumult agentic fault-injecting proxy
  listening: http://127.0.0.1:8080
  upstream:  https://api.anthropic.com
  scenario:  malformed-json-recovery
  faults:    malformed_output
  journal:   target/agentic/proxy.jsonl

Point your agent at the proxy, then drive it as usual:
  Claude Code:  ANTHROPIC_BASE_URL=http://127.0.0.1:8080 claude
  Codex CLI:    OPENAI_BASE_URL=http://127.0.0.1:8080/v1 codex
  OpenCode:     OPENAI_BASE_URL=http://127.0.0.1:8080/v1 opencode
  Copilot CLI:  HTTPS_PROXY=http://127.0.0.1:8080 copilot

Press Ctrl-C to stop.
```

## Wire Up Each Client

Run the proxy in one terminal, then start your agent in another with the
matching environment variable.

### Claude Code

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:8080 claude
```

Use `--scenario malformed-json-recovery` (corrupts model output so you can watch
Claude Code recover) or `--scenario concurrency-storm` (returns synthetic `429`s
so you can watch its backoff).

### Codex CLI

```bash
OPENAI_BASE_URL=http://127.0.0.1:8080/v1 codex
```

Forward to OpenAI: `--upstream https://api.openai.com`.

### OpenCode

Point the provider `baseURL` at the proxy in OpenCode's config, or for an
OpenAI-compatible provider:

```bash
OPENAI_BASE_URL=http://127.0.0.1:8080/v1 opencode
```

### GitHub Copilot CLI

```bash
HTTPS_PROXY=http://127.0.0.1:8080 copilot
```

## How Faults Map to HTTP

| Fault (from the pack)                  | Live proxy behaviour                              |
|----------------------------------------|---------------------------------------------------|
| `model_latency`, `tool_latency`        | delay before forwarding (damages time-to-first-token) |
| `rate_limit`                           | synthetic `429` + `retry-after` (no upstream call) |
| `provider_error`                       | synthetic provider status code                    |
| `model_timeout`                        | synthetic `504`                                   |
| `malformed_output`, `output_truncation`| corrupt / truncate the response body              |
| `tool_failure`                         | replace the body with a tool-error envelope       |
| `retrieval_poisoning`                  | inject poisoned content into the response body    |
| token / retry / hallucination / context| agent-internal — recorded, forwarded unchanged    |

The last row matters: token-budget, retry-loop, hallucinated-tool-call, and
context-truncation faults describe agent *internal* state that cannot be forced
from the HTTP boundary. The proxy records that they were selected but forwards
the response untouched. Use the offline `tumult agentic run --scenario ...` path
to exercise those against synthetic baselines.

## Watch the Evidence

Each proxied request is logged and, with `--journal`, appended as one JSONL row.
No raw prompt or completion is stored — only metadata and contract verdicts, in
line with the metadata-only capture default:

```json
{"scenario":"malformed-json-recovery","method":"POST","path":"/v1/messages","status":200,"latency_ms":312,"faults":["malformed_output"],"contracts":["valid_json=fail","graceful_error=pass"],"body_bytes":15}
```

## Reproducibility

Fault selection is seeded per request from `--seed` (default `1`). The bundled
packs all use probability `1.0`, so every matching request is faulted; lower the
probability in a custom pack and the same `--seed` replays the same decisions.

## Safety Notes

- The proxy forwards your real credentials to the real upstream — run it locally
  and never expose the listen port beyond `127.0.0.1`.
- `accept-encoding` is stripped so responses arrive uncompressed and can be
  inspected and mutated; everything else (including `authorization`) is forwarded
  verbatim.
- Bodies pass through tumult in memory, but **capture is metadata-only by
  default** — no prompt, completion, or body content is written to the journal or
  telemetry (only counts, durations, fault types, and contract verdicts).
- Faults are injected, so the agent will receive corrupted or error responses by
  design. Point it at a throwaway task, not production work.

See the [Security Assessment](../security-assessment.md) (§10, Agentic Fault
Injection Security) for the full trust-boundary analysis.
