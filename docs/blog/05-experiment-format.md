# <img src="../images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Writing Your First Experiment: The TOON Format in Depth

![Tumult Banner](../images/tumult-banner.png)

*Part 5 of the Tumult series. [← Part 4: The Plugin System](./04-plugin-system.md)*

---

A chaos experiment is a scientific experiment. It has a hypothesis, a method, a measurement, and a conclusion. The experiment format is where all of that is expressed. Get the format right, and the engine can validate your experiment before it runs, execute it faithfully, and produce a journal that others can read and reproduce.

Tumult's experiment format is TOON, and this post is a complete reference for writing experiments that work well in production.

---

## Start With the Generator

Before diving into the format, it is worth knowing that you do not need to write experiments from scratch:

```bash
# Generate a generic experiment template
tumult init

# Generate a template pre-filled for a specific plugin
tumult init --plugin tumult-kubernetes
```

`tumult init` creates `experiment.toon` in the current directory with a working template. Always validate before running:

```bash
tumult validate experiment.toon
```

`validate` reports the experiment structure, plugin references, configuration resolution status, and any structural errors.

---

## The Anatomy of an Experiment

Every Tumult experiment has the same sections. Some are required; most are optional but recommended for production-quality experiments.

```
┌─────────────────────────────────────────────────────┐
│  Identity          title, description, tags         │
│  Configuration     env vars, secrets                │
│  Estimate          Phase 0 — prediction             │
│  Baseline          Phase 1 — statistical config     │
│  Steady State      probes that define "healthy"     │
│  Method            fault injection steps            │
│  Rollbacks         restoration steps               │
│  Regulatory        compliance mapping              │
└─────────────────────────────────────────────────────┘
```

---

## Section Reference

### Identity

```toon
title: Redis cache flush validates cache-aside pattern
description: |
  Flush the Redis cache and verify that the application falls back
  to the database and refills the cache within the SLA window.

tags[3]: cache, redis, resilience
```

`tags` drives analytics filtering. Use consistent values like `database`, `kubernetes`, `network`, `cache`, `resilience`, and team names to enable cross-experiment queries.

### Configuration

Configuration provides named values that can be referenced in provider arguments. Values are resolved at runtime from environment variables:

```toon
configuration:
  redis_host:
    type: env
    key: REDIS_HOST
  app_url:
    type: env
    key: APP_URL
```

### Secrets

Secrets follow the same structure but are redacted from logs and journal output:

```toon
secrets:
  db_password:
    type: env
    key: DATABASE_PASSWORD
  ssh_key:
    type: file
    path: /run/secrets/tumult-ssh-key
```

### Estimate (Phase 0)

The estimate is your hypothesis about what will happen. Write it before looking at recent metrics. Its accuracy is tracked across runs.

```toon
estimate:
  expected_outcome: recovered       # recovered | deviated | unaffected
  expected_recovery_s: 8.0          # seconds to full recovery
  expected_degradation: moderate    # none | minor | moderate | severe
  expected_data_loss: false
  confidence: high                  # low | medium | high
  rationale: Cache-aside pattern ensures DB fallback on cache miss
  prior_runs: 12
```

### Baseline (Phase 1)

The baseline configuration controls how the engine establishes "normal" before injecting faults. See [Part 8](./08-statistical-baselines.md) for a detailed treatment of baseline methods.

```toon
baseline:
  duration_s: 120.0     # how long to sample
  warmup_s: 15.0        # discard first N seconds (settling time)
  interval_s: 2.0       # sample every 2 seconds
  method: mean_stddev   # statistical method
  sigma: 2.0            # 2 standard deviations = ~95% of normal values
  confidence: 0.95
```

### Steady State Hypothesis

The hypothesis defines what "healthy" looks like. It is checked twice: before fault injection (to confirm the system is healthy to start) and after (to determine if the system deviated).

```toon
steady_state_hypothesis:
  title: Cache hit rate is acceptable and app responds
  probes[2]:
    - name: app-responds
      activity_type: probe
      provider:
        type: http
        method: GET
        url: http://localhost:8080/health
        timeout_s: 3.0
      tolerance:
        type: exact
        value: 200

    - name: cache-hit-rate
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-redis/probes/hit-rate.sh
      tolerance:
        type: range
        from: 0.7        # tolerate >= 70% hit rate
        to: 1.0
```

If **any** probe fails its tolerance, the hypothesis is not met. Failing the hypothesis before the method causes the experiment to abort. Failing it after the method marks the experiment as `deviated`.

### Tolerance Types

| Type | Description | Example |
|------|-------------|---------|
| `exact` | Value must match exactly | `value: 200` |
| `range` | Numeric value within bounds | `from: 0, to: 500` |
| `regex` | String output matches pattern | `pattern: "^healthy"` |

### Method

The method is the ordered sequence of fault injection steps. Actions change system state. Probes observe it.

```toon
method[3]:
  - name: flush-redis-cache
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-redis/actions/flush-all.sh
      env:
        TUMULT_REDIS_HOST: "{{ configuration.redis_host }}"
    pause_after_s: 2.0      # wait 2 seconds after flushing

  - name: measure-cache-miss-rate
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-redis/probes/hit-rate.sh
    background: false

  - name: send-load-spike
    activity_type: action
    provider:
      type: http
      method: POST
      url: http://localhost:8080/simulate-load
      body: '{"requests": 500}'
      timeout_s: 10.0
    background: true        # run concurrently with next step
```

**Activity fields:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique step identifier |
| `activity_type` | `action` or `probe` | Actions mutate; probes observe |
| `provider` | Provider | How the activity executes |
| `pause_before_s` | float | Wait before executing |
| `pause_after_s` | float | Wait after executing |
| `background` | bool | Run concurrently with next step |

### Provider Types

**HTTP provider** — direct HTTP call:
```toon
provider:
  type: http
  method: GET
  url: http://localhost:8080/health
  headers:
    Authorization: "Bearer {{ secrets.api_token }}"
  timeout_s: 5.0
```

**Process provider** — run a script or binary:
```toon
provider:
  type: process
  path: /usr/local/bin/redis-cli
  arguments[2]: FLUSHALL, ASYNC
  env:
    REDIS_HOST: "{{ configuration.redis_host }}"
  timeout_s: 30.0
```

**Native provider** — call a compiled Rust plugin:
```toon
provider:
  type: native
  plugin: tumult-kubernetes
  function: delete_pod
  arguments:
    namespace: production
    name: api-server-7b8c9d-xk2p1
    grace_period_seconds: 0
```

### Execution Targets

By default, activities run on the local machine. For remote execution:

```toon
- name: stress-remote-db
  activity_type: action
  provider:
    type: process
    path: /usr/bin/stress-ng
    arguments[2]: --cpu, 4, --timeout, 60s
  execution_target:
    type: ssh
    host: db-primary.example.com
    port: 22
    user: ops
    key_path: /home/ops/.ssh/tumult_ed25519
```

Supported execution targets: `local`, `ssh`, `container`, `kube_exec`.

### Rollbacks

Rollback steps restore system state after the experiment. They execute according to the rollback strategy (default: `on-deviation`).

```toon
rollbacks[1]:
  - name: confirm-cache-populated
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-redis/actions/warm-cache.sh
      env:
        TUMULT_REDIS_HOST: "{{ configuration.redis_host }}"
    background: false
```

Run with a specific rollback strategy:

```bash
tumult run experiment.toon --rollback-strategy always   # always rollback
tumult run experiment.toon --rollback-strategy never    # never rollback
tumult run experiment.toon --rollback-strategy deviated # default
```

### Regulatory Mapping

Tag experiments with the regulatory frameworks they provide evidence for:

```toon
regulatory:
  frameworks[2]: DORA, NIS2
  requirements[2]:
    - id: DORA-Art25
      description: ICT resilience testing
      evidence: Cache failure recovery within SLA
    - id: NIS2-Art21-2c
      description: Business continuity
      evidence: Service continues during cache failure
```

This mapping appears in the journal and enables SQL queries that filter experiments by compliance requirement.

---

## A Complete Production-Grade Experiment

Putting it all together — a real experiment for validating Kafka consumer resilience when a broker is killed:

```toon
title: Kafka consumer survives broker kill
description: |
  Kill one Kafka broker in a 3-broker cluster and verify that
  consumers rebalance and resume within 30 seconds.

tags[3]: kafka, messaging, resilience

configuration:
  kafka_bootstrap:
    type: env
    key: KAFKA_BOOTSTRAP_SERVERS
  consumer_group:
    type: env
    key: KAFKA_CONSUMER_GROUP

estimate:
  expected_outcome: recovered
  expected_recovery_s: 20.0
  expected_degradation: moderate
  expected_data_loss: false
  confidence: medium
  rationale: 3-broker cluster with replication factor 3 — single broker loss should trigger consumer rebalance
  prior_runs: 3

baseline:
  duration_s: 60.0
  warmup_s: 10.0
  interval_s: 5.0
  method: mean_stddev
  sigma: 2.0

steady_state_hypothesis:
  title: Consumer lag is acceptable
  probes[1]:
    - name: consumer-lag
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/consumer-lag.sh
        env:
          TUMULT_BOOTSTRAP: "{{ configuration.kafka_bootstrap }}"
          TUMULT_GROUP: "{{ configuration.consumer_group }}"
      tolerance:
        type: range
        from: 0
        to: 100

method[1]:
  - name: kill-kafka-broker-1
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-kafka/actions/kill-broker.sh
      env:
        TUMULT_BROKER_ID: "1"
    pause_after_s: 5.0
    background: false

rollbacks[1]:
  - name: restart-kafka-broker-1
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-kafka/actions/start-broker.sh
      env:
        TUMULT_BROKER_ID: "1"

regulatory:
  frameworks[1]: DORA
  requirements[1]:
    - id: DORA-Art25
      description: ICT resilience testing
      evidence: Messaging layer recovery within SLA
```

Run it:

```bash
tumult run kafka-broker-kill.toon --journal-path journals/kafka-$(date +%Y%m%d).toon
```

The journal captures every phase: the baseline consumer lag, the spike during broker kill, the recovery time, and how accurately the 20-second estimate compared to actual recovery.

---

## Dry Run: See the Plan Without Executing

Before running an experiment in a new environment, use `--dry-run` to see exactly what would execute:

```bash
tumult run experiment.toon --dry-run
```

Output:
```
Dry run: PostgreSQL failover recovery validation
═══════════════════════════════════════════════

Configuration:
  db_host → DATABASE_HOST = "db-primary.staging.internal"

Phase 0 — Estimate:
  expected_outcome: recovered
  expected_recovery_s: 15.0
  confidence: high

Phase 1 — Baseline:
  method: mean_stddev, σ=2.0
  duration: 120s, interval: 2s, warmup: 15s

Hypothesis (BEFORE):
  ✓ health-check  [HTTP GET http://localhost:8080/health → 200]

Method:
  1. kill-db-connections  [native:tumult-db:terminate_connections]
     pause_after: 5s

Hypothesis (AFTER):
  ✓ health-check  [HTTP GET http://localhost:8080/health → 200]

Rollbacks:
  1. restore-connections  [native:tumult-db:reset_connection_pool]

Rollback strategy: on-deviation
```

The dry run resolves configuration values, validates plugin references, and shows the complete execution plan. No experiment runs, nothing is modified.

---

*Next in the series: [Part 6 — Data-Driven Chaos: SQL Analytics Over Experiment Journals →](./06-analytics-pipeline.md)*
