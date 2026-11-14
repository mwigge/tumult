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
| `KRONIKA_ENTITY_EVENTS` | unset (off) | `1` or `true` stores entity events as entities. Any other value is off. Read once, when the daemon starts. |
| `KRONIKA_ENTITY_LABEL_ALLOW` | empty | Comma-separated label names that are kept from `k8s.<kind>.label.<name>` attributes, for example `team,app.kubernetes.io/name`. Every other label is removed before the entity is stored. |

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
    compression: none                # the daemon refuses gzip on OTLP/gRPC
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

`compression: none` is needed on the `otlp` exporter. The daemon's
OTLP/gRPC services take uncompressed messages only, and the exporter
default (gzip) is refused with `UNIMPLEMENTED`, which the Collector treats
as a permanent error and drops the data. The `otelarrow` exporter works
with its default compression.

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
converted to seconds and rounded up. Maps and arrays are stored as JSON
with their structure intact; `relationships` is kept as sent and nothing
reads it yet.

## 4. What is stored

`entity_versions` holds one row per distinct state of an entity, not one
row per report.

| Column | Content |
|---|---|
| `entity_key` | the type, `\|`, then the identifying attributes as `name=value` pairs sorted by name and joined with `,`; for example `k8s.pod\|k8s.pod.uid=3f0c…`. `%`, `\|`, `=`, `,` and `/` inside a part are percent-encoded |
| `environment` | the environment the sending resource names (`resilience.target.environment`, or `deployment.environment.name`); empty when it names none |
| `entity_type`, `entity_id` | the type and the identifying attributes |
| `description`, `relationships` | JSON, after the label allow-list |
| `state_hash` | SHA-256 over description and relationships |
| `valid_from_ns` | when this state was first reported |
| `last_seen_ns` | when it was last reported |
| `interval_s` | seconds between reports; `0` means the sender reports once |
| `deleted_at_ns`, `delete_reason` | set by a delete event |
| `wire_form` | `spec` or `legacy` |
| `observer` | who reported it (section 5) |
| `schema_url` | the schema URL of the log scope, when sent |

The rules:

- A state event equal to the latest row only moves `last_seen_ns`. A
  thousand heartbeats are one row.
- A state event with a different description or different relationships
  opens a new row. The earlier row stays as history.
- A delete event marks the latest row as deleted. It is ignored when it is
  older than the last report, when the entity has no row, when the entity
  is already deleted, or when it comes from another observer than the one
  that reported the entity.
- A state event newer than the deletion opens a new row: the entity is
  back.
- The same entity key in two environments is two entities.

The view `entities_current` is the latest row of each entity per
environment with a `liveness` column:

| `liveness` | Meaning |
|---|---|
| `live` | reported within three report intervals, or reports only once (`interval_s = 0`) |
| `expired` | not deleted, but silent for more than three report intervals |
| `gone` | a delete event was accepted |

Liveness is computed when the view is read, so an entity becomes `expired`
without any write. Deleted rows are kept; nothing removes them.

An entity event that is stored as an entity is not written to `logs`. One
that is dropped (section 6) is not written to `logs` either.

## 5. Trust

Anything in an entity event is a statement by whoever holds the ingest
token. A sender can report an entity it does not run, and it can put any
value in the description, including `ict.entity.owner`, `ict.entity.tier`
or a criticality.

For that reason:

- Ownership and tier carried by an entity event are **claims**. They are
  stored and returned as sent. `org.yaml` and the register stay the source
  of truth, and nothing in this release overrides them from entity data.
- Every row records its `observer`. The daemon has one ingest token today,
  so the value is `token:default` when a token is configured and
  `anonymous` when ingest runs without one. The token itself, or a hash of
  it, is never stored.
- A delete is accepted only from the observer that reported the entity,
  and a deleted entity is shown as `gone` rather than removed. With a
  single ingest token every sender is the same observer, so this rule only
  separates senders once the daemon has more than one token.
- Kubernetes labels can carry internal data. No `k8s.<kind>.label.*`
  attribute is stored unless its label name is in
  `KRONIKA_ENTITY_LABEL_ALLOW`.

## 6. Limits and dropped events

An entity event is dropped, never stored as a log, when it cannot become a
row. Each drop is counted in `tumultd_entity_events_dropped_total` by
reason, and logged as `otlp.entity_event.dropped` at most once a minute
with the number left out.

| `reason` | Dropped when |
|---|---|
| `unknown_kind` | the record is marked as an entity event but is neither a state nor a delete |
| `no_type` | the type attribute of its wire form is missing or empty |
| `no_identity` | it has no identifying attributes and the resource has none for its type (only `host` → `host.id` is known) |
| `malformed` | a type that is not a string, an interval that is not a number, an identifying value that is empty or not a scalar, the same identifying name twice, a description that is not a map, relationships that are not an array of maps, or nesting deeper than 8 |
| `oversized` | a type over 128 bytes, more than 16 identifying attributes, a name or value over 256 bytes, or description and relationships over 64 KiB together |

The report interval is kept between 0 and 30 days. An event time in the
future is replaced by the time of receipt. A delete reason is cut at 256
bytes.

`tumultd_entity_events_total` counts what the store did with the events it
accepted, by `outcome`: `opened`, `refreshed`, `tombstoned`,
`stale_state`, `delete_unknown`, `delete_repeated`, `delete_out_of_order`
and `delete_foreign_observer`. A refused delete from another observer is
also logged as `ingest.entity_event.delete_refused`. Both counters are on
`GET /metrics`.

## 7. Reading entities

Both endpoints need the Viewer role. A user with environment scopes sees
only the entities of those environments; an entity that names no
environment is visible only to users without scopes.

`GET /api/entities?type=&liveness=&range=&limit=` lists the current row of
every entity, most recently heard of first.

| Parameter | Meaning |
|---|---|
| `type` | exact entity type, for example `k8s.pod` |
| `liveness` | `live`, `expired` or `gone`; anything else is `400` |
| `range` | `24h`, `7d`, `14d` or `30d`: only entities last reported or deleted inside it. Left out, every entity is listed |
| `limit` | rows returned, 1 to 1000, default 200 |

```json
{
  "count": 1,
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

`GET /api/entities/{key}/history?limit=` lists every stored row of one
entity, newest first, with the same fields except `liveness`. The key is
one path segment, so `|` is written `%7C`:
`/api/entities/host%7Chost.id=4c4c4544004d/history`. A key with no row in
the caller's environments is `404`.

## 8. Switching it off again

Unset `KRONIKA_ENTITY_EVENTS` and restart the daemon. Entity events are
stored as log records again from then on. The rows already in
`entity_versions` stay and are still returned by the two endpoints; their
`liveness` turns `expired` as their intervals pass.

## 9. Proof

`scripts/proof/entity-events/proof.sh` starts a Collector container
(contrib 0.165.0, `host_metrics` and `resourcedetection`, a ten-second
report interval) against a fresh daemon in four phases: flag off (the
records are in `logs`, no entity row), the legacy form and the
specification form over OTLP/gRPC (one `host` entity, `live`, the right
`wire_form`, exactly one row after several heartbeats, nothing in `logs`,
then `expired` three intervals after the Collector is stopped), and the
specification form over OTel-Arrow.
