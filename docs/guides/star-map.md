---
title: Star Map
parent: Guides
nav_order: 28
---

# Star Map

The Star Map is one graph of everything the platform knows: the
organisation at the centre, then ICT entities, systems, components,
services and workloads, with experiment definitions as satellites. Three
lenses ring the same nodes by evidence, by risk and by telemetry
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
| Definition | `def:<registry id>`, or `def:name/<name>` when only the name is known | registered definitions and the experiments that ran |

An id segment keeps the name as reported, except that `%`, `/` and `|`
are written `%25`, `%2F` and `%7C`, so a name can never be mistaken for
structure. A system without an environment gets `-` in its place (an
environment that is literally called `-` is written `%2D`).

Entity ids compare without regard to case, the way `org.yaml` patterns
match them: `CIF-0042` and `cif-0042` are one node, `ent:cif-0042`. The
node id carries the lower-case form and the label a spelling that was
reported: the agreed one when anybody reports it, else the first in byte
order. The agreed form is lower-case segments joined by `-`, starting
with a letter, at most 63 characters (`cif-0042`); an entity that any
reporter spells otherwise carries the flag `malformed_id` and is still
shown. Nothing else about an id is rewritten.

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

Globs are case-insensitive and the first matching assignment wins; an
empty glob is rejected when `org.yaml` is loaded. The Star Map and the
score tree use the same matcher. In step 3 the experiment names are walked
in byte order and the first one that has an owner decides, so the answer
does not depend on the order runs arrived in.

A service whose telemetry names more than one entity carries
`conflict: true`. An entity carries `flags` when its ownership should not
be taken at face value:

| Flag | Meaning |
|---|---|
| `multiple_owners` | telemetry reports more than one `ict.entity.owner` for it |
| `owner_conflict` | telemetry names an owner other than the one `org.yaml` assigns |
| `malformed_id` | the id, in at least one reported spelling, does not have the agreed form |

## Gaps and roll-ups

A system is a gap when it has no evidence in any discipline (`missing`) or
only stale evidence (`stale`). An entity shows the best of its systems:
one tested system clears the entity's own gap. So that this does not hide
anything, every entity and organisation unit carries a `rollup` that
counts what sits below it: `systems`, `untested_systems`, `stale_systems`,
`failed` runs, `risks_open`, `risks_suppressed` and `gaps`. The tooltip and the drawer show
it as "5 of 6 systems untested" on the node. `counts.gaps` is taken before
clustering, so it does not change with `budget`, and
`counts.risks_unplaced` says how many findings name a target that is not
on the map. A token scoped to environments counts only findings in those
environments: a finding on a node hidden from it, or one with no
environment at all, is not counted.

## Lenses and marks

Every node is a circle in the colour of its kind, with its name below.
An arrow is a relation and carries its name (`OWNS`, `IMPLEMENTS`,
`CALLS`, `RUNS_ON`, `DEPENDS_ON`, `TESTED_BY`, `BROKE`, `COVERS`,
`MEASURES`). A thin grey arrow named `PART_OF` points from a node to the
parent it sits under, when no other relation joins the two. It is drawn
from the `parent` of the node; the API has no such edge.

Each relation has its own colour and its own dash, so an arrow tells
its relation also without a name on it. `BROKE` is thick and red,
`PART_OF` thin and grey.

A relation name is written on the arrow where it has room and lands on
no other caption: in the middle, nearer one end, or below the line.
Hover or select a node to write the name on every one of its links; the
rest of the map is dimmed then. Hover an arrow to read its name and its
two ends. When two neighbours have the same name, each shows its kind in
small text under the name. A long name is cut short, more so when the
map is drawn small; the tooltip has the full name.

Under the map are two rows of chips. The first has each kind and its
count, the second each relation with its line, its name and its count.
Click a chip to hide that kind or that relation, and again to show it.

A lens changes the ring of a node and the mark before its name, never
its colour:

- **Evidence** (default): a green ring and a check mark when every run in
  the window passed, a thick red ring and a cross when one failed. An
  aborted or interrupted run proves nothing and is not counted. A
  dashed hollow node has no evidence at all; a short amber dash means the
  evidence is stale.
- **Risk**: an amber ring for an open finding of the passive risk scan, a
  thick red ring for a high or critical one. A node whose findings are
  all suppressed is faded with a dotted ring; its tooltip says who
  accepted the risk, until when and why, and the ring turns amber when
  the suppression expires within fourteen days. Suppressed findings are
  counted next to the open ones in the legend, never hidden.
- **Conformance**: services by the level `GET /api/otel/conformance`
  reports (`spine`, `target`, `none`).

Click a node for its evidence drawer (runs, gate verdict, objective
verdicts, manual records, findings). Double-click, or "Focus here", to
show only its neighbourhood. Range, environment, lens, selection and focus
are in the URL, so a view can be shared as a link.

Everything the chart offers is also reachable without a pointer: the node
list beside it has a filter and one button per node that opens the same
drawer. Opening the drawer moves focus to its title, Escape closes it and
focus returns to where it was.

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
`sources` (each `ok` or `unavailable`). With `focus`, `focus_found` says
whether that node exists inside the caller's scopes, and `focus` in the
response is the id of the node that was looked for: an entity id is
matched whatever its case, so `focus=ent:CIF-0042` finds `ent:cif-0042`
and the response says `"focus": "ent:cif-0042"`. Other ids are taken as
given. The same data in any order gives the same response.

Four reads of the graph tables stop at 50 000 rows each: the evidence
edges, the declared dependencies, the compliance articles and the gap
nodes. When one of them is longer, the response says `truncated: true`
and the page notes that the map is incomplete. No parameter of the
request brings the missing rows back: these tables hold the whole graph,
not a window of it, so `range`, `env`, `focus` and `kinds` narrow what is
drawn after the read, not what is read. A truncated map is for the
operator: the daemon logs a warning that names the source that reached
its limit (`graph_edges.evidence`, `graph_edges.depends_on` or
`graph_nodes.<kind>`).

The response is cached for 30 seconds per parameter set and set of scopes
and carries an `ETag`; `If-None-Match` with the current value answers
`304`. The cache holds at most 64 maps and 64 MiB, drops expired maps
when it is read, and lets concurrent requests for the same map wait for
one read of the lake instead of each doing their own. That holds also
when the map could not be kept (a write landed while it was read, or it
is larger than the cache): those who waited for it get that map, unless
they asked after the write, in which case they read again.

Send `Cache-Control: no-cache` to read the lake again. Any viewer can send
it, so it is rate-limited rather than trusted: a map that was read less
than 2 seconds ago is answered from the cache all the same. That limit is
per map, and a caller can ask for many different maps, so there is a
second one over all maps and callers: the daemon reads the lake for at
most 4 maps at once, lets at most 64 requests wait for a read, and none
longer than 10 seconds. A request beyond that is answered `503` with
`Retry-After: 2`; the page shows it as any other failed load, and asking
again after the wait is the right response.

Every write that a map shows drops the cache, so the map shows it at
once: suppressing a risk, adding, verifying, rejecting or importing a
manual record, importing a journal and storing a performance verdict. A
map that was being built while such a write landed is answered but not
kept. Telemetry that arrives over OTLP does not drop the cache; it shows
after at most 30 seconds.

`GET /api/topology` remains as a compatibility view of the service and
system call graph. Its ids are the Star Map's (`svc:`,
`sys:<environment>/<system>`; a target known only by
`resilience.target.name` is `sys:-/<name>`), it covers `7d` unless `range`
says otherwise, and the earlier `tgt:` ids are gone.

## What is not there yet

`sources` reports `slo`, `dora` and `maturity` as `unavailable`, and the
matching node fields are `null`. They are reserved for the SLO, DORA and
maturity overlays. The page does not list them. It writes a line under the
map only when a source it draws, the graph or OTel conformance, could not
be loaded.

The roll-up of maturity is a proposal, not a decision: the rule and its
weights are pending the DORA score work and may change with it. As
proposed, a parent's level is the criticality-weighted mean of its
children, capped by the weakest critical child, with a child of unknown
maturity counted at the lowest level so that it lowers the parent rather
than disappearing from it. A unit could then never look more mature than
its most important system, and missing evidence would never be read as
good news. Nothing in the map computes this yet; the `rollup` counts above
are plain sums and carry no weights.

## Proof

`scripts/proof/starmap-step1/proof.sh` starts a fresh daemon on free
loopback ports with a private lake, the demo `org.yaml` and manifests,
runs the demo experiments through the CLI (each must end with its expected
exit code and status) and asserts on the map: the organisation and its
teams, workloads with `runs_on` edges, every definition referenced, the
untested workload as a gap, the failed run on its system, the roll-up to
the organisation, a suppressed finding with its expiry, the cache, its `ETag` and the limit
on forced reads. A free port is not a reserved one, so the proof asks the
kernel (`ss -ltnp`) for every listener on both ports, on any address, and
goes on only when the daemon it started is the one process behind them,
at the start and before every write. `proof.sh self-test` shows that
check at work: it stages a listener that is not the daemon on the HTTP
port (on `127.0.0.1`, on `0.0.0.0` and on `[::]`) and passes only if the
proof refuses each time. The proof removes the daemon and its files on
exit.

`make demo-check` asserts the same minimum on the Krönika demo stack:
`scripts/demo-check.sh` brings `docker/docker-compose.kronika.yml` up as
its own compose project, with its own container names, image tag, ports
and a fresh volume, so a Krönika demo that is already up is not touched.
It runs the seed, compares the journal of each seed experiment with
`demo/kronika/experiments/expectations.json`, calls
`scripts/starmap-check.py --write` (the
organisation, 3 teams, 4 workloads with `runs_on` edges, 8 definitions
referenced, a gap, a failed run, a suppressed risk, no `tgt:` id) and
removes the stack again, also when it is interrupted. The project, its
names and its ports are the same on every run, so one check at a time has
the stack: a second run waits for the first on a lock file
(`${TMPDIR:-/tmp}/tumult-demo-check-kronika.lock`, up to
`STARMAP_LOCK_WAIT` seconds, 900 by default) and touches nothing if it
cannot have its turn. Without `docker` on `PATH` the script says so once
and stops before it runs anything.

`scripts/starmap-check.py` only reads unless it is given `--write`. The
suppressed-risk line needs a suppressed risk; with `--write` the check
suppresses one open risk for 7 days when none is, which cannot be undone
through the API, so that flag is for a daemon that is thrown away.
Without it the line is reported as skipped. `STARMAP_URL=<daemon>
scripts/demo-check.sh --starmap-only` checks a daemon that is already
running and never writes to it; it is the one use of the script that
does not need Docker.
