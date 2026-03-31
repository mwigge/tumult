---
title: Token Efficiency
parent: Guides
nav_order: 9
---

# Token Efficiency

Tumult uses TOON as its primary serialization format. One key advantage over JSON is significantly lower token consumption when journals are fed to LLMs.

## TOON vs JSON Token Comparison

TOON achieves 40-50% fewer tokens than equivalent JSON for typical experiment journals. The savings come from:

- No curly braces or square brackets for structure
- No quoted keys
- Indentation-based nesting instead of delimiters
- No commas between values

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
steady-state-hypothesis
  title = "API responds within SLA"
  probes
    - type = "probe"
      name = "api-latency-check"
      provider
        type = "http"
        url = "https://api.example.com/health"
        timeout = 5
      tolerance
        type = "range"
        range = [0, 300]
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
