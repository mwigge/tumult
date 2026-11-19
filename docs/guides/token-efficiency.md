---
title: Token Efficiency
parent: Guides
nav_order: 15
---

# Token Efficiency

Tumult uses TOON as its primary serialization format. One key advantage over JSON is significantly lower token consumption when journals are fed to LLMs.

## TOON vs JSON Token Comparison

TOON achieves 40-50% fewer tokens than equivalent JSON for typical experiment journals. The savings come from:

- No curly braces, and square brackets only for the length of a list
- Keys are quoted only when they hold a character such as `-`
- Indentation-based nesting instead of delimiters
- No commas between fields

## Example

The same experiment steady-state hypothesis in both formats:

### JSON (approx. 85 tokens)

```json
{
  "steady-state-hypothesis": {
    "title": "API responds within SLA",
    "probes": [
      {
        "type": "probe",
        "name": "api-latency-check",
        "provider": {
          "type": "http",
          "url": "https://api.example.com/health",
          "timeout": 5
        },
        "tolerance": {
          "type": "range",
          "range": [0, 300]
        }
      }
    ]
  }
}
```

### TOON (approx. 45 tokens)

```toon
"steady-state-hypothesis":
  title: API responds within SLA
  probes[1]:
    - type: probe
      name: api-latency-check
      provider:
        type: http
        url: "https://api.example.com/health"
        timeout: 5
      tolerance:
        type: range
        range[2]: 0,300
```

## Rough Numbers

| Format | Avg Tokens (typical journal) | Reduction |
|--------|------------------------------|-----------|
| JSON   | ~1,200                       | baseline  |
| TOON   | ~650                         | ~46%      |

These numbers vary by journal complexity. Larger experiments with many activities see greater savings because TOON eliminates more structural overhead per entry.

## Recommendations

- Store and transmit journals in TOON format to minimize token usage
- Use `tumult export --format json` only when downstream tools require JSON
- When feeding journals to an LLM, prefer TOON or use `tumult_read_journal` via MCP

## Sparse Journal Serialization

Tumult journals omit many optional fields whose value is `None` (or an empty list) from
serialized output. This is done with `#[serde(skip_serializing_if = ...)]` on the fields
below. The phase results of `Journal` (`steady_state_before`, `steady_state_after`,
`estimate`, `baseline_result`, `during_result`, `post_result`, `load_result`, `analysis`,
`regulatory`) are not among them. When absent, they are written as `null`.

| Type | Optional fields omitted when absent |
|------|--------------------------------------|
| `Journal` | `halt`, `blast_radius`, `objectives`, `probe_samples` (when empty), `target_version`, `target` |
| `ActivityResult` | `output`, `error` |
| `BaselineResult` | `sigma` |
| `DuringResult` | `degradation_onset_s`, `degradation_peak_s`, `degradation_magnitude`, `graceful_degradation` |
| `PostResult` | `residual_degradation`, `data_integrity_verified`, `data_loss_detected`, `mttr_s` |
| `ProbeDuring` | `breached_at_ns` |
| `AnalysisResult` | `estimate_accuracy`, `estimate_recovery_delta_s`, `trend`, `resilience_score` |

### Effect on token counts

A minimal successful experiment journal omits the `error` of each step. A phase that did
not run stays in the journal as one line, such as `baseline_result: null`.

### Recommendations

- Do not add `null` fields back for completeness — absent fields are semantically identical to `null`
- Downstream code should use `Option` unwrapping with a default (e.g. `result.mttr_s.unwrap_or(0.0)`)
- When writing custom analytics queries against exported JSON, treat missing keys as `null`
