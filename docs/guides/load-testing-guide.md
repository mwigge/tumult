---
title: Load Testing Guide
parent: Guides
nav_order: 7
---

# Load Testing Guide

Resilience testing under realistic traffic — combining chaos with load simulation.

## Why Test Under Load?

A system that handles faults at idle may fail under production traffic. Load testing during chaos experiments reveals:
- **Degradation curves** — how performance degrades under fault
- **Recovery behavior** — how quickly throughput recovers after fault removal
- **Cascading failures** — faults that only manifest under load (connection pool exhaustion, thread starvation)

## Supported Tools

| Tool | Best For | Metrics |
|------|----------|---------|
| **k6** | HTTP/gRPC APIs, scripted scenarios | req duration, error rate, throughput, VUs |

k6 is the only supported load tool.

## Integration Pattern

Load tools run as **background activities** alongside fault injection:

```
1. Start load tool (background)
2. Wait for traffic to stabilize (~30s)
3. Inject fault (chaos action)
4. Observe system under load + fault
5. Stop load tool
6. Collect load metrics
```

## k6 Quick Start

```bash
# 1. Write a k6 script
cat > load/api-test.js << 'EOF'
import http from 'k6/http';
import { check } from 'k6';

export default function () {
    const res = http.get('http://localhost:8080/api/health');
    check(res, { 'status 200': (r) => r.status === 200 });
}
EOF

# 2. Reference it in your experiment
tumult run experiment-with-load.toon
```

## Key Environment Variables

### k6

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_K6_SCRIPT` | (required) | Path to k6 test script |
| `TUMULT_K6_VUS` | 10 | Virtual users |
| `TUMULT_K6_DURATION` | 30s | Test duration |
| `TUMULT_K6_BINARY` | k6 | k6 binary path |
| `TUMULT_OTEL_ENDPOINT` | (none) | OTLP endpoint for trace correlation |
