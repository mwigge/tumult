---
title: Platform Walkthrough
parent: Guides
nav_order: 22
---

# Platform Walkthrough

A picture tour of Krönika, the Tumult web UI. First the pages that tell you
how resilient you are. Then how to run an experiment safely. Then the pages
for daily work.

Pictures come from the demo stack. Your numbers will differ.

## Start the demo

```bash
cp .env.example .env
# Set the three KRONIKA_* values in .env. The file tells you how.
docker compose -f docker/docker-compose.kronika.yml up -d --build
```

Open <http://localhost:14318/> and sign in as `admin` with the password you
set. The demo runs eight experiments for real, adds three manual records,
and creates a second user, `bob`, who can approve runs.

![Login page](../assets/login.png)

## Part 1: What does the evidence say?

### Overview

How are we doing? Pass rate, deviations, time to recover and the mix of
faults, for the time range you pick.

![Overview dashboard](../images/3.0/screenshots/overview.png)

### Star Map

Everything in one graph: organisation, systems, services, workloads and
experiments. Each kind has its own colour and each arrow carries the name
of the relation. Switch the lens to ring the nodes by evidence, by risk or
by telemetry quality. Click a node to see what is known about it.

![Star Map](../images/3.0/screenshots/starmap.png)

Double-click a node to focus on it. The drawer shows its evidence.

![Star Map focused on one system](../images/3.0/screenshots/starmap-focus.png)

![How the Star Map is built](../images/3.0/diagrams/flow-star-map.png)

More: [Star Map guide](star-map.md).

### Resilience

A ranked list of risks, found without running any fault. Examples: the last
run failed, a service was never tested, a workload has only one replica.
Below it, a grid shows which kinds of test each target has had.

You can accept a risk for a while. The acceptance is recorded and expires
by itself.

![Resilience page with ranked risks and coverage](../images/3.0/screenshots/posture.png)

### Scores and DORA

Scores roll up from single experiments to teams to the whole company. The
DORA view shows which articles and business functions have evidence, and a
maturity level from 1 to 5.

The score shows what evidence exists. It does not say that you comply.

![Scores page](../images/3.0/screenshots/scores.png)

![DORA coverage and maturity](../images/3.0/screenshots/scores-dora.png)

![From evidence to a DORA score](../images/3.0/diagrams/flow-dora-score.png)

More: [DORA score guide](dora-score.md).

### Performance

Did the new version get slower? Pick a target. Tumult compares two versions
and gives a verdict: better, worse, the same, or not enough data.

![Performance page with a version verdict](../images/3.0/screenshots/performance.png)

![From a load run to a verdict](../images/3.0/diagrams/flow-performance-verdict.png)

More: [performance guide](performance-regression.md).

## Part 2: Run an experiment safely

```mermaid
flowchart LR
    accTitle: A governed run
    accDescr: Pick a fault, register the experiment, do a dry run, wait for approval if the risk tier needs it, run, and read the result. A run can be stopped at any time.
    pick[Pick a fault] --> register[Register]
    register --> dry[Dry run]
    dry --> approve{Approval<br>needed?}
    approve -->|yes| wait[Another person approves]
    approve -->|no| run[Run]
    wait --> run
    run --> result[Read the result]
    run -.->|any time| stop[Stop]
```

### 1. Pick a fault

The Author page lists every fault and probe your plugins offer. Pick one
and a wizard writes the experiment file for you.

![Author page with the fault catalog](../assets/author-catalog.png)

![The wizard for one fault](../assets/author-new.png)

### 2. Dry run, then start

Choose the experiment, fill in its values, and look at the dry run. It
shows the exact steps and rollbacks. Then start.

![New run with a dry-run preview](../assets/runs-new-dry-run.png)

### 3. Approval

Every run gets a risk tier, T0 to T3. Higher tiers wait for another person
to approve. You can never approve your own run. The approval is tied to the
exact content of the run, and it expires.

An administrator must first allow the experiment to touch its target. That
is an [execution binding](execution-bindings.md). Without one, the run is
treated as the highest tier.

![A run waiting for approval](../assets/run-pending-approval.png)

![The approver reviews the request](../assets/approvals-queue-review.png)

### 4. Read the result

One page tells the story: each step on a timeline, who approved, and an
audit trail that cannot be changed quietly. The Impact panel shows how far
the fault pushed each probe from its normal range, and how long it took to
come back.

![Run detail with the timeline](../images/3.0/screenshots/run-detail.png)

More: [forecasting and run impact](forecasting.md).

### 5. Stop

Stopping takes two clicks, so it cannot happen by accident. A stopped run
rolls back. **Halt all** on the Runs page stops everything at once.

![Stop confirmation](../assets/run-estop-confirm.png)

![A stopped run](../assets/run-estop-aborted.png)

## Part 3: Reports

Tumult makes three PDF reports from the stored evidence.

| Report | For whom | Content |
|---|---|---|
| R1 | Management | A short resilience digest |
| R2 | Audit and risk | An evidence pack for DORA, NIS2, ISO 27001 and SOC 2, with the approval chain |
| R3 | The team | One GameDay in detail |

Reports summarise evidence. They do not certify compliance. Users who may
see only some environments get reports for those environments only.

![Reports page](../assets/reports-evidence-pack.png)

## Part 4: Daily work

| Page | What it is for | Picture |
|---|---|---|
| Runs | All runs, with filters and **Halt all** | ![Runs](../assets/runs-stop-all.png) |
| Manual | Records of exercises that telemetry cannot see. A second person verifies each one. | ![Manual evidence](../assets/manual-evidence.png) |
| GameDays | Groups of experiments run as one scored campaign | ![GameDays](../assets/gamedays.png) |
| Schedules | Run experiments on a timer | ![Schedules](../assets/schedules.png) |
| Events | Who did what, newest first | ![Events](../assets/events.png) |
| Webhooks | Send events to other tools, signed | ![Webhooks](../assets/webhooks.png) |
| Users | Accounts, roles and API tokens | ![Users](../assets/users.png) |

## All pages

Every page reads the same database through the API. A page stores nothing
of its own.

| Page | Path | What you see | Main API |
|---|---|---|---|
| Overview | `/` | Key numbers, runs per day, fault mix | `GET /api/overview` |
| Resilience | `/resilience` | Ranked risks, coverage grid, resilience score | `GET /api/risks`, `GET /api/posture` |
| Performance | `/performance` | Version comparison and verdicts | `GET /api/perf/{target}` |
| Scores | `/scores` | Company score, treemap by team | `GET /api/scores/tree` |
| DORA | `/scores?view=dora` | Article coverage, maturity level | `GET /api/dora`, `GET /api/maturity` |
| Star Map | `/starmap` | The graph, three lenses, evidence drawer | `GET /api/starmap` |
| Experiments | `/experiments` | Every experiment run, with a trace timeline | `GET /api/experiments` |
| Runs | `/runs` | Daemon runs, new run, run detail with Impact | `GET /api/runs`, `GET /api/runs/{id}/impact` |
| Author | `/author` | Fault catalog and wizard | `GET /api/authoring/catalog` |
| GameDays | `/gamedays` | Campaigns and their runs | `GET /api/gamedays` |
| Approvals | `/approvals` | Requests waiting for a decision | `GET /api/approvals` |
| Schedules | `/schedules` | Recurring runs | `GET /api/schedules` |
| Manual | `/manual` | Manual evidence records | `GET /api/manual/experiments` |
| Logs | `/logs` | Log volume and search | `GET /api/logs` |
| Traces | `/traces` | Trace durations and timelines | `GET /api/traces` |
| Metrics | `/metrics` | Any metric as a chart | `GET /api/metrics/query` |
| Ask | `/ask` | Ask a question, get SQL and a table | `POST /api/ask` |
| Reports | `/reports` | R1, R2, R3 as PDF | `GET /api/reports/v2` |
| Events | `/events` | Run events with their actor | `GET /api/events` |
| Users | `/users` | Accounts and tokens (admin) | `GET /api/users` |
| Webhooks | `/webhooks` | Event sinks (admin) | `GET /api/webhooks` |

Two things have an API but no page yet: forecast bands
(`GET /api/forecast`) and entities from
[entity events](entity-events.md) (`GET /api/entities`).

Every chart, panel and API route of each page is in the
[page reference](../reference/kronika-pages.md).

## Where next

- [Architecture](../architecture/kronika-architecture.md): how it works
  behind the screens.
- [Production deployment](production-deployment.md): run it for real.
- [CLI reference](cli-reference.md): the same work from a terminal.
