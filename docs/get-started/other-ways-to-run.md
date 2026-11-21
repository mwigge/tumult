---
title: Other ways to run
parent: Get started
nav_order: 1
---

# Other ways to run

[Get started](index.md) runs Tumult from a terminal against local test
targets. These are the other four ways to start it. Each needs Docker, and
the commands run from the root of a clone of the repository.

## The whole platform in the web UI

The daemon, the web UI and a demo suite, so every page has data. Open
<http://localhost:14318/> and sign in as `admin`. More in the
[Docker README](https://github.com/mwigge/tumult/blob/main/docker/README.md).

```bash
cp .env.example .env   # set the three KRONIKA_* values
docker compose -f docker/docker-compose.kronika.yml up -d --build
```

## The one-command demo

An order service under load, SigNoz and a control panel at
<http://localhost:8088>, with one fault per domain already run. More in the
[demo README](https://github.com/mwigge/tumult/blob/main/demo/README.md).

```bash
make demo        # make demo-down stops it
```

## Chaos Lab in your browser

Four guided lessons with the real engine at <http://localhost:8089>, and an
optional AI tutor. More in the
[Chaos Lab README](https://github.com/mwigge/tumult/blob/main/chaos-lab/README.md).

```bash
cd chaos-lab && docker compose up -d --build --wait
```

## Docker bundles

Start the parts you need: test targets, SigNoz with a collector, the MCP
server. More in the
[Docker README](https://github.com/mwigge/tumult/blob/main/docker/README.md#bundles).

```bash
./start.sh infra observe   # targets, then SigNoz at http://localhost:3301
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 tumult run examples/redis-chaos.toon
```
