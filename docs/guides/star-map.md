# Star Map

The Star Map is one graph of everything the platform knows: the
organisation at the centre, then ICT entities, systems, components,
services and workloads, with experiment definitions as satellites. Three
lenses colour the same nodes by evidence, by risk and by telemetry
conformance. It replaces the Topology page; `/topology` redirects to
`/starmap`.

Nothing is stored for it. Each request assembles the map from what the
lake already holds (spans, runs, verified manual records, risk
suppressions, ChaosGraph tables), from `org.yaml` and from the manifests
under `KRONIKA_MANIFESTS_DIR`.

## Where nodes come from

| Node | Id | Source |
|---|---|---|
| Organisation unit | `org:<path>` (`org:` is the root) | `org.yaml` nodes |
| ICT entity | `ent:<id>` | `ict.entity.id` on telemetry or on a workload label |
| System | `sys:<environment>/<system>` | `resilience.target.system` on telemetry, runs, manual records, workload labels |
| Component | `cmp:<environment>/<system>/<component>` | `resilience.target.component` |
| Service | `svc:<service.name>` | spans in the window |
| Workload | `wl:<namespace>/<kind>/<name>` | manifests |
| Definition | `def:<registry id or name>` | registered definitions and the experiments that ran |

Entity ids are compared in lowercase: `CIF-0042` and `cif-0042` are the
same entity, and `cif-0042` is the form to write. A system without an
environment gets `-` in its place.

A workload joins the map without telemetry when its manifest carries the
naming contract as labels (`resilience.target.system`,
`resilience.target.component`, `resilience.target.environment`,
`ict.entity.id`, and optionally `ict.entity.tier` and
`ict.entity.owner`). Without them it is matched to a system by its name or
its `app` label.

## Ownership

An entity is placed under an organisation unit by the first of these that
gives an answer:

1. `ict.entity.owner` on its telemetry or workload label, when it names a
   unit in `org.yaml`.
2. An `entities` glob in an `org.yaml` assignment.
3. The experiment-name assignment (`targets`) of the experiments that ran
   against its systems.
4. Otherwise `org:(unassigned)`.

```yaml
assignments:
  - team: data-team
    targets: ["message-queue*"]
    entities: ["cif-0002", "cif-01*"]
```

Globs are case-insensitive and the first matching assignment wins. A
service whose telemetry names more than one entity carries
`conflict: true`.

## Lenses and marks

- **Evidence** (default): a system is green when every run in the window
  passed and red when one failed. A dashed hollow node has no evidence at
  all; a thin amber dash means the evidence is stale.
- **Risk**: fill by the worst open finding of the passive risk scan. A
  node whose findings are all suppressed is dimmed; its tooltip says who
  accepted the risk, until when and why, and the border turns amber when
  the suppression expires within fourteen days. Suppressed findings are
  counted next to the open ones in the legend, never hidden.
- **Conformance**: services by the level `GET /api/otel/conformance`
  reports (`spine`, `target`, `none`).

Click a node for its evidence drawer (runs, gate verdict, objective
verdicts, manual records, findings). Double-click, or "Focus here", to
show only its neighbourhood. Range, environment, lens, selection and focus
are in the URL, so a view can be shared as a link.

## API

`GET /api/starmap?range=&env=&focus=&depth=&kinds=&budget=&include=`

| Parameter | Values | Default |
|---|---|---|
| `range` | `24h`, `7d`, `14d`, `30d` | `7d` |
| `env` | one environment; `403` when it is outside the caller's scopes | the caller's scopes |
| `focus` | a node id; only its neighbourhood is returned | none |
| `depth` | `1` to `4`, with `focus` | `2` |
| `kinds` | comma-separated node kinds to keep | all |
| `budget` | `1` to `5000` nodes; deeper rings collapse into `cluster:` nodes per organisation unit beyond it | `1500` |
| `include` | `runs` adds one node per run | none |

The response has `nodes`, `edges` (`owns`, `implements`, `runs_on`,
`calls`, `depends_on`, `tested_by`, `broke`, `covers`, `measures`, each
with the `source` it was learned from), `clusters`, `counts` and
`sources`. The same data in any order gives the same response.
`GET /api/topology` remains as a compatibility view.

## What is not there yet

`sources` reports `slo`, `dora` and `maturity` as `unavailable`, and the
matching node fields are `null`. They are reserved for the SLO, DORA and
maturity overlays.

When the maturity overlay arrives, a parent's level is rolled up from its
children by one rule: the criticality-weighted mean of the children,
capped by the weakest critical child, with a child of unknown maturity
counted at the lowest level so that it lowers the parent rather than
disappearing from it. A unit can therefore never look more mature than its
most important system, and missing evidence is never read as good news.

## Proof

`scripts/proof/starmap-step1/proof.sh` starts a fresh daemon with the demo
`org.yaml` and manifests, runs the demo experiments through the CLI and
asserts on the map: the organisation and its teams, workloads with
`runs_on` edges, every definition referenced, the untested workload as a
gap, the failed run on its system and a suppressed finding with its
expiry.
