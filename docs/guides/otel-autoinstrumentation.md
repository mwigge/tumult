---
title: OpenTelemetry Auto-instrumentation
parent: Guides
nav_order: 25
---

# OpenTelemetry auto-instrumentation and the resilience contract

Tumult scores, gates and ranks risks by **target**. Everything the platform
knows about a system comes from attributes on the telemetry it receives,
so a service only has to do one thing to take part: carry the naming
contract. It does not have to call any Tumult API, and with
auto-instrumentation it does not have to change a line of code.

This guide shows how the contract travels from an auto-instrumented
application into the lake, which attributes the platform promotes to
columns, and how to check what actually arrived. The attribute reference
is [the metadata standard](../resilience-metadata-standard.md); the
runnable example is `examples/otel/`.

## 1. What the platform reads

| Attribute | Carried as | Promoted to | Drives |
|---|---|---|---|
| `service.name`, `service.version` | resource | `spans.service_name`, `spans.service_version` | topology, performance verdicts per build |
| `resilience.target.system` | resource or span | `spans.target_system` | scores, the gate, resilience score, coverage, risks |
| `resilience.target.technology` | resource or span | `spans.target_technology` | risk rules, reports |
| `resilience.target.environment` (or semconv `deployment.environment.name`) | resource or span | `spans.target_environment` | environment scopes, gate `env=` |
| `resilience.target.component`, `.tier`, `.criticality`, `.region` | resource | `spans.resource_attrs` | reports, conformance, future rules |
| `resilience.experiment.*`, `resilience.fault.*`, `resilience.outcome.*` | span (set by the tumult runner) | experiment and fault columns | everything about a run |
| `ict.schema.version`, `ict.entity.id`, `ict.entity.tier`, `ict.entity.owner`, `ict.stage`, `ict.origin.id` | resource | `spans.resource_attrs` | ICT spine conformance (L1) |

Rules the ingest applies (`tumult-otlp`):

- A span-level `resilience.target.*` attribute always wins. The tumult
  runner sets them on every span of an experiment trace.
- When a span carries none, the **resource** attributes fill the target
  columns. That is how an auto-instrumented application declares what it
  is, once, at deploy time.
- `deployment.environment.name` (and the older `deployment.environment`)
  stand in for `resilience.target.environment` when it is absent.
- Everything else stays queryable in `resource_attrs` and `span_attrs`.

## 2. The contract through the environment

Every OpenTelemetry agent reads the same two variables. Set them at deploy
time (Helm values, a ConfigMap, the task definition, systemd unit) and the
application never sees them.

```sh
OTEL_SERVICE_NAME=checkout
OTEL_RESOURCE_ATTRIBUTES="service.version=1.4.2,\
deployment.environment.name=staging,\
resilience.target.system=api,\
resilience.target.technology=jvm,\
resilience.target.component=shop/checkout,\
resilience.target.tier=backend,\
resilience.target.criticality=high,\
ict.schema.version=0.1.0,\
ict.entity.id=cif-0042,\
ict.entity.tier=critical,\
ict.entity.owner=team-checkout"
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
OTEL_PROPAGATORS=tracecontext,baggage
```

`resilience.target.system` takes a value from the standard's enum
(`database`, `cache`, `message-broker`, `api`, `container`, `vm`, `host`,
`kubernetes`, `network`, `application`, `middleware`, `cloud-service`);
`technology` names the product; `component` is the instance and is the
only high-cardinality key, so it never becomes a metric dimension.

### Java

```sh
JAVA_TOOL_OPTIONS="-javaagent:/otel/opentelemetry-javaagent.jar" \
OTEL_SERVICE_NAME=checkout OTEL_RESOURCE_ATTRIBUTES="…as above…" \
java -jar checkout.jar
```

### Python

```sh
pip install opentelemetry-distro opentelemetry-exporter-otlp && opentelemetry-bootstrap -a install
OTEL_SERVICE_NAME=checkout OTEL_RESOURCE_ATTRIBUTES="…" opentelemetry-instrument python app.py
```

### Node.js

```sh
npm install @opentelemetry/auto-instrumentations-node @opentelemetry/exporter-trace-otlp-grpc
OTEL_SERVICE_NAME=checkout OTEL_RESOURCE_ATTRIBUTES="…" \
node --require @opentelemetry/auto-instrumentations-node/register server.js
```

### .NET

```sh
# OpenTelemetry .NET automatic instrumentation (install script sets OTEL_DOTNET_AUTO_HOME)
OTEL_SERVICE_NAME=checkout OTEL_RESOURCE_ATTRIBUTES="…" \
CORECLR_ENABLE_PROFILING=1 dotnet Checkout.dll
```

### Go and anything without an agent

Use the eBPF-based auto-instrumentation (OpenTelemetry eBPF
Instrumentation, formerly Beyla) with the same `OTEL_RESOURCE_ATTRIBUTES`,
or set the attributes on the SDK resource in the one init call.

### Kubernetes

The OpenTelemetry Operator injects the agent and the shared part of the
contract from one `Instrumentation` resource per namespace; each workload
adds one annotation and names its own target. See
`examples/otel/k8s-instrumentation.yaml`.

## 3. The Collector enforces the contract

Teams arrive with different vocabularies: plain semconv, the ICT spine,
Kubernetes labels. One Collector pipeline (`examples/otel/otel-collector.yaml`)
maps them onto the promoted keys so the lake sees one contract:

- **Defaults** (`resource` processor): schema version, environment from
  `deployment.environment(.name)`.
- **Aliases** (`transform` processor): `ict.entity.id` → `resilience.target.component`,
  `ict.entity.tier` → `resilience.target.criticality`, `k8s.namespace.name/k8s.deployment.name`
  → component, and `resilience.target.environment` back to
  `deployment.environment.name` so other backends agree.
- **Cardinality guard** (`attributes` processor): request-scoped ids are
  dropped before anything becomes a metric dimension.
- **Export** to tumultd over OTLP with the ingest token.

Point every agent at the Collector, not at the daemon, and the contract is
enforced in one place.

## 4. During a run: linking application spans to the experiment

The tumult runner sets `TRACEPARENT` (and `BAGGAGE`) on every process it
starts, so a probe or a load generator joins the experiment trace. For
spans an application emits on its own while a fault is active, the link is
the **target**: the run's `resilience.target.*` matches the application's
resource attributes, and the run window (`/api/experiments/windows`) selects
the spans in time. With `OTEL_PROPAGATORS=tracecontext,baggage` and a
caller that carries `resilience.experiment.id` as baggage, the application
spans carry the experiment id as well.

## 5. Checking what arrived: conformance

`GET /api/otel/conformance?range=24h` reports, per `service.name`, which
contract attributes the lake has actually seen and at which level:

| Level | Means |
|---|---|
| `none` | spans arrived without a target |
| `target` | `resilience.target.system`, `.technology`, `.environment` present on every span (resource or span level) |
| `spine` | `target` plus the ICT spine (`ict.schema.version`, `ict.entity.id`, `ict.entity.tier`, `ict.entity.owner`) |

The response lists the missing keys per service, so a platform team can
see which deployments still need their `OTEL_RESOURCE_ATTRIBUTES`. The
same check runs in the UI smoke script and is the first thing to look at
when a target is absent from the Resilience page (`/resilience`).

## 6. Versions

Pin what you validated. The example pins OpenTelemetry Python 1.37 /
0.58b0, the Collector contrib 0.139 and the Operator agent images of the
same generation; the daemon speaks OTLP 1.x over gRPC and HTTP and accepts
any SDK that does.
