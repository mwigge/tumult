# tumult-loadtest — Load Testing Integration

Integrates k6 and JMeter with the Tumult experiment lifecycle for resilience testing under realistic traffic.

## Prerequisites

- **k6**: Install from https://k6.io/docs/get-started/installation/
- **JMeter**: Install from https://jmeter.apache.org/download_jmeter.cgi

## k6 Driver

### Start Load Test

```toon
method[1]:
  - name: start-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-start.sh
      env:
        TUMULT_K6_SCRIPT: load/payment-api.js
        TUMULT_K6_VUS: 50
        TUMULT_K6_DURATION: 5m
    background: true
```

### Stop and Collect Metrics

```toon
rollbacks[2]:
  - name: stop-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-stop.sh

  - name: collect-metrics
    activity_type: probe
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-metrics.sh
```

### OTLP Correlation

Set `TUMULT_OTEL_ENDPOINT` to correlate k6 metrics with Tumult experiment traces:

```bash
TUMULT_OTEL_ENDPOINT=http://localhost:4317 tumult run experiment.toon
```

k6 will export its metrics through the same OTel Collector pipeline.

## JMeter Driver

### Start Load Test

```toon
method[1]:
  - name: start-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/jmeter-start.sh
      env:
        TUMULT_JMETER_PLAN: load/test-plan.jmx
        TUMULT_JMETER_THREADS: 20
        TUMULT_JMETER_DURATION: 300
    background: true
```

## Example: Chaos Under Load

```toon
title: API survives database failover under load

method[3]:
  - name: start-traffic
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-start.sh
      env:
        TUMULT_K6_SCRIPT: load/payment-api.js
        TUMULT_K6_VUS: 50
        TUMULT_K6_DURATION: 5m
    background: true

  - name: wait-for-stable-load
    activity_type: action
    provider:
      type: process
      path: sleep
      arguments[1]: "30"

  - name: kill-db-connections
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-db-postgres/actions/kill-connections.sh
      env:
        TUMULT_PG_DATABASE: myapp

rollbacks[1]:
  - name: stop-load
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-loadtest/drivers/k6-stop.sh
```
