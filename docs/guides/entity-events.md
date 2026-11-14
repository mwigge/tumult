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
| `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` | empty | Comma-separated description keys that are kept whatever they are: a whole key (`k8s.service.annotation.owner`) or the start of one followed by `*` (`k8s.service.selector.*`). See section 5. |
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
| `interval_s` | seconds between reports; `0` means the sender reports once |
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
after the daemon last heard of it and is then removed with all its rows
(section 6).

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
shown as `gone` rather than removed.

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

- A key with a segment `label`, `labels`, `annotation`, `annotations`,
  `selector` or `selectors` is never stored by default, whatever it starts
  with (`host.label.rack`, `service.annotations.x`).
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
| `malformed` | a control character (NUL included) in the type, in an identifying name or value, in the environment or in the schema URL, a type that is not a string, an interval that is not a number, an identifying value that is empty or not a string (a number or a boolean would name the same entity as the string that reads alike), the same identifying name twice, a description that is not a map, relationships that are not an array of maps, a relationship without a type or without the type and identity of its target, or nesting deeper than 8 |
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
future is replaced by the time of receipt. A delete reason is cut at 256
bytes.

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
| Entities per environment | 50 000, or `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT` | A state event for an entity that has no row is refused: outcome `entity_limit`, and the warning `ingest.entity_events.limit_reached` at a bounded rate. The rest of the export, its log records included, is stored. Events for entities that have a row are applied as usual. |
| Entities in the lake | four times the bound per environment | The same. The environment is named by the sender, so a bound per environment alone would bound nothing. |
| Retention of entities that are not live | 30 days since the daemon last received anything for them | The entity is removed with all its rows. Counted in `tumultd_entity_versions_pruned_total{reason="retention"}`. |

An entity that is gone or expired holds its place under the two entity
bounds until it is removed. A live entity is never removed, however old:
one that reports once and never again (`interval_s = 0`) stays until a
delete event arrives. Removal runs on the writer when entity events
arrive, at most every ten minutes and at most 5 000 entities at a time, so
a daemon that receives no entity events removes nothing. A state event for
an entity that was removed opens it again as a new entity, without its
earlier history.

`tumultd_entity_versions` is the number of rows in the table after the
last export that carried entity events.

### What an export costs the writer

The daemon has one writer for all telemetry, so the time it spends on
entity events is time other exports wait. The entity events of an export
are folded in one transaction with a fixed number of statements whatever
their count: the latest rows of the entities named are read at once, new
rows are inserted at once, changed rows are updated at once and the row
bound is applied at once. Measured on a development machine with an
unoptimised build, 5 000 events in one export take 0.25 s when each opens
a row and 0.15 s when each is a heartbeat or a delete (the test
`a_batch_of_five_thousand_events_is_folded_well_within_the_second` holds
each case under one second); the same export took 13 s when every event
ran its own statements.

`entity_versions` has no primary key and no index. Heartbeats and deletes
update rows in place, and the lake keeps indexes off every table that
receives updates: an index that falls out of step with its table after a
restart makes each later update of that table fail. The statements above
join on `(entity_key, environment, valid_from_ns)` by hashing, which the
figures above include.

## 7. Reading entities

Both endpoints need the Viewer role. A user with environment scopes sees
only the entities of those environments; an entity that names no
environment is visible only to users without scopes.

`GET /api/entities?type=&liveness=&range=&limit=&cursor=` lists the current
row of every entity, most recently heard of first.

| Parameter | Meaning |
|---|---|
| `type` | exact entity type, for example `k8s.pod` |
| `liveness` | `live`, `expired` or `gone`; anything else is `400` |
| `range` | `24h`, `7d`, `14d` or `30d`: only entities last reported or deleted inside it. Left out, every entity is listed |
| `limit` | rows returned, 1 to 1000, default 100 |
| `cursor` | the `next_cursor` of the page before; anything else is `400` |

The answer is one page. `total` is the number of entities the filters
match, `count` the number in this page and `truncated` whether the page
left any out. A page ends at `limit` rows or once its rows weigh about
4 MiB, whichever comes first (a row can carry 128 KiB of description and
relationships, so a page of large rows is shorter than `limit`), and it
always holds one row at least. When `truncated` is true, `next_cursor` is
the text to send as `cursor` with the same filters to get the rows that
follow; otherwise it is `null`. The cursor is the place of the last row
in the listing (its `last_activity_ns`, key and environment), not a
snapshot: an entity that reports while a client walks the pages moves to
the front and is not seen again by a walk that has passed it, and `total`
can differ from page to page.

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
      "liveness": "live",
      "last_activity_ns": 1794650060000000000
    }
  ]
}
```

This is the `host` entity as `host_metrics` reports it: the event carries
no descriptive attributes, so `description` is empty; the host name stays
on the resource and is not copied into the entity.

`GET /api/entities/{key}/history?environment=&limit=&cursor=` lists the
stored rows of one entity, newest first, with the same fields except
`liveness` and `last_activity_ns`. The same key can exist in several
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
