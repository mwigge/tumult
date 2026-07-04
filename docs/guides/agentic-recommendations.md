---
title: Agentic Recommendations
parent: Guides
nav_order: 11
---

# Agentic Recommendations

`tumult recommend` is deterministic by default: it derives its recommendations from heuristics over the analytics store — untested plugin actions (coverage gaps), the most-failing experiments, and the stalest experiments. With `--agent`, that heuristic output is handed to a locally installed agentic coding CLI, which re-ranks the recommendations with reasoning and can propose complete, ready-to-run `.toon` experiments.

```bash
# See which agent CLIs are installed
tumult agents

# Enhance recommendations with Claude Code
tumult recommend --agent claude-code

# Enhance with Codex and also generate proposed experiment files
tumult recommend --agent codex --generate-experiments out/experiments
```

Nothing here talks to a model API directly. The `tumult-agent-cli` crate drives the agent's own CLI binary in **one-shot batch mode** — a single subprocess call with no TTY, no approval prompts, and no session persistence. Your existing CLI login (or API-key env var) is what authenticates the call.

## The adapter contract

Every agent CLI is integrated through the `AgentCliAdapter` trait in `tumult-agent-cli`:

| Method | Purpose |
|--------|---------|
| `name()` | Stable registry name (`claude-code`, `codex`) |
| `binary_env_key()` | Env var that overrides binary resolution (`CLAUDE_CODE_BIN`, `CODEX_BIN`) |
| `install_hint()` / `auth_hint()` | Human-readable guidance shown on failures |
| `detect()` | Probe install + version + auth state (never fails; reports structurally) |
| `build_invocation()` | Argv/stdin/env for a non-interactive run (prompt piped via stdin) |
| `parse_output()` | Extract the model answer from raw output (strict; typed errors) |
| `explain_failure()` | Human-readable explanation for a failed run |

`run_prompt(adapter, request)` executes the full pipeline: detect → auth gate → build → run → parse. Binary resolution honors the env override first (a non-executable override is ignored with a warning), then falls back to a `PATH` search. The child process always runs with `NO_COLOR=1` and inherits the parent environment, so `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` flow through.

Built-in adapters:

| Adapter | Invocation | Output parsing |
|---------|-----------|----------------|
| `claude-code` | `claude -p --output-format json [--model <m>]` | Strict JSON result envelope; `result` field extracted |
| `codex` | `codex exec --ephemeral ... -` | Final answer from stdout |

## What goes into the prompt

`tumult_intelligence::build_agent_prompt` assembles one self-contained prompt (no file access needed by the agent):

1. **Heuristic recommendations** — the deterministic output `tumult recommend` already computes, rendered as text.
2. **Journal signals** — the coverage/failure/staleness report derived from the analytics store.
3. **Plugin catalog** — every discovered plugin with its actions and probes, with an instruction that these are the *only* actions the model may reference.
4. **Task + format example** — when experiment generation is requested, a compact real `.toon` experiment (modeled on `examples/redis-chaos.toon`) is embedded so proposals follow the actual experiment format.
5. **A strict response envelope** — a `## Recommendations` section first, then zero or more fenced code blocks tagged `toon`, each containing exactly one experiment document. The strict envelope is what makes response parsing mechanical rather than heuristic.

`tumult_intelligence::enhance` runs the prompt through the adapter and splits the response into recommendation text plus the raw `toon` blocks. The fence parser is deliberately forgiving: responses without fences yield zero experiments, non-`toon` fences stay in the recommendation text, and an unterminated fence is kept as a partial block so the validation gate can reject it visibly instead of it disappearing.

## The validation gate

Agent-proposed experiments are **never trusted blindly**. With `--generate-experiments <dir>`, each `toon` block goes through the same engine that runs experiments:

1. `tumult_core::engine::parse_experiment` — the block must decode as a well-formed experiment document.
2. `tumult_core::engine::validate_experiment` — version, non-empty method, hypothesis probes, regex tolerances, and range bounds must all check out.

Only experiments passing both are written, to `<dir>/<title-slug>.toon` (slug = sanitized lowercase title). Existing files are never overwritten — collisions get `-2`, `-3`, ... suffixes. Rejected experiments are reported with their validation error and counted in the summary:

```
Wrote out/experiments/redis-restart-under-load.toon
Rejected experiment: experiment method contains no steps
1 experiment(s) written, 1 rejected (validation failed)
```

In `--format json` mode the output gains an `agent` object:

```json
{
  "agent": {
    "adapter": "claude-code",
    "model": null,
    "recommendations": "## Recommendations\n1. ...",
    "experiments_written": ["out/experiments/redis-restart-under-load.toon"],
    "experiments_rejected": [{ "error": "experiment method contains no steps" }]
  }
}
```

A written experiment is validated, not vetted: review it (and `tumult run --dry-run` it) before pointing it at real infrastructure.

## Environment variables

| Variable | Description |
|----------|-------------|
| `CLAUDE_CODE_BIN` | Explicit path to the Claude Code binary (fallback: `claude` on `PATH`) |
| `CODEX_BIN` | Explicit path to the Codex binary (fallback: `codex` on `PATH`) |
| `ANTHROPIC_API_KEY` | Inherited by Claude Code; when set, `detect()` reports authenticated |
| `OPENAI_API_KEY` | Inherited by Codex |

## Adding a new adapter

1. Create `tumult-agent-cli/src/<name>.rs` implementing `AgentCliAdapter`: pick the CLI's non-interactive flags (prompt via stdin, machine-parseable output), a `<NAME>_BIN` env override, and strict `parse_output` with typed `AgentCliError`s.
2. Register it in `AdapterRegistry::builtin()` (`tumult-agent-cli/src/registry.rs`) — this makes it available to `tumult recommend --agent <name>` and `tumult agents` automatically.
3. Add hermetic tests in `tumult-agent-cli/tests/adapters.rs` driving a fake `#!/bin/sh` binary via the env override — no real CLI, no network. Env-mutating tests must hold the shared env mutex.

## See also

- [CLI Reference](cli-reference.md) — full `tumult recommend` / `tumult agents` flag tables
- [Experiment Format](experiment-format.md) — the `.toon` document structure proposals must follow
- [Agentic Fault Injection Quickstart](agentic-quickstart.md) — the inverse direction: injecting faults *into* agents
