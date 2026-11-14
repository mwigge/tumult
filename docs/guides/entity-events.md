# Entity events

An OpenTelemetry Collector can report the things it sees (a host, a
Kubernetes pod, a deployment) as *entity events*: log records that carry
the state of an entity, or say that it is gone. With
`KRONIKA_ENTITY_EVENTS=1` tumultd stores these records as entities with a
lifetime instead of as log lines, and answers what exists, what stopped
reporting and what was deleted.

The feature is off by default. The entity event specification is in
Development status and the attribute names have already changed once, so
this guide also says what is stored when the flag is off and how to go
back.

This is the first step: ingest, storage and two read endpoints. The Star
Map, the maturity roll-up and the web UI do not read entities yet.

## 1. Switching it on

| Variable | Default | Meaning |
|---|---|---|
| `KRONIKA_ENTITY_EVENTS` | unset (off) | `1`, `true`, `yes` or `on` (in any case) stores entity events as entities; unset, empty, `0`, `false`, `no` or `off` leaves them off. Any other value also leaves them off and is logged at start as the warning `ingest.entity_events.flag_unrecognised`. Read once, when the daemon starts. |
| `KRONIKA_ENTITY_LABEL_ALLOW` | empty | Comma-separated label names that are kept from `k8s.<kind>.label.<name>` attributes, for example `team,app.kubernetes.io/name`. See section 5. |
| `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` | empty | Comma-separated description keys that are kept whatever they are: a whole key (`k8s.service.annotation.owner`) or the start of one followed by `*` (`k8s.service.selector.*`). An entry that names no namespace (`*` alone, or a start without a dot such as `k8s*`) is honoured and logged at start as the warning `ingest.entity_events.attribute_allow_broad`. See section 5. |
| `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT` | `50000` | The most entities one environment may hold, a positive integer. The lake as a whole holds four times as many. Anything else keeps the default and is logged at start as the warning `ingest.entity_events.limit_unrecognised`. See section 6. |

With the flag on the daemon logs `daemon.entity_events.enabled` at
startup. The flag applies to all three log carriers: OTLP/gRPC, OTLP/HTTP
and OTel-Arrow.

With the flag off nothing changes. An entity event is an ordinary log
record: it is stored in `logs` and shown by `GET /api/logs`, as before the
feature existed. An export that carries no entity event is stored the same
with the flag on as with it off.

Lake schema v21 adds the table and the view described below, and two
nullable columns on `logs` (`event_name` and `scope_attrs`), which hold
the event name and the scope attributes of every log record whatever the
flag says. The migration runs when the daemon opens the lake and is
additive. A v21 lake is not readable by older binaries; take
`tumult store backup` before upgrading.

## 2. Collector configuration

Entity events come from receivers that collect metadata. Give them a logs
pipeline of their own, away from processors that rewrite or drop
attributes.

```yaml
receivers:
  host_metrics:
    collection_interval: 30s
    metadata_collection_interval: 1m
    scrapers:
      memory:
  k8s_cluster:                       # remove on docker-only stacks
    collection_interval: 30s
    metadata_collection_interval: 5m
processors:
  resourcedetection:
    detectors: [system]
    system:
      hostname_sources: [os]
      resource_attributes:
        host.id:
          enabled: true
  batch:
exporters:
  otlp/tumultd:                      # otelarrow/tumultd works the same way
    endpoint: ${env:TUMULTD_OTLP_ENDPOINT:-tumultd.host:14317}
    tls:
      insecure: true
    headers:
      authorization: "Bearer ${env:KRONIKA_INGEST_TOKEN}"
service:
  pipelines:
    logs/entity_events:
      receivers: [host_metrics, k8s_cluster]
      processors: [resourcedetection, batch]
      exporters: [otlp/tumultd]
```

This configuration passes `otelcol-contrib validate` in
`otel/opentelemetry-collector-contrib:0.165.0`, and the proof (section 9)
runs its `host_metrics` part against a daemon. The `k8s_cluster` receiver
needs its usual RBAC and was not run against a cluster.

The `otlp` exporter needs no `compression` setting: the daemon's OTLP/gRPC
services take its default (gzip), `zstd` and `none`. A message is bounded
by what it expands to (4 MiB), whatever its size on the wire. The
`otelarrow` exporter works with its default compression too.

`resourcedetection` matters for the `host` entity: `host_metrics` sends its
event without an id, and the daemon takes the identity from the resource
attribute `host.id`. A `host` event whose resource has no `host.id` is
dropped and counted (section 6).

`metadata_collection_interval` is the report interval. The Collector sends
it with every event, and the daemon uses it to decide when an entity has
gone silent (section 4).

## 3. The two wire forms

The Collector has two ways of writing an entity event. The daemon accepts
both and stores which one it saw in `wire_form`.

| | Specification form (`spec`) | Earlier form (`legacy`) |
|---|---|---|
| Sent when | the Collector runs with `--feature-gates=pkg.experimentalmetricmetadata.useEntityEventsSpecification` | no feature gate (the default in 0.165.0) |
| Marks a state event | `EventName` = `entity.state` | attribute `otel.entity.event.type` = `entity_state` |
| Marks a delete event | `EventName` = `entity.delete` | attribute `otel.entity.event.type` = `entity_delete` |
| Type | `entity.type` | `otel.entity.type` |
| Identifying attributes | `entity.id` (map) | `otel.entity.id` (map) |
| Descriptive attributes | `entity.description` (map) | `otel.entity.attributes` (map) |
| Relationships | `entity.relationships` (array of maps) | none |
| Report interval | `entity.report.interval`, seconds | `otel.entity.interval`, **milliseconds** |
| Delete reason | `entity.delete.reason` | none |

Both forms also carry the scope attribute `otel.entity.event_as_log` =
`true`. A record is treated as an entity event when it has one of the two
event names, the attribute `otel.entity.event.type`, or that scope
attribute.

The stored row is the same for both forms. The legacy interval is
converted to seconds and rounded up. Of the description only the keys of
section 5 are stored. Of each relationship only `relationship.type` (also
read from `type`, which the specification uses in one place),
`entity.type` and `entity.id` of the target are stored; nothing reads
relationships yet and no Collector component sends them.

## 4. What is stored

`entity_versions` holds one row per distinct state of an entity, not one
row per report.

| Column | Content |
|---|---|
| `entity_key` | the type, `\|`, then the identifying attributes as `name=value` pairs sorted by name and joined with `,`; for example `k8s.pod\|k8s.pod.uid=3f0c…`. `%`, `\|`, `=`, `,` and `/` inside a part are percent-encoded |
| `environment` | the environment the sending resource names (`resilience.target.environment`, or `deployment.environment.name`); empty when it names none |
| `entity_type`, `entity_id` | the type and the identifying attributes |
| `description`, `relationships` | JSON, reduced to the keys of section 5 |
| `state_hash` | SHA-256 over description and relationships (the interval is compared separately) |
| `valid_from_ns` | when this state was first reported |
| `last_seen_ns` | when it was last reported, by the sender's clock |
| `last_received_ns` | when the daemon last received a report of it, by the daemon's clock |
| `interval_s` | seconds between reports; `0` means the sender reports once. Taken from the event, or from the row before when the event carries none |
| `deleted_at_ns`, `delete_reason` | set by a delete event |
| `wire_form` | `spec` or `legacy` |
| `observer` | how the event that opened the row came in (section 5); a record, not a permission |
| `schema_url` | the schema URL of the log scope, when sent |

The rules:

- A state event equal to the latest row only moves `last_seen_ns`. A
  thousand heartbeats are one row.
- A state event with a different description, different relationships or a
  different report interval opens a new row. The earlier row stays as
  history. The interval counts because liveness is derived from it: a
  sender that changes it leaves the row with the earlier interval behind,
  and a copy that is not newer than the stored row cannot change it.
- A state event that carries no interval says nothing about it and the
  entity keeps the interval it has (none, `0`, when the event is its
  first). So two reporters of one entity, of which only one sends an
  interval, share one row, and the one without cannot make the entity
  stop expiring. An interval of `0` that is sent is a statement: it opens
  a row and the entity no longer expires.
- A delete event marks the latest row as deleted. It is ignored when it is
  older than the last report, when the entity has no row or when the
  entity is already deleted. Who sent it plays no part (section 5).
- A state event newer than the deletion opens a new row: the entity is
  back.
- Newer means a later event time. An event without a time, or with one
  ahead of the daemon's clock, gets the time its export was received, so
  several events of one entity in one export can carry the same time.
  Among those the order in the export decides and the later event is the
  newer one; a row opened that way starts one nanosecond after the row
  before it. An event with the same time as a row that an earlier export
  stored is not newer: it cannot be told from a delayed copy.
- The same entity key in two environments is two entities.

The view `entities_current` is the latest row of each entity per
environment with a `liveness` column:

| `liveness` | Meaning |
|---|---|
| `live` | a report was received within three report intervals, or it reports only once (`interval_s = 0`) |
| `expired` | not deleted, but no report received for more than three report intervals |
| `gone` | a delete event was accepted |

Liveness is computed when the view is read, so an entity becomes `expired`
without any write. The silence is counted from `last_received_ns`, the
daemon's own time of receipt, and not from the event time: a Collector
whose clock runs behind, or one that delivers from a retry queue, does not
make a reporting entity look expired. Every repeated state moves
`last_received_ns`, also a delayed copy that is too old to move
`last_seen_ns`. An entity that is gone or expired is kept for 30 days
after the daemon last heard of it and is then removed with all its rows,
or earlier when its place is needed for a new entity (section 6).

An entity event that is stored as an entity is not written to `logs`. One
that is dropped (section 6) is not written to `logs` either.

## 5. Trust

Nothing in an entity event is verified. The daemon has exactly one ingest
token (`KRONIKA_INGEST_TOKEN`): there is no token per sender and no token
id, so the daemon cannot tell one sender from another.

- **Any holder of the ingest token can assert, change, suppress or delete
  any entity in any environment it names.** When no token is set, any
  local process that can reach the ingest port can do the same.
- **`environment` is chosen by the sender.** It is a resource attribute of
  the export, not something the daemon derives from the token or the
  connection. The environment scopes of the read API confine who may read
  an entity; they do not confine who may write one.
- **Liveness is a claim.** The sender chooses the event times, the report
  interval and whether a delete is sent. It can keep an entity `live` that
  does not exist, let a running one show as `expired`, or mark it `gone`.
- **Ownership and tier are claims.** `ict.entity.owner`, `ict.entity.tier`,
  a criticality and every other description value are stored and returned
  as sent. `org.yaml` and the register stay the source of truth, and
  nothing in this release overrides them from entity data.
- **Whatever reads entities next must not treat any of this as verified.**
  That holds for the graph, the Star Map and the maturity roll-up of the
  next phase: an entity, its environment, its liveness, its owner and its
  tier are what a token holder said.

Every row records its `observer`: `token:default` when the event that
opened the row arrived with the ingest token and `anonymous` when ingest
runs without one. The token itself, or a hash of it, is never stored. The
observer is a record and decides nothing. In particular a delete is
accepted whatever the observer of the row is: with one token a rule that
compared observers would refuse no sender, and it would refuse every
delete sent after a token was introduced or removed. A deleted entity is
shown as `gone` rather than removed, until it is evicted to make room or its
retention ends (section 6).

### What of a description is stored

A description is written by the sender, and labels, selectors and
annotations in particular carry internal data: an annotation can hold a
whole manifest with the values of its environment variables. The daemon
therefore stores a description key only when it is listed, and removes
every other key before the row is written. The keys stored by default:

| Key | What it is |
|---|---|
| `host.*`, `os.*`, `service.*`, `deployment.*`, `cloud.*` | the semantic-convention attributes of these namespaces |
| `ict.*` | the function register attributes (`ict.entity.tier`, `ict.entity.owner`, ...) |
| `container.image.*` | image name, tag and digests |
| `container.id`, `container.name`, `container.runtime`, `container.status`, `container.status.reason`, `k8s.container.name` | the container itself |
| `k8s.workload.kind`, `k8s.workload.name` | the workload an object belongs to |
| `k8s.<kind>.name`, `k8s.<kind>.uid` | the name and uid of the object and of its owner, namespace and node |
| `k8s.<kind>.start_time`, `k8s.<kind>.creation_timestamp`, `<kind>.creation_timestamp` | when it started or was created |
| `k8s.<kind>.phase`, `k8s.<kind>.status_reason`, `k8s.node.condition_*` | its status |
| `schedule`, `concurrency_policy`, `current_revision`, `update_revision` | what `k8s_cluster` writes without a prefix for a cron job and a stateful set |

`<kind>` is one name without a dot. On top of the table:

- A key with a segment that starts with `label`, `annotation` or
  `selector`, in any case, is never stored by default, whatever namespace
  it stands in (`host.label.rack`, `service.Labels.team`,
  `host.labels_json`, `host.annotation/owner`). The rule is wide on
  purpose: a harmless key such as `service.label_count` is removed too and
  has to be named in `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` to be kept.
- A key stored by default keeps a scalar or a list of scalars. A map, or a
  list that holds one, is removed with its key, so nothing nested arrives
  under a listed name.
- `KRONIKA_ENTITY_LABEL_ALLOW` names labels: `team` keeps
  `k8s.<kind>.label.team` for every kind. It does not keep an annotation
  or a selector of that name.
- `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` names keys: a whole key, or the start of
  one followed by `*`. A key named here is stored with whatever value it
  has, nested or not, within the size limits of section 6. `*` alone
  stores every key; do not set it on a daemon that receives events from
  clusters you do not control.
- Relationships are reduced to their type and the type and identity of
  their target; the two variables do not widen that.

Removed keys are counted in `tumultd_entity_event_keys_filtered_total` on
`GET /metrics`. When a label you expect is missing from an entity, that
counter moving is the sign that it was removed here.

The table was written from the source of the `k8s_cluster` receiver in
Collector contrib v0.165.0 (`receiver/k8sclusterreceiver/internal`), not
from a running cluster. That source sends, per object, a flat map of
strings: the keys above, `k8s.<kind>.label.<name>` for every label,
`k8s.service.selector.<name>`, `k8s.<kind>.annotation.<name>` for
services, persistent volumes and their claims, and one empty-valued
`k8s.service.<service name>` key on a pod for each service that selects
it. Those last four groups are removed unless named. A receiver that sends
other keys loses them the same way: unlisted is not stored.

## 6. Limits and dropped events

An entity event is dropped, never stored as a log, when it cannot become a
row. Each drop is counted in `tumultd_entity_events_dropped_total` by
reason, and logged as `otlp.entity_event.dropped` at most once a minute
with the number left out.

| `reason` | Dropped when |
|---|---|
| `unknown_kind` | the record is marked as an entity event but is neither a state nor a delete; logged as `otlp.entity_event.unknown_kind` with the name of the event, at most once a minute, so a sender that uses a kind this build does not know can be found |
| `no_type` | the type attribute of its wire form is missing or empty |
| `no_identity` | it has no identifying attributes and the resource has none for its type (only `host` → `host.id` is known) |
| `malformed` | a control character (NUL included) or a Unicode format character (a bidirectional override, a zero-width space and the rest of category `Cf`) in the type, in an identifying name or value, in the type of a relationship, in the environment or in the schema URL, a type that is not a string, an interval that is not a number, an identifying value that is empty, the same identifying name twice, a description that is not a map, relationships that are not an array of maps, a relationship without a type or without the type and identity of its target, or nesting deeper than 8 |
| `non_string_id` | an identifying value, of the entity or of the target of a relationship, is not a string. A number or a boolean would name the same entity as the string that reads alike (`1` and `"1"`), so only strings identify; a sender that writes `process.pid` as a number shows up under this reason and has to send it as text |
| `export_limit` | the export already gave 10 000 entity events; the first 10 000 are stored and the rest dropped |
| `oversized` | a type over 128 bytes, more than 16 identifying attributes, a name or value over 256 bytes, an environment over 256 bytes, a schema URL over 2048 bytes, more than 64 relationships, more than 4096 values in description and relationships (every entry of a map or a list counts, a removed key too), or description and relationships over 64 KiB together |

One export stores at most 10 000 entity events. The daemon has a single
writer for all telemetry and folds the entity events of an export in one
transaction; the limit keeps that transaction short. Events beyond it are
dropped with reason `export_limit` and the export is still accepted, so
its other records are stored and the Collector does not retry it. A
Collector that could send more in one export needs `send_batch_max_size`
at or below 10 000 on its `batch` processor (by default it batches
8 192 records and sets no maximum); a dropped state event is made up for by the next periodic report
of that entity, a dropped delete event is not.

A delete event is read for its type, its identifying attributes and its
reason only. A description or an interval in it is ignored, so a delete is
never dropped for what it says about a state.

The report interval is kept between 0 and 30 days. An event time in the
future is replaced by the time of receipt. A delete reason loses its
control and format characters and is cut at 256 bytes; the delete is
applied whatever the reason held.

`tumultd_entity_events_total` counts what the store did with the events it
accepted, by `outcome`: `opened`, `refreshed`, `tombstoned`,
`stale_state`, `delete_unknown`, `delete_repeated`,
`delete_out_of_order` and `entity_limit`. Both counters are on
`GET /metrics`.

### How far the table can grow

A sender chooses the type, the id and the environment of an entity and can
change its description as often as it likes, so `entity_versions` is
bounded on every side:

| Bound | Value | What happens beyond it |
|---|---|---|
| Rows per entity and environment | 100 | When a new row takes an entity past the bound its oldest rows are removed. The latest row always stays. Counted in `tumultd_entity_versions_pruned_total{reason="version_cap"}`. |
| Entities per environment | 50 000, or `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT` | A state event for an entity that has no row first makes room (below). Only when nothing can make room is it refused: outcome `entity_limit`, and the warning `ingest.entity_events.limit_reached` at a bounded rate. The rest of the export, its log records included, is stored. Events for entities that have a row are applied as usual. |
| Entities in the lake | four times the bound per environment | The same, with room made in any environment. The environment is named by the sender, so a bound per environment alone would bound nothing. |
| Retention of entities that are not live | 30 days since the daemon last received anything for them | The entity is removed with all its rows. Counted in `tumultd_entity_versions_pruned_total{reason="retention"}`. |

**Making room.** At either entity bound the daemon evicts before it
refuses. Entities are removed, with all their rows, in this order:

1. entities that are `gone` or `expired`, the one the daemon heard of
   longest ago first;
2. entities that send no periodic reports (`interval_s = 0`) and of which
   the daemon received nothing for 24 hours. Such an entity never expires
   and is `live` for the API however old it is; for the bounds it ages
   out after a day, so that an environment cannot be filled for good with
   entities that were reported once.

One eviction removes up to a hundredth of the bound per environment (500
with the default, never more than 5 000), so that the next new entities
find room without another search. An entity that the same export names is
not evicted for that export. Evictions are counted in
`tumultd_entity_evictions_total` by `reason` (`gone`, `expired`,
`unreported`) and logged as `ingest.entity_events.evicted` at a bounded
rate. An entity that is live and reports is never evicted: when a bound
is full of those, new entities are refused until an operator purges
(below) or raises the bound. When a search finds nothing to evict the
daemon does not search that bound again for a minute, or until a delete
event is applied.

What this means for a sender that misbehaves, since every holder of the
ingest token can be one (section 5):

- **It names the environment.** It can fill any environment, also one it
  does not otherwise report for, and the lake bound is four environments'
  worth. Entities it keeps live by reporting them cannot be evicted, so it
  can hold a bound against newcomers for as long as it keeps sending.
  The daemon cannot tell its events from honest ones; the remedy is the
  operator's: rotate the token, then purge.
- **It can push out history.** New entities at a full bound evict the
  entities that are gone or expired, with their history, long before the
  30 days of retention.
- **It can roll out the history of one entity.** Every change of state
  opens a row and an entity keeps 100. An entity that flaps between two
  states at each report loses its oldest row with the 101st change: at a
  report interval of one minute that is the history of the last hour and
  forty minutes. The current state is always kept.
- **Worst-case size.** The table holds at most 200 000 entities with 100
  rows each, 20 million rows. A row carries at most 64 KiB of description
  and relationships and a few KiB of identity, so the bound in bytes,
  before the lake's compression, is about 1.3 TiB. Rows as a Collector
  writes them are under 1 KiB, which puts 20 million of them near 20 GiB,
  and identical reports share a row, so a fleet that is left alone stays
  at one or a few rows per entity. Where the worst case matters, lower
  `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT`: every figure here scales with it.

An entity that is gone or expired holds its place under the two entity
bounds until it is evicted or its retention ends. Removal for retention
runs on the writer when entity events arrive, at most every ten minutes
and at most 5 000 entities at a time, so a daemon that receives no entity
events removes nothing. A state event for an entity that was removed
opens it again as a new entity, without its earlier history.

`tumultd_entity_versions` is the number of rows in the table after the
last export that carried entity events.

### Purging an environment

`DELETE /api/entities?environment=<name>[&type=<type>]` removes the
entities of one environment with all their rows: all of them, or those of
one type. It is the remedy when a bound is held by entities that should
not be there (a sender that named the wrong environment, a token that
leaked and was rotated).

```bash
curl -X DELETE -H "Authorization: Bearer $KRO_TOKEN" \
  "http://127.0.0.1:24318/api/entities?environment=staging&type=k8s.pod"
# {"environment":"staging","type":"k8s.pod","entities":1204,"versions":3611}
```

- It needs the Operator role. A principal with environment scopes purges
  its own environments only (`403` for any other).
- `environment` must be given (`400` without it). Given empty,
  `?environment=`, it names the entities whose sender named no
  environment; those are outside every scope, so only an unscoped
  principal removes them.
- It removes live entities too, and history with them. An entity that
  still reports is stored again with its next report, as a new entity:
  purging does not silence a sender, rotating the ingest token does.
- It runs on the daemon's single writer, in one transaction: either all
  matching rows are removed or none. Nothing else in the lake is touched;
  entity events that were stored as log records while the flag was off
  stay in `logs`.
- Every purge is written to the daemon's log as
  `ingest.entity_events.purged`, with `user.name`,
  `deployment.environment.name`, `entity.type` (`*` for all types),
  `entity.purged.entities` and `entity.purged.versions`, also when it
  found nothing. With authentication off (no users) the name is
  `anonymous`.

### What an export costs the writer

The daemon has one writer for all telemetry, so the time it spends on
entity events is time other exports wait. The entity events of an export
are folded in one transaction with a number of statements that does not
grow with their count: the latest rows of the entities named are read at
once, new rows are inserted at once, changed rows are updated at once and
the row bound is applied at once. 5 000 events in one export are at most
40 statements whatever they do to the rows (the test
`a_batch_of_five_thousand_events_is_a_handful_of_statements` holds each
case to that) and take about a quarter of a second in an unoptimised
build; the same export took 13 s when every event ran its own statements.

The writer counts the entities per environment in memory: once, from the
table, when the first export needs it, and from then on by what it adds
and removes itself. The rows of the entities an export names are read by
key and environment. Both matter when the table is as full as a sender
can make it. Measured with `cargo run -p tumult-lake --example
entity_writer_cost` (unoptimised build, a development machine, the
median of 200 exports of one event each, every export a transaction of
its own; before and after the two changes):

| One-event export | Before | After |
|---|---|---|
| heartbeat of a known entity, 50 000 entities in its environment | 37 ms | 38 ms |
| unknown id, the environment at its bound with live entities (refused) | 44 ms | 27 ms |
| the same with 150 000 entities in the lake | 143 ms | 33 ms |
| unknown id, an environment with room (opened), 50 000 entities in the lake | 66 ms | 48 ms |
| heartbeat of an entity whose key is in 100 000 environments | 374 ms | 49 ms |
| unknown id, the environment at its bound with expired entities (evicts, then opened) | refused | 52 ms; 124 ms for the one export in 500 that evicts 500 entities |

About 34 ms of every figure is what a transaction costs on that machine
whatever it holds: a heartbeat in a table of 200 entities takes as long.

What still costs a pass over the table, since it has no index:

- **Opening a row** applies the row bound to the entities that gained
  one, which reads the table once per export: the difference between a
  heartbeat (38 ms) and an export that opens a row (48 ms) above.
- **Eviction** reads the latest row of every entity of the environment
  (of the lake, at the lake bound) to find the ones to remove, and then
  removes them: 124 ms for 500 entities out of 50 000 above. A sender
  that keeps a bound full of entities it lets expire can cause one such
  search per 500 new entities, not one per export.
- **The search that finds nothing** costs the same read without the
  delete and is not repeated for a minute per bound.
- **Retention**, every ten minutes: 10 ms when nothing is due in a table
  of 200 000 rows, 84 ms when 5 000 entities are removed.
- **A purge** of an environment with 50 000 entities took 87 ms.

`entity_versions` has no primary key and no index. Heartbeats and deletes
update rows in place, and the lake keeps indexes off every table that
receives updates: an index that falls out of step with its table after a
restart makes each later update of that table fail. The statements above
join on `(entity_key, environment, valid_from_ns)` by hashing, which the
figures above include.

## 7. Reading entities

Both read endpoints need the Viewer role. A user with environment scopes
sees only the entities of those environments; an entity that names no
environment is visible only to users without scopes. (Removing entities
is `DELETE /api/entities`, section 6.)

`GET /api/entities?type=&liveness=&range=&sort=&limit=&cursor=` lists the
current row of every entity, ordered by key and then environment.

| Parameter | Meaning |
|---|---|
| `type` | exact entity type, for example `k8s.pod` |
| `liveness` | `live`, `expired` or `gone`; anything else is `400` |
| `range` | `24h`, `7d`, `14d` or `30d`: only entities the daemon last received a report or a delete for inside it (`last_received_ns`, the daemon's clock, as for liveness; not the time the sender wrote). Left out, every entity is listed |
| `sort` | `key` (the default) or `recent`: last received first. Anything else is `400` |
| `limit` | rows returned, 1 to 1000, default 100 |
| `cursor` | the `next_cursor` of the page before, with the same `sort`; anything else is `400` |

The answer is one page. `total` is the number of entities the filters
match, `count` the number in this page and `truncated` whether the page
left any out. A page ends at `limit` rows or once its rows weigh about
4 MiB, whichever comes first (a row can carry 64 KiB of description and
relationships, so a page of large rows is shorter than `limit`), and it
always holds one row at least. The store cuts the page: the daemon does
not read `limit` rows to return a few. When `truncated` is true,
`next_cursor` is the text to send as `cursor` with the same filters and
`sort` to get the rows that follow; otherwise it is `null`.

The cursor is the place of the last row in the listing, not a snapshot,
and `total` can differ from page to page. What a walk through the pages
promises depends on the order:

- **By key (the default).** Nothing an entity reports moves it in this
  order. Every entity that matches the filters from the first page to the
  last is returned exactly once. An entity that appears, is removed, or
  starts or stops matching a filter during the walk (`liveness` changes
  with time) may or may not be returned.
- **`sort=recent`.** The order is by `last_received_ns`, so an entity that
  reports during the walk moves to the front and is not returned by a
  walk that has passed its old place: use it to look at the first page,
  not to collect every entity.

```json
{
  "count": 1,
  "total": 1,
  "truncated": false,
  "next_cursor": null,
  "entities": [
    {
      "entity_key": "host|host.id=4c4c4544004d",
      "environment": "",
      "entity_type": "host",
      "entity_id": {"host.id": "4c4c4544004d"},
      "description": {},
      "relationships": [],
      "valid_from_ns": 1794650000000000000,
      "last_seen_ns": 1794650060000000000,
      "last_received_ns": 1794650060120000000,
      "interval_s": 60,
      "deleted_at_ns": null,
      "delete_reason": null,
      "wire_form": "legacy",
      "observer": "token:default",
      "schema_url": "https://opentelemetry.io/schemas/1.43.0",
      "liveness": "live"
    }
  ]
}
```

This is the `host` entity as `host_metrics` reports it: the event carries
no descriptive attributes, so `description` is empty; the host name stays
on the resource and is not copied into the entity.

`GET /api/entities/{key}/history?environment=&limit=&cursor=` lists the
stored rows of one entity, newest first, with the same fields except
`liveness`. A stored row keeps its place in that order, so a walk returns
every row that exists throughout it once. The same key can exist in several
environments, each with a history of its own: every row names its
`environment`, `environments` lists the environments the caller may read
with the number of rows in each, and `environment=` keeps the rows of one.
The answer is paged like the list, with `total`, `count`, `truncated` and
`next_cursor` of its own. A key with no row in the caller's environments
is `404`.

```json
{
  "entity_key": "host|host.id=4c4c4544004d",
  "count": 2,
  "total": 2,
  "truncated": false,
  "next_cursor": null,
  "environments": [{"environment": "", "versions": 2}],
  "versions": [{"environment": "", "valid_from_ns": 1794650000000000000}]
}
```

(Each element of `versions` has all the fields; two are shown.)

The key goes into the path as one segment, and it is not safe to put there
as it is. Inside a type or an id the key writes `%`, `|`, `=`, `,` and `/`
as `%25`, `%7C`, `%3D`, `%2C` and `%2F`; every other character is kept as
the sender sent it, so a key can hold a space, `?`, `#` or a quote. A
client must percent-encode the whole key, its `%` signs included, the way
`encodeURIComponent` does:

| Key as the list returns it | In the path |
|---|---|
| `host\|host.id=4c4c4544004d` | `/api/entities/host%7Chost.id%3D4c4c4544004d/history` |
| `service\|service.name=a%2Fb` (the name is `a/b`) | `/api/entities/service%7Cservice.name%3Da%252Fb/history` |

A key sent without that encoding names another path, or another entity.

## 8. Switching it off again

Unset `KRONIKA_ENTITY_EVENTS` and restart the daemon. Entity events are
stored as log records again from then on. The rows already in
`entity_versions` stay and are still returned by the two endpoints; their
`liveness` turns `expired` as their intervals pass.

### Rolling back to an older build

Entity events arrive with lake schema v21. A build from before v21 does
not know the table or the two new `logs` columns, and a v21 lake is not
readable by it. To be able to go back, stop the daemon and run
`tumult store backup --output <dir>` before the upgrade, and restore that
backup with the older build. From v21 on a build that finds a later
schema version in the lake than its own refuses to open it for writing
and says `lake schema vN is newer than this build supports (vM)`; nothing
in the lake is changed by the attempt.

## 9. Proof

`scripts/proof/entity-events/proof.sh` starts a Collector container
(contrib 0.165.0, `host_metrics` and `resourcedetection`, a ten-second
report interval) against a fresh daemon in four phases: flag off (the
records are in `logs`, no entity row), the legacy form and the
specification form over OTLP/gRPC (one `host` entity, `live`, the right
`wire_form`, exactly one row after several heartbeats, nothing in `logs`,
then `expired` three intervals after the Collector is stopped), and the
specification form over OTel-Arrow. The Collector's `otlp` exporter runs
with its default compression (gzip).

A fifth phase has no Collector. One quiet host never changes, deletes or
labels anything, so the example binary
`tumult-ingest/examples/entity_probe.rs` sends those events itself, one
gzip-compressed OTLP/gRPC export each, for a `k8s.pod` entity, to a daemon
started with `KRONIKA_ENTITY_LABEL_ALLOW=app`:

| Sent | Checked on the API and `/metrics` |
|---|---|
| a state, phase `Pending` | one live entity, one row |
| a state, phase `Running` | a second row; the first still says `Pending` |
| a delete | the entity is `gone` with its reason, still two rows |
| the state again | the entity is `live` in a third row; the deleted row keeps its mark |
| a state with two labels, an annotation holding a manifest, a container label and a nested map of labels | the description holds the name, the phase and `k8s.pod.label.app` only; `tumultd_entity_event_keys_filtered_total` is 4; the secret in the manifest is in no API answer and not in the lake file |
| a state whose description is one byte over 64 KiB | `tumultd_entity_events_dropped_total{reason="oversized"}` is 1, no entity, no log record |

Build the probe with
`cargo build -p tumult-ingest --example entity_probe` before running the
proof.
