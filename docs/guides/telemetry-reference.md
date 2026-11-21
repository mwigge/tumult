---
title: Telemetry reference
parent: Send telemetry
grand_parent: Guides
nav_order: 6
summary: "Spans, attributes, metrics and the ingest limits"
---

# Telemetry reference

The spans, attributes, events and metrics a run emits, and the limits of
the daemon's ingest. For setup, see [Observability setup](observability-setup.md).
The meaning of each `resilience.*` attribute is in the
[metadata standard](../resilience-metadata-standard.md).

## Spans

A run has `resilience.experiment` as its root span. Only the steps that
run emit spans.

```text
resilience.experiment                 root, from the runner
├── resilience.hypothesis.before
│   └── resilience.probe              one per probe
│       └── script.execute            script plugins
├── resilience.action                 one per method step
│   ├── script.execute
│   ├── ssh.connect, ssh.execute      SSH targets
│   └── k8s.pod.delete, k8s.node.drain,
│       k8s.deployment.scale, k8s.networkpolicy.apply
├── resilience.hypothesis.after
│   └── resilience.probe
├── resilience.rollback               one over all rollback steps
│   └── resilience.action
└── resilience.analytics.ingest       the journal into the lake
    ├── resilience.analytics.query
    └── resilience.analytics.export
```

A script plugin gets `TRACEPARENT` and `TRACESTATE`, so the spans of the
script become children of `script.execute`. With telemetry off, there is
no span and no `TRACEPARENT`.

Over MCP, `tumult_run_experiment` makes the run a child of the server's
`mcp.tool.call` span. The server reads no `traceparent` from the caller,
so the trace starts at the tool call.

## Attributes

| Span | Attribute | Meaning |
|---|---|---|
| `resilience.experiment` | `resilience.experiment.id` | Id of the run |
| | `resilience.experiment.title` | Title of the experiment |
| | `resilience.test.kind` | Kind of test, such as `chaos` |
| | `resilience.outcome.status` | `completed`, `deviated`, `failed`, `aborted`, `interrupted` or `halted` |
| | `resilience.outcome.hypothesis_met` | Whether the steady state held, when the run judged it |
| | `resilience.outcome.recovery_time_s` | Seconds to recover, when the run saw it |
| `resilience.action`, `resilience.probe` | `resilience.action.name` | Name of the step, on probes too |
| | `resilience.activity.type` | `action` or `probe` |
| | `resilience.plugin.name` | The plugin that ran it |
| | `resilience.fault.type` | `injection` for an action, `observation` for a probe |
| | `resilience.fault.name` | Name of the step |
| | `resilience.target.type` | `process`, `native` or `script` |
| `script.execute` | `script.path`, `script.timeout_seconds` | The script and its timeout |
| `ssh.connect`, `ssh.execute` | `net.peer.name`, `net.peer.port` | The SSH host and port |
| | `ssh.command` | The first 256 bytes of the command |
| | `ssh.auth_method` | `key` or `agent` |
| `resilience.analytics.ingest` | `resilience.experiment.id`, `resilience.experiment.title` | The run being stored |

## Events

| Event | On span | Fields |
|---|---|---|
| `experiment.started` | `resilience.experiment` | `experiment_id`, `experiment_title`, `user`, `started_at_ns` |
| `experiment.completed` | `resilience.experiment` | `experiment_id`, `experiment_title`, `status`, `duration_ms`, `deviations` |
| `script.completed` | `script.execute` | `script.exit_code`, `script.stdout_bytes`, `script.stderr_bytes` |
| `ssh.command.completed` | `ssh.execute` | `ssh.exit_code` |
| `journal.ingested` | `resilience.analytics.ingest` | `resilience.experiment.id`, `resilience.activity.count` |

`experiment.started` and `experiment.completed` are also log records, for
a SIEM. `experiment.completed` is written once for every run, however it
ended. OTLP log records carry the trace and span id. See
[Logging](logging.md) for the log format.

## Metrics

A `resilience.outcome.status` label is `success` or `failure`.

| Metric | Kind | Labels, or what it counts |
|---|---|---|
| `tumult.experiments.total` | counter | `resilience.outcome.status`; only a completed run is `success` |
| `tumult.actions.total` | counter | `resilience.plugin.name`, `resilience.action.name`, `resilience.outcome.status` |
| `tumult.probes.total` | counter | `resilience.plugin.name`, `resilience.probe.name`, `resilience.outcome.status` |
| `tumult.hypothesis.deviations.total` | counter | `resilience.experiment.name` |
| `tumult.plugin.script.executions.total` | counter | `script.success` |
| `tumult.rollbacks.total` | counter | `resilience.action.name`, `resilience.outcome.status` |
| `tumult.plugin.errors.total` | counter | as the actions or probes counter |
| `tumult.action.duration` | histogram, seconds | as `tumult.actions.total` |
| `tumult.probe.duration` | histogram, seconds | as `tumult.probes.total` |
| `tumult.experiment.duration` | histogram, seconds | `resilience.outcome.status` |
| `resilience.store.experiments` | gauge | runs in the store |
| `resilience.store.activities` | gauge | steps in the store |
| `resilience.store.size_bytes` | gauge | size of the store file |
| `tumult.store.disk_usage_pct` | gauge | disk use of the store's volume |

Counters and histograms are deltas, see
[Delta metrics](observability-setup.md#delta-metrics). For the names in
Prometheus, see [Grafana stack](grafana-stack.md).

## Ingest limits

A value over its maximum is set to the maximum, and a zero or unreadable
value to the default, each with a warning at start.

| Variable | Default | Maximum | Bounds |
|---|---|---|---|
| `KRONIKA_OTAP_MAX_ROWS` | 250000 | 10000000 | rows per OTAP table and per dictionary |
| `KRONIKA_OTAP_MAX_ATTRIBUTES` | 1000000 | 100000000 | attributes per batch |
| `KRONIKA_OTAP_MAX_DECODED_BYTES` | 67108864 | 4294967296 | decoded Arrow buffers per batch |
| `KRONIKA_OTAP_MAX_MESSAGE_BYTES` | 4194304 | 268435456 | one message, before and after gRPC decompression |
| `KRONIKA_OTAP_MAX_STREAMS` | 64 | 4096 | Arrow streams open at once |
| `KRONIKA_OTAP_MAX_STREAMS_PER_CONNECTION` | 32 | `MAX_STREAMS` | streams from one connection |
| `KRONIKA_OTAP_MAX_STREAMS_PER_PEER` | half of `MAX_STREAMS` (32) | `MAX_STREAMS` | streams from one peer |
| `KRONIKA_OTAP_MAX_CONCURRENT_DECODES` | 4 | 256 | batches decoded or waiting for the writer at once |
| `KRONIKA_OTAP_STREAM_IDLE_SECS` | 300 | 86400 | silence before a stream is closed |
| `KRONIKA_OTAP_STREAM_LIFETIME_SECS` | 600 | 604800 | age at which a stream is closed |
| `KRONIKA_OTAP_ACK_TIMEOUT_SECS` | 30 | 3600 | wait for a client that does not read its status |
| `KRONIKA_OTAP_BATCH_DEADLINE_MS` | 10000 | 600000 | time to decode and convert one batch |
| `KRONIKA_OTAP_MAX_STREAM_RETAINED_BYTES` | 33554432 | 1073741824 | schemas and dictionaries one stream keeps |
| `KRONIKA_OTAP_MAX_RETAINED_BYTES` | 268435456 | 17179869184 | the same, all streams together |
| `KRONIKA_OTAP_MAX_CONNECTION_RETAINED_BYTES` | an eighth of the total (33554432) | the total | the same, one connection |
| `KRONIKA_OTAP_MAX_PEER_RETAINED_BYTES` | an eighth of the total (33554432) | the total | the same, one peer |
| `KRONIKA_OTAP_MAX_DICTIONARY_DELTAS` | 65536 | 16777216 | delta dictionaries per stream |
| `KRONIKA_OTAP_MAX_REJECTED_BATCHES` | 8 | 1000000 | refused batches before the stream is closed |
| `KRONIKA_GRPC_MAX_CONNECTIONS` | 1024 | 65536 | connections on the gRPC listener |
| `KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER` | 64 | `MAX_CONNECTIONS` | connections from one peer |
| `KRONIKA_GRPC_MAX_CONNECTIONS_PER_IPV6_SITE` | four times `PER_PEER` (256) | `MAX_CONNECTIONS` | connections from one IPv6 /48 |
| `KRONIKA_GRPC_HANDSHAKE_TIMEOUT_SECS` | 10 | 300 | time to finish TLS and the HTTP/2 preface |
| `KRONIKA_GRPC_AUTH_TIMEOUT_SECS` | 10 | 300 | with a token: time to make the first call that carries it |
| `KRONIKA_GRPC_MAX_CONNECTION_AGE_SECS` | 1800 | 604800 | age of a connection |
| `KRONIKA_GRPC_MAX_CONNECTION_AGE_GRACE_SECS` | 600 | 3600 | time after that age before the connection is cut |

How to read them:

- A peer is an IPv4 address or an IPv6 /64. Behind a proxy or NAT all
  clients are one peer: raise the three per-peer limits there.
- With `KRONIKA_INGEST_TOKEN` set, a connection that has not sent the
  token is closed after the auth timeout, and gives its place to a new
  one when the listener is full. Without a token the listener cannot tell
  clients apart. Run without one only where everyone who can reach the
  port is trusted.
- Each connection is a file descriptor. Raise the `nofile` limit
  (`LimitNOFILE=` in systemd) above `KRONIKA_GRPC_MAX_CONNECTIONS`, or
  lower the variable.
- Keep the Collector's `max_stream_lifetime` below
  `KRONIKA_OTAP_STREAM_LIFETIME_SECS`, so the Collector ends long streams.
- Arrow buffers must be zstd; LZ4 is refused. Span links and metric
  exemplars are not stored, on OTAP as on OTLP.

## What the sender is told

| Event | `BatchStatus` | Stream |
|---|---|---|
| Stored | `OK` | stays open |
| Lake write failed | `UNAVAILABLE`, retried | stays open |
| A batch that cannot be converted, such as multivariate metrics | `INVALID_ARGUMENT`, not retried | stays open, closed after `KRONIKA_OTAP_MAX_REJECTED_BATCHES` |
| A payload that cannot be decoded | `INVALID_ARGUMENT`, not retried | closed with `INVALID_ARGUMENT` |
| Over a limit of the batch or the stream, or over the deadline | `INVALID_ARGUMENT`, not retried | closed with `RESOURCE_EXHAUSTED` |
| Over the retained bytes all streams share | `UNAVAILABLE`, retried | closed with `RESOURCE_EXHAUSTED` |
| Missing or wrong token | none | refused with `UNAUTHENTICATED` |

Unary OTLP answers a failed write with gRPC `UNAVAILABLE` or HTTP `503`,
which exporters retry. The daemon logs a refused token with the peer's
address, never the token.
