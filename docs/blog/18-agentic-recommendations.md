---
title: "Bring Your Own Agent: Agentic Recommendations"
parent: Blog
nav_order: 19
updated: 2026-07-21
---

# Bring Your Own Agent: Agentic Recommendations

*2026-07-04*

`tumult recommend` has always been deterministic: heuristics over the analytics store tell you which plugin actions you've never tested, which experiments keep failing, and which ones have gone stale. Useful; and a bit predictable.

As of this release, you can hand that heuristic output to an agentic coding CLI you already have installed; Claude Code or Codex; and get back re-ranked recommendations with actual reasoning, plus complete, ready-to-run `.toon` experiments. Validated before they ever touch your disk.

```bash
tumult recommend --agent claude-code --generate-experiments out/experiments
```

No API keys handed to Tumult. No new subscription. Your CLI, your login, your model.

## Why heuristics plateau

The heuristic engine reports only what it knows: coverage gaps, failure counts, timestamps. What it can't do is *reason*. It will tell you `tumult-kafka/kill-broker` has never been tested; it won't tell you that testing it *before* your consumer-lag experiment is pointless because the lag probe is what makes the broker kill interpretable.

That kind of judgment is exactly what a frontier model is good at. And you probably already have one on your machine, wearing a CLI.

## The big picture

```
┌──────────────┐   ┌──────────────┐
│   journals    │   │   coverage    │
│  (analytics   │   │  gaps, fails, │
│    store)     │   │   staleness   │
└──────┬───────┘   └──────┬───────┘
       └────────┬─────────┘
                ▼
       ┌─────────────────┐
       │    heuristics    │  deterministic, same as always
       └────────┬────────┘
                ▼
       ┌─────────────────┐
       │      prompt      │  heuristics + signals + plugin
       └────────┬────────┘  catalog + format example
                ▼
       ┌─────────────────┐
       │  your agent CLI  │  claude-code | codex
       │  (one-shot, no   │  subprocess, no TTY
       │   TTY, timeout)  │
       └────────┬────────┘
                ▼
       ┌─────────────────┐
       │  enhanced recs   │
       │  + proposed      │
       │  experiments     │
       └────────┬────────┘
                ▼
       ┌─────────────────┐
       │  validation gate │  parse + validate; or reject
       └────────┬────────┘
                ▼
       ┌─────────────────┐
       │  .toon files     │  <dir>/<title-slug>.toon
       └─────────────────┘
```

The heuristic output still prints first, unchanged. The agent section is strictly additive; if you drop `--agent`, nothing about `tumult recommend` is different.

## The adapter contract

Each CLI is wrapped in an adapter; the `AgentCliAdapter` trait in the new `tumult-agent-cli` crate. The contract is deliberately small:

```
 detect ──► build ──► run ──► parse ──► answer
   │          │        │        │
   │          │        │        └─ strict extraction; malformed
   │          │        │           output is a typed error, not
   │          │        │           a silent fallback
   │          │        └─ one subprocess, NO_COLOR=1,
   │          │           prompt piped via stdin,
   │          │           killed + reaped on timeout
   │          └─ argv / stdin / env for a
   │             non-interactive invocation
   └─ install? version? auth?; never fails,
      reports structurally
                                          on any failure:
                                 explain-failure ──► human-readable
                                                     hint (stderr,
                                                     exit code, ...)
```

`claude-code` runs `claude -p --output-format json` and extracts the `result` field from the JSON envelope. `codex` runs `codex exec --ephemeral` with a read-only sandbox and takes the final message from stdout. Both get the prompt on stdin; a single non-interactive call, no approval prompts, no session left behind.

Everything is blocking and run-to-completion, so there's no async runtime involved. The subprocess gets a deadline (default 120 s, `--agent-timeout` to change it) and is killed *and reaped* if it blows through; no zombies, no runaway agents.

## The trust boundary

A required safety property is: **nothing the model writes lands on disk unvalidated.**

There are two layers to that. First, the prompt carries the *real* plugin catalog; every discovered plugin, action, and probe; with an explicit instruction that these are the only ones that exist. So the model isn't guessing what your installation looks like.

Second, we don't trust that instruction. With `--generate-experiments <dir>`, every proposed experiment goes through the exact same engine code that runs experiments:

```
  model output
       │
       ▼
┌──────────────┐   everything outside ```toon fences
│ fence parser │──► stays in the recommendation text;
└──────┬───────┘    an unterminated fence is kept as a
       │            partial block; so the gate rejects
       ▼            it visibly instead of it vanishing
┌───────────────────┐
│ parse_experiment  │──✗──► rejected: not a well-formed document
└──────┬────────────┘
       ▼
┌─────────────────────┐
│ validate_experiment │──✗──► rejected: empty method, bad
└──────┬──────────────┘       tolerance, missing probes, ...
       ▼
┌──────────────┐
│    write     │  <dir>/<title-slug>.toon
└──────────────┘  collisions get -2, -3, ...; never overwritten

  2 experiment(s) written, 1 rejected (validation failed)
```

Rejections aren't swallowed. Each one is printed with its validation error, and the summary line counts both sides. A hallucinated plugin, a method with no steps, a broken regex tolerance; none of it gets a file.

(A written experiment is validated, not vetted. Read it, `tumult run --dry-run` it, *then* point it at infrastructure. We're a chaos engineering tool, not a chaos generation tool.)

## Finding the binary

No config file, no setup wizard. Resolution is two steps:

```
        CLAUDE_CODE_BIN / CODEX_BIN set?
                     │
          ┌── yes ───┴─── no ──┐
          ▼                    ▼
   executable file?      scan PATH for
          │              claude / codex
   ┌─ yes ┴─ no ─┐             │
   ▼             ▼        ┌ hit ┴ miss ┐
  use it    warn, then   use it      report the
            scan PATH                install command
```

A stale `*_BIN` override degrades gracefully to the PATH lookup with a warning instead of failing. And when nothing is found, the error tells you exactly how to install the CLI.

The child inherits your environment, so `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` flow through; or your existing CLI login just works.

## Quickstart

Three commands:

```
$ tumult agents
ADAPTER        INSTALLED  VERSION    DETAIL
claude-code    yes        2.0.13     Authenticated via ANTHROPIC_API_KEY.
codex          no         -          Codex CLI not found on PATH. Install with: npm i -g @openai/codex

$ tumult recommend --agent claude-code
...heuristic recommendations (as always)...

=== Agent-enhanced recommendations (claude-code) ===

## Recommendations
1. Exercise redis restart recovery first; the cache tier has zero
   action coverage and every downstream experiment assumes it heals.
2. ...

$ tumult recommend --agent claude-code --generate-experiments out/experiments
...
Wrote out/experiments/redis-restart-under-load.toon
Rejected experiment: experiment method contains no steps
1 experiment(s) written, 1 rejected (validation failed)
```

`--agent-model` passes a model override through to the CLI, and `--format json` adds an `agent` object with the recommendations, written paths, and rejections; handy for wiring this into anything automated.

## What's next

Two adapters today: `claude-code` and `codex`. That's it; no grand claims about "any CLI".

But the trait is one file to implement: pick the CLI's non-interactive flags, an env override for the binary, a strict `parse_output`, and register it in `AdapterRegistry::builtin()`. The test suite drives adapters against fake `#!/bin/sh` binaries via the env override, so a new adapter ships with hermetic tests; no real CLI, no network, no flakes.

If your favorite agent CLI has a batch mode, it's a candidate. PRs welcome.

---

*Full details in the [Agentic Recommendations guide](../guides/agentic-recommendations.md) and the [CLI reference](../guides/cli-reference.md).*
