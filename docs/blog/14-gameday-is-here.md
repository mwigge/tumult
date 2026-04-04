---
title: "GameDay Is Here: From Individual Tests to Compliance Programmes"
parent: Blog
nav_order: 14
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> GameDay Is Here: From Individual Tests to Compliance Programmes

![Tumult Banner](/images/tumult-banner.png)

*Part 14 of the Tumult series. [← Part 13: Load During Chaos](./13-load-during-chaos.md)*

---

Running a single chaos experiment proves a single point of resilience. Your database recovers from a connection kill. Good. But an auditor asking about DORA Article 24 compliance doesn't want to see one test — they want to see a **testing programme**: multiple scenarios, measured outcomes, recovery validation, and evidence that you do this regularly.

That's what GameDay delivers.

---

## What Is a GameDay?

A GameDay is a coordinated campaign of experiments:

```
GameDay: Q2 PostgreSQL Resilience Programme
  ├── Experiment 1: Connection kill under load
  ├── Experiment 2: Container pause (5s total outage)
  ├── Experiment 3: CPU stress injection
  ├── Experiment 4: Memory stress injection
  └── Shared load: k6 running throughout
```

All experiments run in sequence under a shared load generator. They share a single OTel parent trace (`resilience.gameday`). Their results aggregate into a **resilience score** that maps directly to compliance articles.

---

## Creating a GameDay

```bash
tumult gameday create q2-postgres-resilience \
  --load k6 --load-script examples/k6/smoke-test.js \
  --experiments gamedays/pg-connection-kill.toon,gamedays/pg-container-pause.toon,gamedays/pg-cpu-stress.toon,gamedays/pg-mem-stress.toon \
  --framework dora
```

This scaffolds a `.gameday.toon` file:

```toon
title: Q2 PostgreSQL Resilience Programme

regulatory:
  frameworks[2]: DORA, NIS2
  requirements[3]:
    - id: DORA-Art24
      description: ICT resilience testing programme
      evidence: Quarterly GameDay with 4 fault scenarios under shared load
    - id: DORA-Art25
      description: Scenario-based testing of ICT tools and systems
    - id: DORA-Art11
      description: Response and recovery

load:
  tool: k6
  script: examples/k6/smoke-test.js
  vus: 5
  duration_s: 60.0

experiments[4]:
  - path: gamedays/pg-connection-kill.toon
    compliance_maps[1]: DORA-Art25
  - path: gamedays/pg-container-pause.toon
    compliance_maps[2]: DORA-Art25, DORA-Art11
  - path: gamedays/pg-cpu-stress.toon
    compliance_maps[1]: DORA-Art25
  - path: gamedays/pg-mem-stress.toon
    compliance_maps[1]: DORA-Art25

scoring:
  pass_threshold: 0.75
  mttr_target_s: 30.0
  recovery_required: true
```

Each experiment is mapped to specific compliance articles. The scoring config sets thresholds for what constitutes compliance.

---

## Running the GameDay

```bash
tumult gameday run q2-postgres-resilience.gameday.toon
```

The runner:
1. Starts k6 as shared background load
2. Runs each experiment in sequence
3. Stops load, collects metrics
4. Computes resilience score
5. Writes a GameDay journal

---

## The Resilience Score

The score is a weighted aggregate of four components:

| Component | Weight | What it measures |
|-----------|--------|-----------------|
| **Pass rate** | 30% | Fraction of experiments that completed |
| **Recovery compliance** | 25% | MTTR within target, full recovery achieved |
| **Load impact tolerance** | 25% | Error rate during load (lower = better) |
| **Compliance coverage** | 20% | Mapped articles with passing experiments |

Score interpretation:
- **0.90+** → COMPLIANT
- **0.75-0.89** → PARTIAL
- **< 0.75** → NON-COMPLIANT

This isn't an arbitrary number. Each component maps to something an auditor can verify:

- "Did your tests pass?" → pass rate
- "Did the system recover within your declared RTO?" → recovery compliance
- "Did your users experience degradation?" → load impact tolerance
- "Which regulatory articles have you tested?" → compliance coverage

---

## Why This Matters for DORA

The Digital Operational Resilience Act (EU 2022/2554) entered into force on 17 January 2025. It requires financial entities to:

- **Article 24**: Establish an ICT resilience testing programme
- **Article 25**: Execute scenario-based tests including performance and end-to-end testing
- **Article 26**: Carry out threat-led penetration testing (TLPT) for systemically important entities
- **Article 11**: Test response and recovery procedures with measured recovery times

A GameDay maps directly to these requirements:

| DORA Article | GameDay Evidence |
|-------------|-----------------|
| Art. 24 — Testing programme | The GameDay itself — structured, repeatable, documented |
| Art. 25 — Scenario testing | Each experiment is a fault scenario with measured outcome |
| Art. 11 — Response & recovery | MTTR measured per experiment, aggregate recovery score |

The journal is the audit artifact. The resilience score is the compliance metric. The OTel trace is the forensic evidence.

---

## NIS2 Coverage

The same GameDay also covers NIS2 (EU 2022/2555):

| NIS2 Article | GameDay Evidence |
|-------------|-----------------|
| Art. 21(2)(c) — Business continuity | Recovery validated after each fault |
| Art. 21(2)(f) — Effectiveness assessment | Baseline vs chaos comparison via load metrics |

One GameDay, two regulations, documented in a single journal.

---

## Real Results

The Q2 PostgreSQL Resilience Programme ran against a live Docker stack with shared k6 load:

```
GameDay: Q2 PostgreSQL Resilience Programme
Status:  4/4 PASS (COMPLIANT)
Duration: 60.3s

  #1 [PASS] PostgreSQL connection kill under load (2225ms)
  #2 [PASS] PostgreSQL container pause — total unavailability (7394ms)
  #3 [PASS] PostgreSQL CPU stress — resource pressure (10626ms)
  #4 [PASS] PostgreSQL memory stress — resource pressure (8942ms)

Resilience Score: 1.00
  Pass rate:    1.00  Recovery: 1.00  Load: 1.00  Compliance: 1.00

Load (k6): 2,980 requests, p95=101ms, error_rate=0.03%
```

Four fault scenarios. One minute. Full compliance evidence. All under shared load.

## Try It

```bash
curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh
make up-targets
tumult gameday run gamedays/q2-postgres-resilience.gameday.toon
tumult gameday analyze gamedays/q2-postgres-resilience.gameday.toon
```

The resilience score tells the story. The journal is the evidence. The compliance mapping is the bridge between engineering and regulation.

---

*Chaos engineering that proves compliance — not just resilience.*
