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

The scoring unit is the function, not the service. Functions are declared in the org file (`KRONIKA_ORG_FILE`, default `<lake dir>/org.yaml`) next to the org hierarchy. The daemon writes the declaration to the lake when it starts, and reads the file again every `KRONIKA_REGISTER_RELOAD_INTERVAL` (default `60s`; `0` or `off` reads it at start only), so a change needs no restart.

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

A declaration is checked as a whole. A duplicate or malformed function id, a LEI with wrong check digits, an unknown profile, flag, feed, tier or org unit, or a house parameter out of its range refuses the file, and the lake keeps its previous register.

What the org file says about a function is a declaration, and a declaration scores nothing. A `test_plan`, `scheduled: true` or a `last_assessed` date alone gives the rows that read them (the testing programme of Art. 24(1), the criticality assessment) coverage 0 and the status Designed, with a reason that says it is declared only. They count from the day the lake holds a run of the plan or a record that cites the row. `rto_s` and `rpo_s` are the targets a stored measurement is compared with, and the criticality tier is a weight; neither is evidence.

Every score response and the page say where the register stands:

| `register_state` | Meaning |
|---|---|
| `ok` | The lake holds the register of the org file |
| `stale` | The org file is valid and changed, and is not written to the lake yet; `register_reason` says why when a write failed |
| `invalid` | The org file was refused. `register_reason` has the reason, the lake keeps the previous register, and the house parameters fall back to the defaults (see [House parameters](#house-parameters)) |
| `none` | There is no org file and so no register |

Three things follow from the register:

- **Profile.** The profile selects the registry rows. Under `simplified` the Arts. 5–15 rows are not applicable and the Art. 16 and 2024/1774 rows apply instead. Art. 26 rows apply only with the `tlpt_designated` flag. `GET /api/dora?profile=full` scores the same lake under another profile without changing the register. Such a response carries `profile_overridden: true` and `registered_profile`, and the page shows a banner above the numbers; stored history and the daily snapshot are always of the registered profile.
- **Feeds.** `feeds` records which evidence feeds the entity says it has. A declaration is never scored. A feed counts only through records of it stored in the lake, and this release ingests none of the five, so every row that needs one is *unknown* and every gate that needs one stays shut, whatever is listed. Each response lists the feeds with `declared`, `connected` and the reason.
- **Register codes.** Each function gets a code `F1`, `F2`, … for the register of information. A code is stored once and never renumbered, also when functions are removed or reordered.

### The form of `ict.entity.id`

The id is lower-case groups joined by `-`, at most 63 characters: `^[a-z][a-z0-9]*(-[a-z0-9]+)+$`, for example `cif-0042`. The register refuses any other form. Telemetry that carries another form (for example `CF-0042`) is kept exactly as received and flagged; it is never lower-cased, because two systems that disagree about case must be fixed at the source. `GET /api/otel/conformance` lists the flagged ids per service under `malformed_entity_ids`.

A malformed id is bounded wherever it travels. Ingest stores at most 63 characters of it; a longer one is cut and ends in `~`, which is outside the canonical form, so a cut value never reads as a valid id. A malformed id never becomes a function, a node or a label of the score. The conformance list shows at most 64 characters of an id (`truncated: true` on the row when the stored one was longer) and at most 500 rows (`malformed_entity_ids_limit`; `malformed_entity_ids_truncated: true` when there were more). Each one received is counted in `tumultd_malformed_entity_ids_total` on `/metrics`, and the daemon log has at most one warning per minute with the number it left out. An `ict.entity.id` set on a span is treated exactly like one set on the resource: cut, flagged and counted the same way.

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
| `no_events` | Records of the feed are stored and none falls in the window. No feed is ingested in this release, so a live lake does not show this state | A dash, "no events in window" |

No number is shown without its unknown share. A coverage of 90 % with an unknown share of 60 % describes less than half of the rows.

### Evidence status

Each cell also has a status, which is what a reviewer asks for:

| Status | Rule |
|---|---|
| None | Observable, nothing declared, no evidence |
| Designed | A target, plan, schedule or assessment date is declared; no qualifying evidence in the window. A declaration never gives more than this |
| Existing | `v ≥ 0.75` today |
| Operating | `v ≥ 0.75` today and on each of the last 180 days, as the daemon stored them |

The daemon stores each day's status (`KRONIKA_SCORE_SNAPSHOT_INTERVAL`, default `24h`; `0` or `off` disables it), and only stored days count. The streak is the number of days in a row, back from yesterday, that have a stored row at or above the threshold. A day below the threshold and a day that was never stored both start the count again: a day the daemon did not run is a day nobody observed, and it is never filled in afterwards from the evidence. A test that is 180 days old on a lake that has stored no day is therefore Existing with 180 days to go. A cell at Existing shows its streak and the days left until Operating. With the snapshot disabled nothing reaches Operating.

The job looks for a new UTC day every minute as well as on its interval, so a day is not stepped over because the daemon started late in the previous one. A day is stored only when it was scored with the house's own parameters. While the org file is refused and the daemon has never read a valid one, the score is computed with the defaults (see [House parameters](#house-parameters)) and no day is stored: `snapshot.skipping` is `true` in every score response, `snapshot.reason` says why, `snapshot.skipped` counts the days left out since the daemon started (`tumultd_score_snapshots_skipped_total` on `/metrics`), the daemon log has a warning per skipped day and the page shows a notice. Those days stay missing, as any day that was not observed.

The daily rows are kept for 800 days and older ones are deleted when a day is stored. The streak reads the last 183 days and the history at most 458, so both stay whole.

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

The level is always printed with its target, its profile and its unknown share, "level 2 of target 3 (simplified), 21 % unknown", and with the reason when something holds it below the index. The share in the headline is the one of the rolled-up index the level is read from (`unknown_share`); the share of the pillars alone is `flat_unknown_share` and the page shows both.

### History

`history` has the last four finished quarters and today. A past quarter shows the level, index and unknown share the daily job stored on the last stored day of that quarter (`state: stored`, with `as_of`). Nothing is recomputed, so a later change of the register or of a parameter does not rewrite the past. A quarter with no stored day is `state: no_data` with a reason and no level; the chart draws a gap there and the table under it says so. A profile override and a scoped caller get `no_data` for every past quarter, because stored days are of every environment under the registered profile.

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

`lei` selects the legal entity when the register holds more than one. A principal scoped to some environments that asks for a `lei` with no function in those environments gets the answer an unknown `lei` gets (`400`, "is not registered", without the entity's name, profile or feeds), so the register can not be listed by guessing.

A principal scoped to some environments gets a partial view and is told so: `scoped: true` and a `scope_note`, shown as a banner on the page. It lists only the functions with telemetry or evidence in those environments; the level is of that part alone; register data, stored history and the Operating status are not shown.

Every score response carries what it was computed with: `params` (see [House parameters](#house-parameters)), `snapshot` (whether days are being stored), `proof_days` (see [Proof](#proof)), `register_state` with `register_reason`, `feeds`, `profile_overridden` and `scoped`. Rows of the expectation table and of an article carry `scope_basis` and `window_days`. `scope_basis: house` marks a row that the house limits to critical or important functions where the provision does not (the Art. 24 rows); the page labels it "house scope". `window_days: null` means the row has no freshness window for this entity.

The score endpoints reuse a response for `KRONIKA_SCORE_CACHE_TTL` seconds (default `5`; `0` turns it off), measured on a monotonic clock, so a step of the wall clock does not keep one alive. A reload of the org file or the manifests and a stored snapshot drop the cached responses at once; ingested telemetry does not, so a score can be that many seconds behind the lake. The manifests are read at start and on each reload, not per request.

## House parameters

All of these are fields of `HouseParams` and can be overridden in the org file. Missing fields keep their defaults; an unknown field refuses the file, and so does a value outside its range (for example a share or factor outside 0–1, a negative weight, pillar weights that do not sum to 1, a gate threshold outside 0–100 or lower than the same gate one level down, a level outside 1–5). A threshold can not be switched off: `existing_threshold`, `operating_share`, the bands and the gate thresholds must be above 0, `gates.l3_max_unknown_share` must be below 1, and a day count (`operating_days`, the windows, `gates.l3_slo_history_days`) must be within 1 and 3650.

A house may lower a gate, and nobody reading the score may miss that it did. Every response has a `params` block and the page shows it under the headline:

```json
"params": {
  "source": "org_file",
  "fallback": false,
  "held": false,
  "reason": null,
  "overridden": [{"key": "operating_days", "value": 30, "default": 180}]
}
```

`overridden` lists every key that differs from its default, with both values.

When the org file is refused, `register_state` is `invalid`, `reason` says what was refused, and the parameters depend on what the daemon has read before:

- It read a valid file earlier in this run: those parameters stay in force. `source` is `last_valid_org_file`, `held` is `true`, `overridden` still lists what that file overrides, and the daily snapshot goes on. A typo in the file does not silently move the house back to the defaults.
- It has not read a valid file since it started: the score is computed with the defaults, `source` is `defaults` and `fallback` is `true`, and no day is stored until a valid file is read (see [Evidence status](#evidence-status)). The held parameters are kept in memory only, so a restart with a refused file is this case.

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
| `manual_kinds` | failover → dr, drill → dr, gameday → chaos, pentest → security, tlpt → security | Discipline of a manual exercise type |
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
| `rollup.unknown_floor` | 1.0 | Index of an unknown node. Fixed: any other value is refused, and an unknown node is level 1 |
| `rollup.unassigned_weight` | 1.0 | Weight of a service with no function, and of an org unit with no function |
| `rollup.critical_inheritance` | true | A node that supports a critical function is critical |
| `conformance` | window_days 90; level1 … level4 key lists | Attribute keys each conformance level needs on every span |
| `expected_status` | empty | Per-row override of the expected status |
| `simplified_test_row` | `rts1774.art40` | The row T reads under the simplified profile |
| `management_report_rows` | Art. 13(5), 6(5) and 16(2) rows | Records that satisfy the level-4 management report condition |
| `tlpt_row` | `dora.art26.1` | The attestation level 5 needs for a designated entity |
| `band_good`, `band_fair` | 0.8, 0.5 | Coverage bands |

Outside the org file: `KRONIKA_ORG_FILE` (where the register is read from), `KRONIKA_REGISTER_RELOAD_INTERVAL` (how often it and the manifests are read again), `KRONIKA_SCORE_SNAPSHOT_INTERVAL` (the daily snapshot), `KRONIKA_SCORE_CACHE_TTL` (how long a response is reused), `KRONIKA_MANIFESTS_DIR` (workload nodes), `KRONIKA_SCORE_PROOF_DAYS` (a test setting, see [Proof](#proof); leave it unset). The id pattern and its length limit are constants in `tumult-core` (`ENTITY_ID_PATTERN`, `ENTITY_ID_MAX_LEN`). The history shows four quarters.

## Limits

- Incident, SLO, register-of-information, alert-rule and forecast records are not ingested. On a live lake loop closure (L) is `unknown` and counts 0; objective attainment (S) is `unknown` under the full profile and `not declared` under a reduced one; the Art. 28(3) row is `unknown`; and the alert-rule and forecast conditions of levels 4 and 5 do not hold. A pillar that is unknown or left out of the index never passes a gate, so a live lake reaches level 3 at most.
- The history and the Operating status start on the day the daemon first stored a snapshot. Quarters before it are `no data`, and no status is Operating until 180 days have been stored.
- Art. 26(1) is scored only by a manual record of type `tlpt`. A `pentest` record is a penetration test under Art. 25 and does not count for it.
- The reporting-readiness panel shows register completeness. It does not produce the register of information or an incident report.

## Upgrading and rolling back

The score tables arrive with lake schema v20 (`ict_legal_entities`, `ict_functions`, `ict_function_services`, `requirement_status_daily`, `maturity_daily`). The change is additive, and the first start of a new binary migrates the lake in place. There is no downgrade: treat a lake that a v20 binary has opened as not readable by an older binary. The older binary has no check that refuses it; it does not know the new tables, and it fails on rows it can not parse, such as a manual record of type `tlpt`.

To be able to go back:

1. Before the upgrade, stop the daemon and run `tumult store backup --output <dir>`.
2. To roll back, stop the daemon, install the older binary and run `tumult store restore --input <dir> --output <new-db>` with it, then point `TUMULT_LAKE_PATH` at the restored file.
3. What was ingested after the backup is in the v20 lake only. Keep that file; a later upgrade can open it again.

Without a backup from before the upgrade there is no supported way back to the older binary.

## Proof

`scripts/proof/dora-score/proof.sh` starts a daemon on a fresh lake, posts telemetry and backdated test runs over OTLP, and asserts the behaviour described above: the registry per profile, the register codes across a restart, Art. 19 unknown and the level unchanged when every feed is declared, a 180-day-old test that is Existing until 180 days are stored, a missed day that breaks the streak, Existing at 179 stored days and Operating at 180, the cap by an uninstrumented critical service and its lifting, history from stored days with `no data` before them, the flagged `CF-0042`, and an org file changed under the running daemon (a lowered `operating_days` named in `params`, then a refused file that leaves those parameters in force as `held`), and a daemon started on a refused file (defaults in force, the daily snapshot skipped and counted). The two worked examples of the model (level 4 at index 4.42 under the full profile; level 2 at index 3.60 under the simplified one) are fixture inputs and are asserted by their tests.

The proof listens on two ports that are free when it starts and checks before every write that the listener is the daemon it started. The stored days of the Operating steps are written by `dora_store_status_days` (`tumult-api/examples/`), a test-only tool that runs the daemon's daily job with the clock set to each past day against the proof's private lake, with the daemon stopped. It refuses a lake that is not also named in `DORA_PROOF_PRIVATE_LAKE`. No endpoint of the daemon stores a past day.

The tool marks every row it writes with the source `proof`, and the score leaves such days out: they do not extend a streak and are not in the history. They count only when the daemon is started with `KRONIKA_SCORE_PROOF_DAYS=1` (or `true`; any other value is off). That is a test setting. With it on, the daemon logs a warning at start, every score response has `proof_days.enabled: true` with the number counted and a `note` saying that the Operating status and the history are not evidence of operation, the page shows the note above the numbers, and a history point stored by the tool has `source: proof` and reads "by the proof tool" in the table. With it off, `proof_days.ignored` is the number of such days in the lake that were left out, and the page mentions it. The proof shows both: the stored days count for nothing until the daemon is restarted with the setting.
