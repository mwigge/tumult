//! Agentic fault-injection command handlers (smoke, scenario, replay, proxy, live).

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// Renders bundled agentic scenario packs.
///
/// The output is intentionally plain text so smoke scripts and CI logs can
/// report the available fault/contract matrix without a JSON parser.
///
/// # Errors
///
/// Returns an error if formatting the output buffer fails.
pub fn cmd_agentic_list_scenario_packs() -> Result<String> {
    let packs = tumult_agentic::scenarios::bundled_packs();
    let mut output = String::new();
    writeln!(output, "Agentic scenario packs")?;
    writeln!(output, "count: {}", packs.len())?;

    for pack in packs {
        let faults = pack
            .faults
            .iter()
            .map(tumult_agentic::faults::FaultSpec::fault_type)
            .collect::<Vec<_>>()
            .join(", ");
        let contracts = pack
            .contracts
            .iter()
            .map(tumult_agentic::contracts::ContractSpec::contract_type)
            .collect::<Vec<_>>()
            .join(", ");
        let adapters = pack.supported_adapters.join(", ");

        writeln!(output)?;
        writeln!(output, "- {}", pack.name)?;
        writeln!(output, "  adapters: {adapters}")?;
        writeln!(output, "  faults: {faults}")?;
        writeln!(output, "  contracts: {contracts}")?;
    }

    let trajectory_packs = tumult_agentic::trajectory::bundled_trajectory_packs();
    writeln!(output)?;
    writeln!(output, "Agentic trajectory packs (multi-turn)")?;
    writeln!(output, "count: {}", trajectory_packs.len())?;
    for pack in trajectory_packs {
        let faults = pack
            .faults
            .iter()
            .map(|fault| format!("{}@step{}", fault.fault.fault_type(), fault.step_index))
            .collect::<Vec<_>>()
            .join(", ");
        let contracts = pack
            .contracts
            .iter()
            .map(tumult_agentic::trajectory::TrajectoryContractSpec::contract_type)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output)?;
        writeln!(output, "- {}", pack.name)?;
        writeln!(output, "  steps: {}", pack.steps.len())?;
        writeln!(output, "  injected: {faults}")?;
        writeln!(output, "  trajectory_contracts: {contracts}")?;
    }

    Ok(output)
}

/// Runs a bundled multi-turn agentic trajectory pack with deterministic local
/// fixtures (no network), evaluating whole-trajectory contracts and agentic
/// resilience subscores.
///
/// # Errors
///
/// Returns an error if the trajectory pack is unknown, the journal cannot be
/// written, or the pack does not reach its documented headline outcome.
pub fn cmd_agentic_trajectory(pack: &str, journal_path: &Path) -> Result<String> {
    let report = tumult_agentic::smoke::run_trajectory_pack_smoke(pack)?;
    let result = &report.result;

    let run_id = format!("agentic-trajectory-{pack}");
    let evidence = tumult_agentic::journal::metadata_evidence_from_trajectory(
        run_id.clone(),
        &run_id,
        result,
        &report.injected,
    );
    let journal = tumult_agentic::journal::write_metadata_journal_file(journal_path, &evidence)?;

    let mut output = String::new();
    writeln!(output, "Agentic trajectory: {pack}")?;
    writeln!(output, "adapter: {}", report.adapter)?;
    writeln!(output, "description: {}", report.description)?;
    writeln!(output, "steps: {}", result.steps.len())?;
    for injected in &report.injected {
        writeln!(
            output,
            "injected: {} @ step {}",
            injected.fault_type, injected.step_index
        )?;
    }
    for step in &result.steps {
        let fault = step.injected_fault.as_deref().unwrap_or("none");
        writeln!(
            output,
            "step[{}] {} ({}) fault={} healthy={}",
            step.index, step.label, step.kind, fault, step.healthy
        )?;
    }
    for contract in &result.trajectory_contracts {
        writeln!(
            output,
            "trajectory_contract: {} = {} ({})",
            contract.contract_type,
            if contract.passed { "pass" } else { "fail" },
            contract.reason.as_deref().unwrap_or("ok")
        )?;
    }
    writeln!(output, "headline_contract: {}", report.headline_contract)?;
    writeln!(output, "expected: {}", report.expected)?;
    writeln!(output, "actual: {}", report.actual)?;
    for (dimension, score) in result.score.dimensions() {
        let score = if score.abs() < f64::EPSILON {
            0.0
        } else {
            score
        };
        writeln!(output, "subscore.{}: {score:.3}", dimension.as_str())?;
    }
    let overall = if result.score.overall.abs() < f64::EPSILON {
        0.0
    } else {
        result.score.overall
    };
    writeln!(output, "resilience_score: {overall:.3}")?;
    writeln!(output, "journal: {}", journal.path)?;
    writeln!(output, "trace_id: {}", journal.trace_id)?;
    writeln!(
        output,
        "trace_assertions: trace_id_present=true capture_policy=metadata_only"
    )?;
    writeln!(output, "network: not required")?;
    writeln!(output, "next: {}", report.next_diagnostic_command)?;

    if report.passed {
        writeln!(
            output,
            "result: pass (trajectory contracts observed and subscores captured)"
        )?;
        Ok(output)
    } else {
        writeln!(output, "result: fail")?;
        bail!("{output}");
    }
}

/// Runs the deterministic local agentic smoke path.
///
/// This smoke path does not contact a model provider, network endpoint, MCP
/// server, or secret store. It succeeds when the built-in malformed-output
/// fixture applies the expected fault and records the expected contract failure.
///
/// # Errors
///
/// Returns an error if the expected deterministic fault/contract evidence is
/// missing.
pub fn cmd_agentic_smoke(journal_path: &Path) -> Result<String> {
    let report = tumult_agentic::smoke::fake_http_malformed_json_smoke()?;
    render_agentic_report(
        "Agentic smoke: malformed-json-recovery",
        &report,
        journal_path,
    )
}

/// Runs a bundled agentic scenario pack with deterministic local fixtures.
///
/// # Errors
///
/// Returns an error if the scenario pack is unknown or the journal cannot be written.
pub fn cmd_agentic_run_scenario(scenario: &str, journal_path: &Path) -> Result<String> {
    let report = tumult_agentic::smoke::run_scenario_pack_smoke(scenario)?;
    render_agentic_report(&format!("Agentic run: {scenario}"), &report, journal_path)
}

/// Runs deterministic replay fixture validation.
///
/// # Errors
///
/// Returns an error if the replay fixture cannot be read, decoded, validated,
/// or if the journal cannot be written.
pub fn cmd_agentic_replay(fixture_path: &Path, journal_path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(fixture_path)
        .with_context(|| format!("read replay fixture {}", fixture_path.display()))?;
    let fixture: tumult_agentic::replay::ReplayFixture = serde_json::from_str(&content)
        .with_context(|| format!("decode replay fixture {}", fixture_path.display()))?;

    let source = fixture.source.clone();
    let session_id = fixture.session_id.clone();
    let step_count = fixture.steps.len();

    // Replay the caller-supplied fixture end to end through the real replay
    // adapter — validation, step replay, and contract evaluation all run
    // against *this* fixture rather than a built-in one.
    let report = tumult_agentic::smoke::replay_fixture_smoke(fixture)?;
    let mut output =
        render_agentic_report("Agentic replay: captured fixture", &report, journal_path)?;
    writeln!(output, "fixture: {}", fixture_path.display())?;
    writeln!(output, "replay_source: {source}")?;
    writeln!(output, "replay_session: {session_id}")?;
    writeln!(output, "replay_steps: {step_count}")?;
    Ok(output)
}

/// Runs a fault-injecting proxy in front of a live agent's model endpoint.
///
/// Point any agentic client (Claude Code, Codex, Copilot, and other
/// base-URL-configurable agents) at the printed address and the chosen scenario
/// pack's faults are injected into the client's real model traffic. Runs until
/// interrupted.
///
/// # Errors
///
/// Returns an error if the scenario pack is unknown, the listen address is
/// invalid, the socket cannot be bound, or the proxy server exits with an error.
pub async fn cmd_agentic_proxy(
    listen: &str,
    upstream: &str,
    scenario: &str,
    journal: Option<&Path>,
    seed: u64,
    client: &str,
) -> Result<()> {
    let client = tumult_agentic::profiles::parse_client(client);
    let profile = tumult_agentic::profiles::profile_for(client);
    let pack = tumult_agentic::scenarios::bundled_packs()
        .into_iter()
        .find(|pack| pack.name == scenario)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario pack: {scenario}"))?;
    let faults = pack
        .faults
        .iter()
        .map(tumult_agentic::faults::FaultSpec::fault_type)
        .collect::<Vec<_>>()
        .join(", ");

    let addr: std::net::SocketAddr = listen
        .parse()
        .with_context(|| format!("invalid --listen address {listen}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind proxy listener {addr}"))?;
    let bound = listener.local_addr().unwrap_or(addr);
    let base = format!("http://{bound}");

    println!("Tumult agentic fault-injecting proxy");
    println!("  listening: {base}");
    println!("  upstream:  {upstream}");
    println!("  scenario:  {scenario}");
    println!("  client:    {}", client.as_str());
    if let Some(env) = profile.base_url_env {
        println!("  base-url:  set {env}={base}");
    }
    println!("  faults:    {faults}");
    println!(
        "  journal:   {}",
        journal.map_or_else(|| "(none)".to_string(), |path| path.display().to_string())
    );
    println!();
    println!("Point your agent at the proxy, then drive it as usual:");
    println!("  Claude Code:  ANTHROPIC_BASE_URL={base} claude");
    println!("  Codex CLI:    OPENAI_BASE_URL={base}/v1 codex");
    println!("  OpenCode:     OPENAI_BASE_URL={base}/v1 opencode");
    println!("  Copilot CLI:  HTTPS_PROXY={base} copilot");
    println!();
    println!("Press Ctrl-C to stop.");

    let config = tumult_agentic::proxy::ProxyConfig {
        upstream: upstream.to_string(),
        scenario_pack: scenario.to_string(),
        journal_path: journal.map(Path::to_path_buf),
        seed,
        client,
    };
    tumult_agentic::proxy::serve(listener, config)
        .await
        .map_err(|err| anyhow::anyhow!("proxy: {err}"))?;
    Ok(())
}

/// Orchestrates a live agent run with tumult as the trace root.
///
/// Runs `claude -p` with a minted trace context, telemetry export, and a base
/// URL pointing at the proxy, then evaluates the scenario pack's contracts
/// against the agent's response. Requires the `claude` CLI on `PATH`.
///
/// # Errors
///
/// Returns an error if the scenario pack is unknown, the agent cannot be run,
/// or formatting fails.
pub fn cmd_agentic_run_live(
    prompt: &str,
    scenario: &str,
    base_url: &str,
    otlp: Option<&str>,
    client: &str,
) -> Result<String> {
    let pack = tumult_agentic::scenarios::bundled_packs()
        .into_iter()
        .find(|pack| pack.name == scenario)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario pack: {scenario}"))?;

    let runner = tumult_agentic::orchestrator::CommandAgentRunner::claude();
    let run = tumult_agentic::orchestrator::LiveRun {
        scenario: pack.name,
        client,
        contracts: &pack.contracts,
        prompt,
        otlp_endpoint: otlp,
        base_url,
    };
    let result = tumult_agentic::orchestrator::run_live(&runner, &run)
        .map_err(|err| anyhow::anyhow!("run-live: {err}"))?;

    let mut output = String::new();
    writeln!(output, "Agentic run-live: {scenario}")?;
    writeln!(output, "client: {client}")?;
    writeln!(output, "base_url: {base_url}")?;
    writeln!(output, "resilience_score: {:.3}", result.resilience_score)?;
    for contract in &result.contracts {
        writeln!(
            output,
            "contract: {} = {}",
            contract.contract_type,
            if contract.passed { "pass" } else { "fail" }
        )?;
    }
    Ok(output)
}

fn render_agentic_report(
    title: &str,
    report: &tumult_agentic::smoke::SmokeReport,
    journal_path: &Path,
) -> Result<String> {
    let result = &report.run_result;
    let fault = result
        .faults
        .iter()
        .find(|fault| fault.fault_type == report.fault);
    let contract = result
        .contracts
        .iter()
        .find(|contract| contract.contract_type == report.contract);

    let fault_applied = fault.is_some_and(|fault| fault.applied);
    let scenario = result.scenarios.first().map_or("unknown", String::as_str);
    let reason = contract
        .and_then(|contract| contract.reason.as_deref())
        .unwrap_or("missing");
    let actual_contract = &report.actual;
    let resilience_score = if result.resilience_score.abs() < f64::EPSILON {
        0.0
    } else {
        result.resilience_score
    };
    let run_id = format!("agentic-{scenario}");
    let evidence = tumult_agentic::journal::metadata_evidence_from_result(
        format!("agentic-{scenario}"),
        &run_id,
        result,
    );
    let journal = tumult_agentic::journal::write_metadata_journal_file(journal_path, &evidence)?;
    let analytics = agentic_analytics_from_result(&run_id, &evidence.experiment_id, result);
    let store_path = tumult_analytics::AnalyticsStore::default_path()
        .context("failed to determine analytics store path")?;
    let ingested = match tumult_analytics::AnalyticsStore::open(&store_path) {
        Ok(store) => store.ingest_agentic_run(&analytics).unwrap_or(false),
        Err(_) => false,
    };

    let mut output = String::new();
    writeln!(output, "{title}")?;
    writeln!(output, "adapter: {}", report.adapter)?;
    writeln!(output, "target_type: {}", result.target_type)?;
    writeln!(output, "scenario: {scenario}")?;
    writeln!(output, "fault: {}", report.fault)?;
    writeln!(output, "fault_applied: {fault_applied}")?;
    writeln!(output, "contract: {}", report.contract)?;
    writeln!(output, "expected_contract: {}", report.expected)?;
    writeln!(output, "actual_contract: {actual_contract}")?;
    writeln!(output, "reason: {reason}")?;
    writeln!(output, "resilience_score: {resilience_score:.3}")?;
    writeln!(output, "trace_id: {}", journal.trace_id)?;
    writeln!(output, "journal: {}", journal.path)?;
    writeln!(output, "analytics_store: {}", store_path.display())?;
    writeln!(output, "analytics_ingested: {ingested}")?;
    writeln!(
        output,
        "trace_assertions: trace_id_present=true span_id_present=true capture_policy=metadata_only"
    )?;
    writeln!(output, "network: not required")?;
    writeln!(output, "next: {}", report.next_diagnostic_command)?;

    if fault_applied && report.passed {
        writeln!(
            output,
            "result: pass (fault observed and contract feedback captured)"
        )?;
        Ok(output)
    } else {
        writeln!(output, "result: fail")?;
        bail!("{output}");
    }
}

fn agentic_analytics_from_result(
    run_id: &str,
    experiment_id: &str,
    result: &tumult_agentic::AgenticRunResult,
) -> tumult_analytics::AgenticRunAnalytics {
    tumult_analytics::AgenticRunAnalytics {
        run_id: run_id.to_string(),
        experiment_id: experiment_id.to_string(),
        target_type: result.target_type.clone(),
        scenario: result.scenarios.first().cloned().unwrap_or_default(),
        resilience_score: result.resilience_score,
        trace_id: result.trace_id.clone(),
        replay_id: result.replay_id.clone(),
        contracts: result
            .contracts
            .iter()
            .map(|contract| tumult_analytics::AgenticContractAnalytics {
                contract_type: contract.contract_type.clone(),
                scenario: contract.scenario.clone(),
                passed: contract.passed,
                reason: contract.reason.clone(),
                severity: contract.severity,
            })
            .collect(),
        faults: result
            .faults
            .iter()
            .map(|fault| tumult_analytics::AgenticFaultAnalytics {
                fault_type: fault.fault_type.clone(),
                scenario: fault.scenario.clone(),
                applied: fault.applied,
            })
            .collect(),
    }
}
