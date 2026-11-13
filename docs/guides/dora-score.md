# DORA coverage and maturity score

Krönika reads the lake and answers two questions for one legal entity:

1. **Coverage** — for each DORA article and each business function, is there evidence, how fresh is it, and how much can not be seen at all?
2. **Maturity** — an internal level from 1 to 5 for every node, from a workload up to the organisation, with the node that holds the level down named.

The page is **Scores → DORA** (`/scores?view=dora`). The API is `GET /api/dora`, `GET /api/dora/{article}` and `GET /api/maturity`.

Read this first: the score says what evidence exists. It does not say that the entity complies with the regulation, and the page never uses that word. The 1–5 level is an internal scale. No authority has defined it or set a target for it.

## What comes from the regulation and what is a house choice

| From the regulation | House choice |
|---|---|
| Which paragraphs exist and which entity they bind (Regulation (EU) 2022/2554, Delegated Regulation (EU) 2024/1774) | Which evidence counts for a paragraph (the selector of each registry row) |
| The simplified framework of Art. 16 replacing Arts. 5–15 for the entities it names | The weight of a row (1, 2, 3 or 5) and its expected evidence status |
| The carve-outs for microenterprises | Freshness windows where the text gives none, and the decay after the window |
| Threat-led penetration testing (Art. 26) applying only to designated entities | Every threshold, every weight, the 180-day rule, the five pillars, the level gates, the target level and the roll-up |
| The yearly test of Art. 24(6) and of Art. 40 of 2024/1774 | |

Every house number has a name and a default in `tumult-compliance/src/dora/params.rs`, and can be changed per deployment in the `score:` section of the org file (see [House parameters](#house-parameters)).

## The function register

The scoring unit is the function, not the service. Functions are declared in the org file (`KRONIKA_ORG_FILE`, default `<lake dir>/org.yaml`) next to the org hierarchy. The daemon writes the declaration to the lake when it starts.

```yaml
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    profile: simplified        # full | simplified
    flags: []                  # microenterprise, tlpt_designated, csd, ccp, …
    feeds: []                  # incident, slo, register, alert_rules, forecast
functions:
  - id: cif-0001               # the ict.entity.id telemetry carries
    name: Order processing
    tier: critical             # critical | important | other
    org_unit: platform/compute
    rto_s: 3600
    services: ["api-worker*", order-archive]   # service.name globs
```

A declaration is checked as a whole. A duplicate or malformed function id, a LEI with wrong check digits, or an unknown profile, flag, feed, tier or org unit refuses the file, and the lake keeps its previous register.

Three things follow from the register:

- **Profile.** The profile selects the registry rows. Under `simplified` the Arts. 5–15 rows are not applicable and the Art. 16 and 2024/1774 rows apply instead. Art. 26 rows apply only with the `tlpt_designated` flag. `GET /api/dora?profile=full` scores the same lake under another profile without changing the register.
- **Feeds.** A feed that is not listed is not connected, and the rows that need it are *unknown*. A listed feed with nothing in it gives *no events*.
- **Register codes.** Each function gets a code `F1`, `F2`, … for the register of information. A code is stored once and never renumbered, also when functions are removed or reordered.

### The form of `ict.entity.id`

The id is lower-case groups joined by `-`, at most 63 characters: `^[a-z][a-z0-9]*(-[a-z0-9]+)+$`, for example `cif-0042`. The register refuses any other form. Telemetry that carries another form (for example `CF-0042`) is kept exactly as received and flagged; it is never lower-cased, because two systems that disagree about case must be fixed at the source. `GET /api/otel/conformance` lists the flagged ids per service under `malformed_entity_ids`.

## Coverage

For each function `f` and registry row `r`, the best evidence gives a value

```
v(f, r) = max over evidence of  freshness · outcome · strength
```

- **Freshness** is 1 inside the row's window and decays to 0 over a further quarter of the window.
- **Outcome** is 1 for a passed test, 0.75 for a partial one and 0.5 for a failed one. A failed test is still a test.
- **Strength** is 1 when the evidence is direct for the row, 0.75 when supporting, 0.25 when indirect; a manual record of a test counts 0.75.

Coverage of an article is the mean of `v` over its cells, weighted by row weight and by function criticality (critical 3, important 2, other 1.5). Three states are left out of that ratio and shown on their own:

| State | Meaning | On the heatmap |
|---|---|---|
| `na` | The row does not apply to the profile or to the function's tier | Omitted; the footer counts the rows |
| `unknown` | The evidence class is not connected, so Krönika can not see either way | Hatched |
| `no_events` | The feed is connected and nothing happened in the window | A dash, "no events in window" |

No number is shown without its unknown share. A coverage of 90 % with an unknown share of 60 % describes less than half of the rows.

### Evidence status

Each cell also has a status, which is what a reviewer asks for:

| Status | Rule |
|---|---|
| None | Observable, nothing declared, no evidence |
| Designed | A target, plan or schedule is declared; no qualifying evidence in the window |
| Existing | `v ≥ 0.75` today |
| Operating | `v ≥ 0.75` on every one of the last 180 days |

A single day below the threshold starts the count again. A cell at Existing shows the days left until Operating. The daemon stores each day's status (`KRONIKA_SCORE_SNAPSHOT_INTERVAL`, default `24h`; `0` or `off` disables it). Days that were never stored are computed from the evidence in the lake as it stood on that day.

Each row has an expected status: Operating for weight-5 and weight-3 rows under the full profile, Existing elsewhere. The gap is the distance to it. The page words this as "meets the house expectation".

## Maturity

Five pillars, each 0–100, or `unknown`, `no events` or `not declared`:

| Pillar | Measures |
|---|---|
| C, conformance | How far the services' telemetry follows the naming contract, up to level 4 (level 2 under the simplified profile or for a microenterprise) |
| T, coverage | Full profile: coverage of function × test discipline over a year. Simplified: the yearly continuity or restore test per function. Microenterprise: planned tests executed within their interval |
| S, objectives | Attainment of declared service-level objectives. Not declared under the simplified profile is left out, not counted as unknown |
| R, recovery | Share of critical and important functions with a measured recovery inside the target in the last year |
| L, loop | Share of incidents and test findings closed by a passing run |

```
index = 1 + 4 · Σ weight · pillar / (100 · Σ weight)
level = min(floor(index), highest gate passed)
```

An unknown pillar counts as 0 in the index. `no events` and `not declared` are left out and the weights are renormalised. The gates are conditions a level needs regardless of the index; for example level 2 needs registered functions, the ICT spine on half of the services and T ≥ 25, and level 3 needs R ≥ 60 and the weight-5 rows at Existing for every critical and important function, with 80 % of them Operating. The page lists each gate with the conditions that fail.

The level is always printed with its target and profile, "level 2 of target 3 (simplified)", and with the reason when something holds it below the index.

## Maturity per node and the cap

The same pillars are computed for every node on the spine: workload → service → component or system → function → org unit → organisation. A node uses only the pillars that can be observed at its kind.

```
index(n) = ω · own(n) + (1 − ω) · mean of the children's indexes, weighted by criticality
level(n) = min(floor(index(n)), gate(n), level of the weakest critical child)
```

Two rules decide how to read it:

- **Unknown counts at the floor.** A node that is known to exist but has no telemetry and no test (a declared service that nothing reports as, a workload in the manifests with no telemetry) enters its parent's mean at 1.0 with its full weight. It is never skipped.
- **The weakest critical child caps the level.** A parent can not be at a higher level than a critical child. An unknown critical child is at level 1, so it holds everything above it at level 1.

### How to read the cap

`GET /api/maturity` returns the cap at the top level and on every node:

| Field | Meaning |
|---|---|
| `cap.rule` | `weakest_critical_child`, `gate`, or `none` |
| `cap.level` | The level the cap allows |
| `cap.node_ids` | The direct children that set the cap |
| `cap.origin` | The deepest nodes on the chain: the ones to fix |
| `cap.reason` | The same in words |

Example: `level 1, capped by svc:order-archive` with reason `capped by svc:order-archive (critical, no telemetry and no test)`. The critical function declares a service that sends nothing. Instrument the service or run a test against it and the cap lifts; the level then follows the index and the gates, and the reason names the gate that holds the next level ("level 3 blocked by: recovery (R) is 43, needs 60").

The document carries both views. `level` and `index` are the rolled-up values for the organisation. `flat_level` and `flat_index` are the pillars alone for the legal entity, before the roll-up. When they differ, the difference is what the tree adds: weak or unknown nodes that the entity-wide pillars average away.

`by=node` returns every node, `by=entity` the functions and what is above them, `by=org` the org units. `nodes[].drag` lists children that lower a node's index without capping it.

## API

| Request | Returns |
|---|---|
| `GET /api/dora?lei=&profile=&by=entity\|org&env=` | Portfolio coverage, articles with their rows, the function × article cells, the expectation table, not-applicable rows, reporting readiness |
| `GET /api/dora/{article}?entity=&lei=&profile=&env=` | One article with every evidence row behind each cell |
| `GET /api/maturity?lei=&profile=&by=node\|entity\|org&env=` | Level, index, pillars, gates, cap, history by quarter and the per-node roll-up |
| `GET /api/otel/conformance` | Contract conformance per service and `malformed_entity_ids` |

`lei` selects the legal entity when the register holds more than one. A scoped principal sees only its own environments.

## House parameters

All of these are fields of `HouseParams` and can be overridden in the org file. Missing fields keep their defaults; an unknown field refuses the file.

```yaml
score:
  operating_days: 180
  targets: {simplified: 3}
  gates: {l3_recovery: 60}
  rollup: {unknown_floor: 1.0}
```

| Parameter | Default | Meaning |
|---|---|---|
| `criticality` | critical 3, important 2, other 1.5 | Weight of a function |
| `outcome` | passed 1, partial 0.75, failed 0.5 | Outcome factor |
| `strength` | direct 1, supporting 0.75, indirect 0.25, manual_test 0.75 | Strength factor |
| `freshness_decay_fraction` | 0.25 | Decay period after the window, as a share of the window |
| `existing_threshold` | 0.75 | `v` needed for Existing |
| `operating_days` | 180 | Unbroken days needed for Operating |
| `operating_share` | 0.8 | Share of gating cells that must be Operating for level 3 |
| `kind_weights` | chaos 3, dr 3, load 2, performance 1, robustness 1, security 1, nfr 1 | Discipline weights in T |
| `manual_kinds` | failover → dr, drill → dr, gameday → chaos, pentest → security | Discipline of a manual exercise type |
| `test_window_days`, `recovery_window_days`, `loop_window_days` | 365 each | Windows of T, R and L |
| `loop_closure_weight`, `loop_previously_tested_weight` | 0.75, 0.25 | The two parts of L |
| `slo` | blocking 3, major 2, minor 1; low_budget 0.25, low_budget_credit 0.75 | Weights and low-budget credit in S |
| `pillar_weights.full` | C 0.20, T 0.25, S 0.25, R 0.20, L 0.10 | Pillar weights, full profile |
| `pillar_weights.reduced` | C 0.20, T 0.30, S 0.15, R 0.30, L 0.05 | Pillar weights, simplified profile or microenterprise |
| `conformance_ceiling` | full 4, reduced 2 | Highest conformance level that counts |
| `targets` | full 3, full_tlpt_designated 4, full_microenterprise 2, simplified 3, simplified_microenterprise 2 | Target level |
| `gates` | l2_c1 50, l2_coverage 25; l3_c1 80, l3_c2 50, l3_coverage 60, l3_slo 80, l3_slo_history_days 180, l3_recovery 60, l3_max_unknown_share 0.5; l4_c3 80, l4_coverage 80, l4_slo 90, l4_recovery 80, l4_loop 60; l5_c4 90, l5_coverage 90, l5_slo 95, l5_recovery 90, l5_loop 85 | Gate thresholds |
| `rollup.own_share` | workload 1, service 0.5, component 0.3, system 0.3, entity 0.5, org 0 | ω per node kind |
| `rollup.own_pillars` | workload C T R; service C T S; component and system T R; entity T S R L; org none | Pillars a node kind scores itself on |
| `rollup.unknown_floor` | 1.0 | Index of an unknown node |
| `rollup.unassigned_weight` | 1.0 | Weight of a service with no function, and of an org unit with no function |
| `rollup.critical_inheritance` | true | A node that supports a critical function is critical |
| `conformance` | window_days 90; level1 … level4 key lists | Attribute keys each conformance level needs on every span |
| `expected_status` | empty | Per-row override of the expected status |
| `simplified_test_row` | `rts1774.art40` | The row T reads under the simplified profile |
| `management_report_rows` | Art. 13(5), 6(5) and 16(2) rows | Records that satisfy the level-4 management report condition |
| `tlpt_row` | `dora.art26.1` | The attestation level 5 needs for a designated entity |
| `band_good`, `band_fair` | 0.8, 0.5 | Coverage bands |

Outside the org file: `KRONIKA_ORG_FILE` (where the register is read from), `KRONIKA_SCORE_SNAPSHOT_INTERVAL` (the daily snapshot), `KRONIKA_MANIFESTS_DIR` (workload nodes). The id pattern and its length limit are constants in `tumult-core` (`ENTITY_ID_PATTERN`, `ENTITY_ID_MAX_LEN`). The history shows four quarters.

## Limits

- Incident and objective feeds are read as connected and empty when declared; incident and SLO records are not ingested yet. Loop closure and objective attainment therefore read `no events` or `not declared` on a live lake.
- The history re-scores the evidence of each quarter end against today's register.
- The reporting-readiness panel shows register completeness. It does not produce the register of information or an incident report.

## Proof

`scripts/proof/dora-score/proof.sh` starts a daemon on a fresh lake, posts telemetry and backdated test runs over OTLP, and asserts the behaviour described above: the registry per profile, the register codes across a restart, unknown before the incident feed and no events after it, Existing at 179 days and Operating at 180, the cap by an uninstrumented critical service and its lifting, and the flagged `CF-0042`. The two worked examples of the model (level 4 at index 4.42 under the full profile; level 2 at index 3.60 under the simplified one) are fixture inputs and are asserted by their tests.
