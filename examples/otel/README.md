# Auto-instrumentation with the resilience naming contract

Nothing in `python-app/` knows about telemetry. The OpenTelemetry agent
wraps the process, the environment carries the naming contract, and the
Collector maps whatever vocabulary a team already uses (OTel semconv,
the ICT spine) onto the `resilience.*` keys the platform promotes to
columns, scores and gates by.

| File | What it shows |
|---|---|
| `python-app/` | A Flask service run under `opentelemetry-instrument`; no code changes. |
| `docker-compose.yml` | The service, a traffic generator and a Collector feeding the Kronika stack. |
| `otel-collector.yaml` | The contract enforced in one place: defaults, aliases, cardinality guard, export with the ingest token over OTel-Arrow (`otelarrow` exporter, with a commented plain-OTLP fallback). |
| `k8s-instrumentation.yaml` | The same contract through the OpenTelemetry Operator: one `Instrumentation` per namespace, one annotation per workload. |

The guide is `docs/guides/otel-autoinstrumentation.md`; the attribute
reference is `docs/resilience-metadata-standard.md`. Check what the lake
actually received with `GET /api/otel/conformance`.

The Collector talks to tumultd over OTel-Arrow (OTAP): columnar,
zstd-compressed batches on one long-lived gRPC stream per signal, on the
same port and with the same ingest token as OTLP/gRPC. That needs the
contrib distribution (`otel/opentelemetry-collector-contrib`, verified with
0.157.0); on a Collector without the `otelarrow` exporter, swap in the
commented `otlp/tumultd` exporter. The whole path, with a real Collector
container, is exercised by `scripts/proof/otap/proof.sh`; the carrier is
described in `docs/guides/observability-setup.md` ("OTel-Arrow carrier").
