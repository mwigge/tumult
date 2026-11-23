# Tumult

[![CI](https://github.com/mwigge/tumult/actions/workflows/ci.yml/badge.svg)](https://github.com/mwigge/tumult/actions/workflows/ci.yml)
[![Coverage](https://github.com/mwigge/tumult/actions/workflows/coverage.yml/badge.svg)](https://github.com/mwigge/tumult/actions/workflows/coverage.yml)
![Version](https://img.shields.io/badge/version-3.0.3-brightgreen)
![Rust](https://img.shields.io/badge/rust-1.92.0%2B-orange)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)

Tumult is a resilience intelligence platform, written in Rust. You test your
systems. Tumult keeps the evidence and tells you what it means.

![Krönika, the web UI of Tumult, shows the Star Map: organisation, systems, services and workloads as one graph](docs/images/3.0/screenshots/starmap.png)

## Install

```bash
curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh
```

It clones this repository, builds the `tumult` CLI, starts test targets in
Docker and runs a check. You need Git, Make, Docker and Rust 1.92.0 or
newer. Then follow [Get started](https://tumult.rs/get-started/).

## Try it

**Run one experiment**, from the `tumult` folder:

```bash
tumult run examples/redis-chaos.toon
```

**See the whole platform** with demo data at <http://localhost:14318/>:

```bash
cp .env.example .env   # set the three KRONIKA_* values
docker compose -f docker/docker-compose.kronika.yml up -d --build
```

**Learn chaos testing in your browser** at <http://localhost:8089>:

```bash
cd chaos-lab && docker compose up -d --build --wait
```

## How it works

![The evidence loop: plan, run, observe, store, score, find risks, pick the next test](docs/images/3.0/diagrams/flow-evidence-loop.svg)

- **Collect:** chaos experiments, load and performance runs, telemetry from
  your services, and manual records of exercises.
- **Store:** one daemon, `tumultd`, keeps it all in one DuckDB database, the
  lake.
- **Understand:** ranked risks, scores, version verdicts and forecasts.
- **Show:** the web UI, Krönika. A CI gate (`tumult gate`) and PDF reports
  use the same data.

| | |
|---|---|
| ![Resilience: ranked risks and coverage per discipline](docs/images/3.0/screenshots/posture.png) | ![Scores: DORA coverage and maturity level](docs/images/3.0/screenshots/scores-dora.png) |
| **Resilience.** Ranked risks and the gaps in your testing, without running a fault. | **Scores.** DORA coverage per article and function, and a maturity level from 1 to 5. |
| ![Performance: version against version with a verdict](docs/images/3.0/screenshots/performance.png) | ![Run detail with the span waterfall](docs/images/3.0/screenshots/run-detail.png) |
| **Performance.** Did the new version get slower? A verdict per version. | **Run detail.** Every step of a run on a timeline, with its approvals. |

More pictures: [platform walkthrough](https://tumult.rs/guides/platform-walkthrough.html).

## Safe by design

- Every run gets a risk tier, T0 to T3. Risky runs wait for a second
  person, and nobody can approve their own run.
- Two clicks stop all running experiments and roll them back.
- Every action goes into an audit trail that shows any change.
- Roles: `viewer`, `operator`, `approver`, `admin`.

Chaos actions break things on purpose. Check every experiment, target and
rollback before you use Tumult outside a test environment. Journals keep
command output, so keep secrets out of experiment arguments. Reports
summarise evidence; they are not legal or audit statements.

## AI agents

Tumult exposes 44 tools over MCP, on stdio or HTTP. Read-only tools need the
`viewer` role. Tools that change or break things need `operator`.

```bash
tumult mcp serve                                                   # stdio
tumult mcp serve --transport http --port 3100 --token "$MCP_TOKEN"
```

<details>
<summary>All 44 tools</summary>

| Area | Tools |
|---|---|
| Experiments | `tumult_run_experiment`, `tumult_validate`, `tumult_discover`, `tumult_create_experiment`, `tumult_list_experiments` |
| Journals and analytics | `tumult_read_journal`, `tumult_list_journals`, `tumult_analyze`, `tumult_analyze_store`, `tumult_store_stats`, `tumult_query_traces`, `tumult_report`, `tumult_compliance`, `tumult_trend` |
| GameDays | `tumult_gameday_run`, `tumult_gameday_analyze`, `tumult_gameday_create`, `tumult_gameday_list` |
| Intelligence | `tumult_recommend`, `tumult_coverage`, `tumult_agents`, `tumult_fault_catalog`, `tumult_scaffold_experiment` |
| Agentic testing | `tumult_agentic_list_scenarios`, `tumult_agentic_smoke`, `tumult_agentic_run_experiment` |
| ChaosGraph | `tumult_chaosgraph_query`, `tumult_chaosgraph_neighbors`, `tumult_chaosgraph_coverage_gaps`, `tumult_chaosgraph_cypher` |
| Topology | `tumult_topology_import`, `tumult_topology_map`, `tumult_compliance_lineage`, `tumult_recommend_injection` |
| Autopilot | `tumult_autopilot_run`, `tumult_autopilot_status`, `tumult_autopilot_respond`, `tumult_autopilot_export`, `tumult_autopilot_notify` |
| Manual registry | `tumult_manual_list`, `tumult_manual_get`, `tumult_manual_create_draft`, `tumult_manual_plans_list` |
| Access | `tumult_whoami` |

Thirty-four tools return structured content with an output schema. Four tools can
break things: `tumult_run_experiment`, `tumult_gameday_run`,
`tumult_autopilot_run` (with `execute=true`) and `tumult_autopilot_respond`
(with `approve=true`).

</details>

More in the [MCP guide](https://tumult.rs/guides/mcp-guide.html).

## Upgrading from 2.x

Read the [3.0.0 release notes](https://tumult.rs/releases/3.0.0.html) first.
The lake moves to schema v21 and DuckDB 2.0, and an older Tumult cannot
open it afterwards. Stop the daemon and back up first with
`tumult store backup --output <dir>`.

## Documentation

All of it is on [tumult.rs](https://tumult.rs/).

| I want to | Go to |
|---|---|
| Start | [Get started](https://tumult.rs/get-started/) |
| Click through the web UI | [Platform walkthrough](https://tumult.rs/guides/platform-walkthrough.html) |
| Learn the ideas | [Concepts](https://tumult.rs/concepts/) |
| Find a guide | [Guides](https://tumult.rs/guides/) |
| Look up a command | [CLI reference](https://tumult.rs/guides/cli-reference.html) |
| Run it for real | [Operate](https://tumult.rs/operate/) |
| See what changed | [Release notes](https://tumult.rs/releases/) |

Report a security issue through
[GitHub private vulnerability reporting](https://github.com/mwigge/tumult/security/advisories/new).
See [SECURITY.md](SECURITY.md).

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) and run its checks before you send
a change.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
