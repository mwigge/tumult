---
title: "The AI Advantage: Why TOON Changes Everything"
parent: Blog
nav_order: 2
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> The AI Advantage: Why TOON Changes Everything for Chaos Engineering

![Tumult Banner](/images/tumult-banner.png)

*Part 2 of the Tumult series. [Read Part 1: Introducing Tumult →](./01-introducing-tumult.md)*

---

There is a quiet revolution happening in quality engineering. AI agents — systems that can reason, plan, and execute multi-step tasks — are beginning to take on roles that previously required human engineers: analyzing test failures, identifying patterns across thousands of runs, and proposing the next experiment to run. Tumult was designed from day one to work with this reality.

This post is about the format that makes it possible.

---

## The Token Economy

When you feed text to a Large Language Model, you pay in tokens. A token is roughly four characters of English text. The cost, latency, and context window limitations of LLM calls all scale with token count.

Legacy chaos engineering tools output JSON journals. JSON is fine for human inspection and machine parsing. But it is verbose. Structural characters — braces, brackets, quotes, colons — dominate. Here is a representative excerpt from a Chaos Toolkit journal:

```json
{
  "activity": {
    "type": "probe",
    "name": "application-must-respond-normally",
    "provider": {
      "type": "http",
      "url": "http://sample-app:8080/",
      "method": "GET",
      "timeout": 3
    },
    "tolerance": 200,
    "background": false
  },
  "output": 200,
  "status": "succeeded",
  "start": "2024-01-15T14:23:11.234567",
  "end": "2024-01-15T14:23:11.341089",
  "duration": 0.106522
}
```

Now here is the same result in a Tumult TOON journal:

```toon
- name: application-must-respond-normally
  activity_type: probe
  status: succeeded
  output: 200
  started_at_ns: 1705327391234567000
  duration_ms: 107
```

Same information. The TOON version uses **53% fewer tokens**. That is not a rounding error. Over a journal with 200 activities — a typical long-running experiment with baseline sampling, continuous during-fault probing, and post-fault recovery — the difference is thousands of tokens. At scale across a CI/CD pipeline running hundreds of experiments per day, the cumulative reduction is substantial.

---

## TOON Design Principles

TOON (Token-Oriented Object Notation) was chosen as Tumult's format for three reasons:

### 1. Structure without ceremony

JSON requires every string to be quoted. Every object opens and closes with braces. Every key-value pair is separated by a colon and quoted. TOON drops the ceremony:

```toon
# TOON
title: Database failover test
tags[2]: database, resilience
estimate:
  expected_outcome: recovered
  confidence: high
```

```json
// JSON equivalent
{
  "title": "Database failover test",
  "tags": ["database", "resilience"],
  "estimate": {
    "expected_outcome": "recovered",
    "confidence": "high"
  }
}
```

The array length hint (`tags[2]`) is a TOON feature that tells parsers the expected cardinality upfront — useful for streaming parsers and for giving LLMs structural context without having to infer it.

### 2. Serde-compatible parsing

Tumult's TOON parsing is built on `serde`, Rust's serialization framework. TOON documents deserialize directly into strongly-typed Rust structs. There is no runtime type coercion, no stringly-typed dictionaries, no "it might be a string or it might be a number" edge cases. If the experiment is malformed, `tumult validate` tells you exactly what is wrong and where.

### 3. Human authoring and machine output share the same format

Experiment definitions and experiment journals use the same TOON format. This means the data pipeline is simple: you author a `.toon` experiment, run it, and get a `.toon` journal. Both are readable in the same tool, parseable by the same library, and queryable with the same analytics queries. No format translation required.

---

## What an AI Agent Can Do With a TOON Journal

Consider a scenario: your CI pipeline runs 50 chaos experiments per night. In the morning, you want to know which experiments showed degraded resilience trends, which estimates were wrong, and which systems need attention.

With a legacy JSON pipeline, this requires either custom tooling to parse the journals, or feeding the full verbose JSON into an LLM — expensive and often exceeding context window limits.

With Tumult, the journal is compact enough to pass directly to an LLM for analysis. Here is a representative journal excerpt that captures the full five-phase outcome:

```toon
experiment_id: 550e8400-e29b-41d4-a716-446655440000
title: PostgreSQL failover recovery validation
status: deviated
started_at_ns: 1705327391000000000
duration_ms: 187432

estimate:
  expected_outcome: recovered
  expected_recovery_s: 15.0
  confidence: high

baseline:
  method: mean_stddev
  samples: 120
  mean: 45.2
  stddev: 3.8
  p95: 52.3

during:
  peak_value: 198.4
  peak_deviation_pct: 339.0
  shape: catastrophic

post:
  recovery_time_s: 47.3
  full_recovery: true
  data_loss_detected: false

analysis:
  estimate_accuracy: 0.0
  estimate_recovery_delta_s: -32.3
  resilience_score: 0.41
```

An LLM can read this and immediately identify: the prediction was wrong (expected `recovered`, got `deviated`), the degradation was catastrophic rather than graceful (the system didn't degrade slowly — it fell off a cliff), and recovery took three times longer than predicted. These are the signals an engineering team needs for the morning briefing.

---

## The Five-Phase Data Model

Tumult structures every experiment as five phases of evidence. Each phase adds a layer to the understanding of what happened:

```
Phase 0: ESTIMATE  — What do we expect will happen?
Phase 1: BASELINE  — What does "normal" look like right now?
Phase 2: DURING    — How did the system degrade under fault?
Phase 3: POST      — How fast did it recover?
Phase 4: ANALYSIS  — How accurate was our prediction?
```

This structure is not cosmetic. It encodes a scientific method into the data model itself:

- Phase 0 forces a hypothesis before observation (the classic scientific requirement of predicting before measuring)
- Phase 1 establishes the baseline against which deviation is measured
- Phase 2 captures the degradation curve with statistical precision
- Phase 3 measures recovery with the same rigor as the baseline
- Phase 4 closes the loop — was the prediction right? Is resilience improving or degrading across runs?

Every field in every phase maps to a `resilience.*` OTel attribute. The journals, the traces, and the analytics tables all use the same attribute names. There is no impedance mismatch between what the experiment produces and what the observability stack stores.

---

## Prediction Tracking: Where Teams Learn

The Phase 0 estimate is the feature that, in our experience, changes team behavior most profoundly.

When you are forced to write down your prediction before the experiment runs — "I expect recovery in 15 seconds, with moderate degradation, high confidence" — and then the experiment runs and recovery takes 47 seconds with catastrophic degradation, something important happens. The gap between expectation and reality is visible, measurable, and attributed to a specific team and a specific system.

Over time, the analysis phase computes estimate accuracy across all runs. Teams that consistently over-estimate their system's resilience are learning that their mental models are wrong. Teams whose estimate accuracy improves over time are building institutional knowledge about how their systems actually behave.

This is what it looks like in SQL:

```sql
-- Which teams have the worst estimate accuracy?
SELECT
    tags->>'team' AS team,
    COUNT(*) AS experiments,
    AVG(CASE WHEN estimate_outcome = actual_outcome THEN 1.0 ELSE 0.0 END) AS outcome_accuracy,
    AVG(ABS(estimate_recovery_s - actual_recovery_s)) AS avg_recovery_error_s
FROM journals
WHERE estimate_outcome IS NOT NULL
GROUP BY tags->>'team'
ORDER BY outcome_accuracy ASC;
```

---

## The Path to Autonomous Chaos Engineering

Tumult's Phase 3 roadmap includes an MCP (Model Context Protocol) server adapter. When complete, this means any AI agent that speaks MCP can:

1. Call `tumult.discover_plugins()` to find available fault injection capabilities
2. Call `tumult.run_experiment(definition)` to execute a chaos experiment
3. Read the TOON journal from the response — compact enough to fit in context
4. Analyze the result and decide what to run next

The AI agent does not need to understand Rust, manage binaries, or parse verbose JSON. It sends structured requests and receives structured, token-efficient responses. Tumult becomes a tool in an agentic quality engineering workflow — one capability among many that an autonomous QE system can invoke.

This is not science fiction. The pieces are in place: the format is designed for it, the data model is designed for it, and the MCP adapter is on the roadmap. What remains is building the adapter layer and connecting it to the agent orchestration frameworks that are already in production use.

---

## What This Means Today

Even before the MCP server ships, the TOON format delivers value:

- **Cheaper LLM analysis**: pass journal files directly to your LLM of choice for post-experiment analysis without hitting context limits
- **Human readability**: engineers can read experiments and journals without a JSON formatter
- **Fast CI integration**: the compact format parses faster, transfers faster, and stores more compactly
- **SQL analytics**: every TOON journal loads directly into DuckDB for structured queries — covered in detail in a future post

The format choice is an investment in the future of the toolchain. As AI tooling matures, Tumult experiments and journals will be natively consumable by the generation of tools being built today.

---

*Next in the series: [Part 3 — Built-In Proof: Native Observability with OpenTelemetry →](./03-built-in-observability.md)*
