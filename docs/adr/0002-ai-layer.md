# ADR 0002: AI analytics layer — deterministic math, governed semantics, LLM narrates

- Status: accepted (Phases 1–2 live: NL query, narrative digests)
- Date: 2026-07-28 (updated 2026-07-28 for Phase 2)

## Context

Krönika's AI layer will offer NL-query, narrative digests and anomaly
explanation over resilience data. Realistic text-to-SQL accuracy (BIRD
benchmark) is 60–75 % first-shot and 85–95 % with grounding; unguarded LLM
SQL against the store is a correctness *and* safety hazard, and log/span
content is untrusted text that can carry prompt injection.

## Decision

**The LLM never decides numbers.** All math is computed deterministically
(in SQL / in Rust); the LLM only narrates a prepared facts package or
translates a question into a query against the governed semantic layer.

Concretely:

1. **Semantic layer first.** The grounding ROI order (semantic layer >
   golden Q→SQL few-shot > schema linking > self-correction > constrained
   decoding) puts `kronika-metrics` at the center: the LLM is steered to
   metric definitions and allow-listed views, not raw tables.
2. **Guardrail pipeline** (`kronika_ai::sql_guard`, best-effort tokenizer,
   documented as such):
   - queries run on a **read-only connection**;
   - **allow-listed tables/views** only;
   - **single statement**, must start with `SELECT`/`WITH`, no `;`, no
     `--`/`/* */` comments;
   - **injected `LIMIT`** + statement timeout;
   - `EXPLAIN` validation before execution (Phase 2);
   - ≤ 3 self-correction retries with execution feedback;
   - the generated SQL is **always shown** to the user.
3. **Narrative digests**: every number in prose must exist in the facts
   package (verified mechanically); digests are JSON-schema-constrained.
4. **Anomaly explanation**: detection is statistical (MAD/rolling-median in
   SQL; STL + robust ESD sidecar for flagged series; before/during/after
   experiment comparisons as the chaos-specific core). The LLM receives an
   evidence package and must cite it.
5. **Eval harness**: a golden-set Q→SQL eval runs in CI before any prompt
   or model change ships.
6. **One LLM interface**: `kronika_ai::Llm` + `OpenAiCompatClient`
   (`KRONIKA_LLM_BASE_URL`/`KRONIKA_LLM_API_KEY`/`KRONIKA_LLM_MODEL`;
   defaults to local Ollama). Batch digests may delegate to smedja later.

## Phases

1. **NL query** — question → governed SQL (guardrails above) → result + SQL shown.
   *Landed:* `/api/ask` (golden answers + LLM path with sql_guard).
2. **Narrative digests** — scheduled reports verbalized from facts packages.
   *Landed:* `kronika_report::narrative` builds a facts package from the
   report's own KPI/table numbers, asks the LLM for a short summary, and
   mechanically keeps only sentences whose numerals are grounded in those
   facts (percent values match in `x` and `x/100` forms; 1% tolerance for
   LLM rounding). Ungrounded sentences are dropped; an unreachable LLM,
   timeout (30s) or a fully ungrounded reply ships the digest unchanged.
   Wired into the daemon's report scheduler and `POST /api/reports/generate`.
3. **Anomaly explanation** — evidence-package-grounded explanations with citations.
4. **Suggested insights** — proactive "what matters" selection.

## Consequences

- Everything the LLM can reach is read-only and allow-listed by construction.
- Selection of *what matters* (which facts enter the package) is the real
  product problem and stays in deterministic, testable code.
- Adding a model/provider is configuration, not code.
