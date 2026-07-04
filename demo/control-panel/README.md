# Tumult Control Panel

A role-aware web UI for [`tumult-mcp`](../../tumult-mcp). It is a pure **MCP
client**: every button is a `tools/call` over the Model Context Protocol, so the
panel drives Tumult exactly the way an autonomous agent would. It runs against
**any** `tumult-mcp` server — the [demo compose](../../docker/docker-compose.demo.yml)
is just one deployment of it.

Sections:

- **Overview** — server status and the caller's access level.
- **Author** — pick a fault from the live catalog and scaffold a validated
  experiment (`tumult_fault_catalog` + `tumult_scaffold_experiment`). Read-only:
  it generates and validates, it never runs.
- **Run** — inject faults, the auto-halt guardrail, and the full
  discover→validate→run→analyze→recommend loop. **Operator only.**
- **Analytics** — SQL over the persistent store (`tumult_analyze_store`).
- **Compliance** — regulatory evidence over the run corpus (`tumult_compliance`).
- **ChaosGraph** — the knowledge graph of everything tested
  (`tumult_chaosgraph_*`).

## Role-awareness (defense in depth)

`tumult-mcp` enforces two-tier RBAC: a **viewer** token may call read-only tools
only; an **operator** token may call everything, including fault injection and
execution. On load the panel calls the read-only `tumult_whoami` tool
(`GET /api/whoami`) to learn its own role and adapts:

| Role | Author (scaffold) | Analytics / Compliance / ChaosGraph | Run faults / guardrail / full loop |
|------|:--:|:--:|:--:|
| **operator** | ✅ | ✅ | ✅ |
| **viewer** | ✅ | ✅ | ❌ hidden/disabled with a "needs operator role" note |

This is **defense in depth**, not the security boundary: the panel hides
operator actions from viewers so the UI never offers a call the server would
reject, but the **server still enforces RBAC regardless** of what the UI shows.
If `tumult_whoami` cannot be reached, the panel assumes **least privilege
(viewer)** and shows a notice.

To see the viewer experience, point the panel at an operator-less token
(`TUMULT_MCP_TOKEN=<viewer-token>`); the Run section renders read-only. To prove
the plumbing without a browser:

```sh
curl -s localhost:8088/api/whoami
# operator token → {"role":"operator","authenticated":true,"resolved":true,...}
# viewer token   → {"role":"viewer","authenticated":true,"resolved":true,...}
```

## Configuration

All configuration is environment-driven. Each field takes a **neutral**
`TUMULT_UI_*` name first (so the same binary runs against a production
`tumult-mcp`), then falls back to the demo's `DEMO_*` / legacy names.

| Neutral env | Demo/legacy fallback | Default | Purpose |
|-------------|----------------------|---------|---------|
| `TUMULT_UI_MCP_URL` | `MCP_URL` | `http://tumult-mcp:3100` | MCP server base URL (`/mcp` appended) |
| `TUMULT_MCP_TOKEN` | — | *(none)* | Bearer token — its **role** (viewer/operator) determines what the UI exposes |
| `TUMULT_UI_EXPERIMENTS_DIR` | `DEMO_EXPERIMENTS_DIR` | `/demo/experiments` | Experiment dir **as seen inside the tumult-mcp container** |
| `TUMULT_UI_JOURNALS_DIR` | `DEMO_JOURNALS_DIR` | `/journals` | Journal corpus the Compliance card evaluates |
| `TUMULT_UI_FRAMEWORK` | `DEMO_COMPLIANCE_FRAMEWORK` | `dora` | Regulatory framework for Compliance / ChaosGraph |
| `TUMULT_UI_TARGET_HINT` | `TRACE_SERVICE` | `demo-app` | Service name to filter traces by |
| `TUMULT_UI_TARGET_URL` | `DEMO_APP_URL` | `http://localhost:8080` | Target-app link in the top bar |
| `TUMULT_UI_OBSERVABILITY_URL` | `SIGNOZ_URL` | `http://localhost:3301` | Observability UI (traces deep link) |
| `TUMULT_UI_PORT` | `PORT` | `8088` | Bind port |

## Run standalone against any tumult-mcp

```sh
cargo run -p demo-control-panel      # or: ./demo-control-panel

# pointed at a real server with an operator token
TUMULT_UI_MCP_URL=https://tumult-mcp.internal:3100 \
TUMULT_MCP_TOKEN=<operator-token> \
TUMULT_UI_TARGET_HINT=payments-api \
TUMULT_UI_FRAMEWORK=dora \
TUMULT_UI_PORT=8088 \
  ./demo-control-panel
# open http://localhost:8088
```

## Deploy: Docker Compose

```yaml
services:
  tumult-control-panel:
    build:
      context: .
      dockerfile: docker/Dockerfile.demo-control-panel
    ports:
      - "8088:8088"
    environment:
      TUMULT_UI_MCP_URL: http://tumult-mcp:3100
      TUMULT_MCP_TOKEN: ${TUMULT_MCP_TOKEN}   # operator or viewer — sets the UI tier
      TUMULT_UI_TARGET_HINT: payments-api
      TUMULT_UI_FRAMEWORK: dora
    # depends_on an existing tumult-mcp service
```

## Deploy: Kubernetes

A minimal Deployment + Service pointing at an in-cluster `tumult-mcp`
(see [`deploy/k8s/tumult-mcp.yaml`](../../deploy/k8s/tumult-mcp.yaml)):

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: tumult-control-panel
  labels: { app: tumult-control-panel }
spec:
  replicas: 2                       # stateless UI — safe to scale
  selector:
    matchLabels: { app: tumult-control-panel }
  template:
    metadata:
      labels: { app: tumult-control-panel }
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 65532
        seccompProfile: { type: RuntimeDefault }
      containers:
        - name: control-panel
          image: ghcr.io/mwigge/tumult-control-panel:latest
          ports:
            - { name: http, containerPort: 8088 }
          env:
            - { name: TUMULT_UI_MCP_URL, value: "http://tumult-mcp:3100" }
            - { name: TUMULT_UI_TARGET_HINT, value: "payments-api" }
            - name: TUMULT_MCP_TOKEN       # operator or viewer token → UI tier
              valueFrom:
                secretKeyRef: { name: tumult-mcp-token, key: token }
          readinessProbe:
            httpGet: { path: /healthz, port: http }
---
apiVersion: v1
kind: Service
metadata:
  name: tumult-control-panel
spec:
  selector: { app: tumult-control-panel }
  ports:
    - { port: 80, targetPort: http }
# Expose externally only through an Ingress that terminates TLS.
```

The panel serves `/healthz` for probes and never panics if `tumult-mcp` is
unreachable — cards report a clean in-band error and recover automatically.
