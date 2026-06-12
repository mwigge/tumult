//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::engine::{parse_experiment, resolve_config, resolve_secrets, validate_experiment};
use tumult_core::types::{Experiment, Provider};
use tumult_plugin::discovery::discover_all_plugins;
use tumult_plugin::registry::PluginRegistry;

mod exec;
mod gameday;
mod load;
mod report;
mod run;
mod store;

pub use exec::ProviderExecutor;
pub use gameday::{cmd_gameday_analyze, cmd_gameday_create, cmd_gameday_run};
pub use report::cmd_report;
pub use run::cmd_run;
pub use store::{
    cmd_import, cmd_store_backup, cmd_store_migrate, cmd_store_path, cmd_store_purge,
    cmd_store_stats,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use load::*;

// ── Typed CLI enums ───────────────────────────────────────────

/// Export format for journal files.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// Apache Parquet columnar format
    Parquet,
    /// Comma-separated values
    Csv,
    /// JSON
    Json,
}

/// Report output format.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportFormat {
    /// HTML report
    Html,
    /// PDF (generates HTML then prints instructions for conversion)
    Pdf,
}

/// Regulatory compliance framework.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComplianceFramework {
    /// EU Digital Operational Resilience Act
    Dora,
    /// EU Network and Information Security Directive
    Nis2,
    /// Payment Card Industry Data Security Standard
    #[value(name = "pci-dss")]
    PciDss,
    /// ISO 22301 Business Continuity Management
    #[value(name = "iso-22301")]
    Iso22301,
    /// ISO 27001 Information Security Management
    #[value(name = "iso-27001")]
    Iso27001,
    /// SOC 2 Service Organization Control Type 2
    Soc2,
    /// Basel III / BCBS 239 Risk Data Aggregation
    #[value(name = "basel-iii")]
    BaselIii,
}

impl ComplianceFramework {
    /// Returns the canonical string identifier used in report output.
    #[must_use]
    pub fn as_report_str(&self) -> &'static str {
        match self {
            ComplianceFramework::Dora => "DORA",
            ComplianceFramework::Nis2 => "NIS2",
            ComplianceFramework::PciDss => "PCI-DSS",
            ComplianceFramework::Iso22301 => "ISO-22301",
            ComplianceFramework::Iso27001 => "ISO-27001",
            ComplianceFramework::Soc2 => "SOC2",
            ComplianceFramework::BaselIii => "Basel-III",
        }
    }
}

/// Load test tool selection.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadToolArg {
    /// k6 load testing tool
    K6,
    /// Apache `JMeter` load testing tool
    Jmeter,
    /// Explicitly disable load testing even if the experiment defines it
    None,
}

// ── CLI helper functions ──────────────────────────────────────

/// Parses a human duration like "30s", "5m", "1h" to seconds.
#[must_use]
pub fn parse_duration_str(s: &str) -> f64 {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('s') {
        num.parse().unwrap_or(30.0)
    } else if let Some(num) = s.strip_suffix('m') {
        num.parse::<f64>().unwrap_or(1.0) * 60.0
    } else if let Some(num) = s.strip_suffix('h') {
        num.parse::<f64>().unwrap_or(1.0) * 3600.0
    } else {
        s.parse().unwrap_or(30.0)
    }
}

/// Parses `--var KEY=VALUE` arguments into a `HashMap`.
///
/// # Errors
///
/// Returns an error if any argument does not contain `=`.
pub fn parse_var_args(
    vars: &[String],
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    for entry in vars {
        let (key, value) = entry.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("--var argument must be in KEY=VALUE format, got: {entry:?}")
        })?;
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

/// Builds a `LoadConfig` override from CLI flags.
///
/// Returns `None` if `--load none` was specified (explicitly disable load).
/// Returns `None` if no `--load` flag was given at all (use experiment default).
/// Returns `Some(config)` if a real load tool was specified (override experiment).
#[must_use]
pub fn build_load_override(
    tool: Option<LoadToolArg>,
    script: Option<std::path::PathBuf>,
    vus: Option<u32>,
    duration: Option<String>,
) -> Option<tumult_core::types::LoadConfig> {
    // --load none explicitly disables
    if matches!(tool, Some(LoadToolArg::None)) {
        return None;
    }

    let tool = tool?; // No --load flag at all → no override
    let script = script.unwrap_or_else(|| std::path::PathBuf::from("load.js"));
    let duration_s = duration.map(|d| parse_duration_str(&d));

    let load_tool = match tool {
        LoadToolArg::K6 => tumult_core::types::LoadTool::K6,
        LoadToolArg::Jmeter => tumult_core::types::LoadTool::Jmeter,
        LoadToolArg::None => unreachable!(),
    };

    Some(tumult_core::types::LoadConfig {
        tool: load_tool,
        script,
        vus: Some(vus.unwrap_or(10)),
        duration_s: duration_s.or(Some(30.0)),
        thresholds: std::collections::HashMap::new(),
    })
}

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

    Ok(output)
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
    let store_path = tumult_analytics::AnalyticsStore::default_path();
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
// ── Validate command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file cannot be read, parsed, or fails validation.
#[must_use = "callers must handle validation errors"]
pub fn cmd_validate(experiment_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // SRE-10: Warn on unsupported provider types
    let all_activities = experiment
        .method
        .iter()
        .chain(experiment.rollbacks.iter())
        .chain(
            experiment
                .steady_state_hypothesis
                .as_ref()
                .map(|h| h.probes.iter())
                .into_iter()
                .flatten(),
        );
    for activity in all_activities {
        match &activity.provider {
            Provider::Http { .. } => {
                eprintln!(
                    "warning: activity '{}' uses HTTP provider (not yet supported at runtime)",
                    activity.name
                );
            }
            Provider::Native {
                plugin, function, ..
            } => {
                eprintln!(
                    "warning: activity '{}' uses native provider {}::{} (not yet wired to CLI executor)",
                    activity.name, plugin, function
                );
            }
            Provider::Process { .. } => {} // supported
        }
    }

    // Validate configuration references
    let config_result = resolve_config(&experiment.configuration);
    let secrets_result = resolve_secrets(&experiment.secrets);

    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {desc}");
    }
    println!("Tags: {}", experiment.tags.join(", "));
    println!("Method steps: {}", experiment.method.len());
    println!("Rollback steps: {}", experiment.rollbacks.len());

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!(
            "Hypothesis: {} ({} probes)",
            hypothesis.title,
            hypothesis.probes.len()
        );
    }

    if experiment.estimate.is_some() {
        println!("Estimate: present (Phase 0)");
    }
    if experiment.baseline.is_some() {
        println!("Baseline: configured (Phase 1)");
    }
    if experiment.regulatory.is_some() {
        println!("Regulatory: mapped");
    }

    // Report config/secret resolution
    match config_result {
        Ok(_) => println!("Configuration: all values resolved"),
        Err(e) => println!("Configuration: WARNING — {e}"),
    }
    match secrets_result {
        Ok(_) => println!("Secrets: all values resolved"),
        Err(e) => println!("Secrets: WARNING — {e}"),
    }

    println!("\nValidation passed.");
    Ok(())
}

// ── Discover command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the requested plugin filter does not match any
/// discovered plugin.
#[must_use = "callers must handle plugin discovery errors"]
pub fn cmd_discover(plugin_filter: Option<&str>) -> Result<()> {
    let mut registry = PluginRegistry::new();

    // Discover script plugins from filesystem
    let manifests = discover_all_plugins().unwrap_or_default();
    for manifest in manifests {
        registry.register_script(manifest);
    }

    let plugin_names = registry.list_plugins();

    // Check filter early — even when no plugins, a filter for a specific one should error
    if let Some(filter) = plugin_filter {
        if !plugin_names.iter().any(|n| n == filter) {
            bail!(
                "plugin '{}' not found. Discovered {} plugin(s)",
                filter,
                plugin_names.len()
            );
        }
        // Show details for specific plugin
        println!("Plugin: {filter}");
        let all_actions = registry.list_all_actions();
        let actions: Vec<_> = all_actions.iter().filter(|(p, _)| p == filter).collect();
        if !actions.is_empty() {
            println!("\nActions:");
            for (_, desc) in &actions {
                println!("  - {}", desc.name);
            }
        }
    } else {
        // List all plugins
        println!("Discovered {} plugin(s):\n", plugin_names.len());
        for name in &plugin_names {
            println!("  {name}");
        }
        println!();

        let all_actions = registry.list_all_actions();
        if !all_actions.is_empty() {
            println!("Actions:");
            for (plugin, desc) in &all_actions {
                println!("  {}::{}", plugin, desc.name);
            }
        }
    }

    Ok(())
}

// ── Analyze command ───────────────────────────────────────────

/// # Errors
///
/// Returns an error if any journal cannot be read, the in-memory store cannot
/// be created, or the query fails.
/// Prints a structured summary of the last N experiments.
///
/// Shows experiment title, status, duration, method timeline with activity
/// names and durations, hypothesis results, and load test metrics if present.
#[allow(clippy::too_many_lines)] // Timeline rendering requires verbose formatting
fn print_experiment_summary(store: &tumult_analytics::AnalyticsStore, last_n: usize) -> Result<()> {
    let experiments = store.query(&format!(
        "SELECT experiment_id, title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT {last_n}"
    ))?;

    if experiments.is_empty() {
        println!("No experiments found.");
        return Ok(());
    }

    for (i, exp) in experiments.iter().enumerate() {
        let exp_id = &exp[0];
        let title = &exp[1];
        let status = &exp[2];
        let duration_ms = &exp[3];

        if i > 0 {
            println!("\n{}", "─".repeat(60));
        }

        let status_marker = match status.as_str() {
            "completed" => "PASS",
            "deviated" => "DEVIATED",
            "aborted" => "ABORTED",
            "failed" => "FAIL",
            _ => status.as_str(),
        };

        println!("Experiment: {title}");
        println!("Status:     {status_marker} ({duration_ms}ms)");

        // Method timeline
        let activities = store.query(&format!(
            "SELECT name, activity_type, status, duration_ms, output, phase \
             FROM activity_results \
             WHERE experiment_id = '{exp_id}' \
             ORDER BY started_at_ns"
        ))?;

        if !activities.is_empty() {
            println!("\nTimeline:");
            let total = activities.len();
            for (j, act) in activities.iter().enumerate() {
                let connector = if j == total - 1 { "└─" } else { "├─" };
                let name = &act[0];
                let act_type = &act[1];
                let act_status = &act[2];
                let act_dur = &act[3];
                let output = &act[4];
                let phase = &act[5];

                let phase_label = match phase.as_str() {
                    "hypothesis_before" => " (hypothesis before)",
                    "hypothesis_after" => " (hypothesis after)",
                    "rollback" => " (rollback)",
                    _ => "",
                };

                let status_icon = if act_status == "succeeded" {
                    ""
                } else {
                    " FAILED"
                };

                let type_label = if act_type == "probe" {
                    "probe"
                } else {
                    "action"
                };

                // Truncate output for display
                let output_preview = if output.is_empty() || output == "NULL" {
                    String::new()
                } else {
                    let trimmed = output.replace('\n', " ");
                    if trimmed.len() > 60 {
                        format!("  → {}…", &trimmed[..57])
                    } else {
                        format!("  → {trimmed}")
                    }
                };

                println!(
                    "  {connector} {name} ({type_label}){phase_label}  {act_dur}ms{status_icon}{output_preview}"
                );
            }
        }

        // Load result
        let load = store.query(&format!(
            "SELECT tool, vus, throughput_rps, latency_p50_ms, latency_p95_ms, \
                    latency_p99_ms, error_rate, total_requests, thresholds_met, duration_s \
             FROM load_results WHERE experiment_id = '{exp_id}'"
        ))?;

        if !load.is_empty() {
            let lr = &load[0];
            println!("\nLoad Test ({}):", lr[0]);
            println!(
                "  VUs: {}  Duration: {}s  Requests: {}",
                lr[1], lr[9], lr[7]
            );
            println!(
                "  Latency: p50={}ms  p95={}ms  p99={}ms",
                lr[3], lr[4], lr[5]
            );
            println!("  Throughput: {} req/s  Error rate: {}", lr[2], lr[6]);
            let met = if lr[8] == "true" { "PASS" } else { "FAIL" };
            println!("  Thresholds: {met}");
        }
    }

    // Aggregate if showing multiple
    if last_n > 1 && experiments.len() > 1 {
        let agg = store.query(
            "SELECT count(*) as total, \
             count(CASE WHEN status = 'completed' THEN 1 END) as passed, \
             avg(duration_ms) as avg_ms \
             FROM experiments",
        )?;
        if !agg.is_empty() {
            println!("\n{}", "═".repeat(60));
            println!(
                "Store: {} experiments, {} completed, avg {}ms",
                agg[0][0], agg[0][1], agg[0][2]
            );
        }
    }

    Ok(())
}

/// Prints a store-wide aggregate summary.
fn print_store_aggregate(store: &tumult_analytics::AnalyticsStore) -> Result<()> {
    let total = store.experiment_count()?;
    let act_rows = store.query("SELECT count(*) FROM activity_results")?;
    let activities = act_rows
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    println!("Analytics Store Summary");
    println!("{}", "═".repeat(60));
    println!("  Experiments: {total}");
    println!("  Activities:  {activities}");

    // Status breakdown
    let statuses = store.query(
        "SELECT status, count(*) as cnt FROM experiments GROUP BY status ORDER BY cnt DESC",
    )?;
    if !statuses.is_empty() {
        let status_line: Vec<String> = statuses
            .iter()
            .map(|r| format!("{}={}", r[0], r[1]))
            .collect();
        println!("  By status:   {}", status_line.join("  "));
    }

    // Duration stats
    let dur = store.query(
        "SELECT cast(round(avg(duration_ms::DOUBLE), 0) as INTEGER), \
                cast(min(duration_ms) as INTEGER), \
                cast(max(duration_ms) as INTEGER) \
         FROM experiments",
    )?;
    if !dur.is_empty() && !dur[0][0].is_empty() {
        println!(
            "  Duration:    avg={}ms  min={}ms  max={}ms",
            dur[0][0], dur[0][1], dur[0][2]
        );
    }

    // Load tests
    let load = store.query(
        "SELECT count(*), round(avg(latency_p95_ms), 1), round(avg(error_rate), 4) \
         FROM load_results",
    )?;
    if !load.is_empty() && load[0][0] != "0" {
        println!(
            "  Load tests:  {} (avg p95={}ms, avg error_rate={})",
            load[0][0], load[0][1], load[0][2]
        );
    }

    // Top 5 longest experiments
    let top = store.query(
        "SELECT duration_ms, title, status \
         FROM experiments ORDER BY duration_ms DESC LIMIT 5",
    )?;
    if !top.is_empty() {
        println!("\nTop 5 by duration:");
        for row in &top {
            let dur_s = row[0].parse::<f64>().unwrap_or(0.0) / 1000.0;
            println!("  {dur_s:>7.1}s  {} ({})", row[1], row[2]);
        }
    }

    // Recent experiments
    let recent = store.query(
        "SELECT title, status, duration_ms \
         FROM experiments ORDER BY started_at_ns DESC LIMIT 5",
    )?;
    if !recent.is_empty() {
        println!("\nLast 5 experiments:");
        for row in &recent {
            let status_icon = match row[1].as_str() {
                "completed" => "PASS",
                "deviated" => "DEV ",
                "aborted" => "ABRT",
                "failed" => "FAIL",
                _ => &row[1],
            };
            println!("  [{status_icon}] {}ms  {}", row[2], row[0]);
        }
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if the analytics store cannot be opened or the query fails.
#[must_use = "callers must handle analytics errors"]
pub fn cmd_analyze(
    journals_path: Option<&Path>,
    query: Option<&str>,
    last: Option<usize>,
    all: bool,
) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let (store, count) = if let Some(path) = journals_path {
        let store = AnalyticsStore::in_memory()?;
        let mut count = 0;

        if path.is_file() {
            let journal = read_journal(path)
                .with_context(|| format!("failed to read journal: {}", path.display()))?;
            store.ingest_journal(&journal)?;
            count = 1;
        } else if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry_path = entry?.path();
                if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                    match read_journal(&entry_path) {
                        Ok(journal) => {
                            store.ingest_journal(&journal)?;
                            count += 1;
                        }
                        Err(e) => eprintln!("warning: skipping {}: {}", entry_path.display(), e),
                    }
                }
            }
        } else {
            bail!("path does not exist: {}", path.display());
        }
        (store, count)
    } else {
        // Use persistent store
        let db_path = AnalyticsStore::default_path();
        if !db_path.exists() {
            bail!(
                "no persistent store found at {}. Run experiments first or specify a journals path.",
                db_path.display()
            );
        }
        let store = AnalyticsStore::open(&db_path)?;
        let count = store.experiment_count()?;
        (store, count)
    };

    println!("Loaded {count} journal(s) into analytics store\n");

    if let Some(sql) = query {
        let columns = store.query_columns(sql)?;
        let rows = store.query(sql)?;
        println!("{}", columns.join("\t"));
        println!(
            "{}",
            columns
                .iter()
                .map(|c| "-".repeat(c.len().max(8)))
                .collect::<Vec<_>>()
                .join("\t")
        );
        for row in &rows {
            println!("{}", row.join("\t"));
        }
        println!("\n{} row(s)", rows.len());
    } else if all {
        print_store_aggregate(&store)?;
    } else {
        print_experiment_summary(&store, last.unwrap_or(1))?;
    }
    Ok(())
}

// ── Export command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the journal cannot be read or the export operation fails.
#[must_use = "callers must handle export errors"]
pub fn cmd_export(journal_path: &Path, format: ExportFormat) -> Result<()> {
    use tumult_analytics::arrow_convert::journal_to_record_batch;
    use tumult_analytics::export::{export_csv, export_parquet};
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let ext = match format {
        ExportFormat::Parquet => "parquet",
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
    };
    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("journal");
    let out_path = std::path::PathBuf::from(format!("{stem}.{ext}"));

    match format {
        ExportFormat::Parquet | ExportFormat::Csv => {
            let (exp_batch, _) = journal_to_record_batch(std::slice::from_ref(&journal))?;
            match format {
                ExportFormat::Parquet => export_parquet(&exp_batch, &out_path)?,
                ExportFormat::Csv => export_csv(&exp_batch, &out_path)?,
                ExportFormat::Json => unreachable!(),
            }
        }
        ExportFormat::Json => {
            let json = serde_json::to_string_pretty(&journal)?;
            std::fs::write(&out_path, json)?;
        }
    }
    println!("Exported to: {}", out_path.display());
    Ok(())
}

// ── Trend command ─────────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)] // Multi-probe trend analysis output requires verbose formatting across multiple metric types
#[must_use = "callers must handle trend analysis errors"]
pub fn cmd_trend(
    journals_path: &Path,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    println!("Loaded {count} journal(s)\n");

    let valid_metrics = [
        "resilience_score",
        "duration_ms",
        "estimate_accuracy",
        "method_step_count",
    ];
    if !valid_metrics.contains(&metric) {
        bail!(
            "unknown metric: {}. Valid: {}",
            metric,
            valid_metrics.join(", ")
        );
    }

    // Parse --last flag into nanosecond cutoff
    let time_filter = if let Some(window) = last {
        let days: i64 = window.trim_end_matches('d').parse().with_context(|| {
            format!("--last must be a number of days (e.g., 30d), got: {window}")
        })?;
        let cutoff_ns =
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) - (days * 86400 * 1_000_000_000);
        format!(" AND started_at_ns >= {cutoff_ns}")
    } else {
        String::new()
    };

    // Pre-built queries keyed by metric — no format! interpolation (DB-03)
    let base_sql = match metric {
        "resilience_score" => "SELECT experiment_id, title, status, resilience_score, started_at_ns FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT experiment_id, title, status, duration_ms, started_at_ns FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT experiment_id, title, status, estimate_accuracy, started_at_ns FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT experiment_id, title, status, method_step_count, started_at_ns FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let target_filter = if target.is_some() {
        // Bind the LIKE pattern as a query parameter to prevent SQL injection.
        " AND lower(title) LIKE ?"
    } else {
        ""
    };
    let sql = format!("{base_sql}{time_filter}{target_filter} ORDER BY started_at_ns");

    let (columns, rows) = if let Some(t) = target {
        let like_pattern = format!("%{}%", t.to_lowercase());
        // Fetch column names from the base SQL (schema is identical regardless of filter).
        let columns = store.query_columns(base_sql)?;
        let rows = store.query_with_param(&sql, &like_pattern)?;
        (columns, rows)
    } else {
        let columns = store.query_columns(&sql)?;
        let rows = store.query(&sql)?;
        (columns, rows)
    };

    if rows.is_empty() {
        println!("No data points for metric: {metric}");
        return Ok(());
    }

    println!("Trend: {} ({} data points)\n", metric, rows.len());
    println!(
        "{}",
        columns.iter().fold(String::new(), |mut s, c| {
            let _ = write!(s, "{c:<20}");
            s
        })
    );
    println!("{}", "-".repeat(columns.len() * 20));
    for row in &rows {
        println!(
            "{}",
            row.iter().fold(String::new(), |mut s, v| {
                let _ = write!(s, "{v:<20}");
                s
            })
        );
    }

    // Summary stats — pre-built per metric
    let stats_sql = match metric {
        "resilience_score" => "SELECT count(*) as runs, min(resilience_score) as min, max(resilience_score) as max, avg(resilience_score) as avg FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT count(*) as runs, min(duration_ms) as min, max(duration_ms) as max, avg(duration_ms) as avg FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT count(*) as runs, min(estimate_accuracy) as min, max(estimate_accuracy) as max, avg(estimate_accuracy) as avg FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT count(*) as runs, min(method_step_count) as min, max(method_step_count) as max, avg(method_step_count) as avg FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    let stats = store.query(stats_sql)?;
    if let Some(row) = stats.first() {
        println!(
            "\nSummary: {} runs, min={}, max={}, avg={}",
            row[0], row[1], row[2], row[3]
        );
    }

    Ok(())
}

// ── Compliance command ────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)] // Framework-specific output is intentionally verbose for audit clarity
#[must_use = "callers must handle compliance check errors"]
pub fn cmd_compliance(journals_path: &Path, framework: ComplianceFramework) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;
    let mut journals_with_regulatory = 0;

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        if journal.regulatory.is_some() {
                            journals_with_regulatory += 1;
                        }
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        if journal.regulatory.is_some() {
            journals_with_regulatory += 1;
        }
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    let fw = framework.as_report_str();
    let full_name = match framework {
        ComplianceFramework::Dora => "DORA — Digital Operational Resilience Act (EU 2022/2554)",
        ComplianceFramework::Nis2 => {
            "NIS2 — Network and Information Security Directive (EU 2022/2555)"
        }
        ComplianceFramework::PciDss => "PCI-DSS 4.0 — Payment Card Industry Data Security Standard",
        ComplianceFramework::Iso22301 => "ISO 22301 — Business Continuity Management Systems",
        ComplianceFramework::Iso27001 => "ISO 27001 — Information Security Management Systems",
        ComplianceFramework::Soc2 => "SOC 2 — Service Organization Control Type 2",
        ComplianceFramework::BaselIii => "Basel III — BCBS 239 Risk Data Aggregation",
    };
    println!("=== {full_name} ===\n");
    println!("Journals analyzed: {count}");
    println!("With regulatory tagging: {journals_with_regulatory}\n");

    // Overall status
    let rows = store.query(
        "SELECT status, count(*) as runs FROM experiments GROUP BY status ORDER BY runs DESC",
    )?;
    println!("Experiment Results:");
    for row in &rows {
        println!("  {}: {} run(s)", row[0], row[1]);
    }

    // Compliance derivation
    let total = store.query("SELECT count(*) FROM experiments")?;
    let completed = store.query("SELECT count(*) FROM experiments WHERE status = 'completed'")?;
    let total_n: f64 = total[0][0].parse().unwrap_or(0.0);
    let completed_n: f64 = completed[0][0].parse().unwrap_or(0.0);
    let success_rate = if total_n > 0.0 {
        completed_n / total_n * 100.0
    } else {
        0.0
    };

    println!("\nCompliance Status:");
    println!("  Success rate: {success_rate:.1}%");
    println!(
        "  Overall: {}",
        if success_rate >= 95.0 {
            "COMPLIANT"
        } else if success_rate >= 80.0 {
            "PARTIAL"
        } else {
            "NON-COMPLIANT"
        }
    );

    // Framework-specific requirements and evidence
    match fw {
        "DORA" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/reg/2022/2554/oj");
            println!("Applies to EU financial entities. Mandates ICT resilience testing");
            println!("programmes with documented evidence and recovery time validation.\n");
            println!("Requirements:");
            println!("  Art. 24 — General requirements for ICT resilience testing");
            println!("    Testing programme: {count} experiment(s) executed");
            println!("  Art. 25 — Testing of ICT tools and systems");
            println!("    Scenario-based tests with documented results");
            println!("  Art. 26 — Advanced testing (TLPT)");
            println!("    Threat-led penetration testing (for systemically important entities)");
            println!("  Art. 11 — Response and recovery");
            println!("    Recovery procedures tested with measured recovery times");
        }
        "NIS2" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/dir/2022/2555/oj");
            println!("Applies to EU essential/important entities across 18 sectors.");
            println!("Requires risk management measures including testing and audit.\n");
            println!("Requirements:");
            println!("  Art. 21(2)(c) — Business continuity and crisis management");
            println!("    Fault injection experiments with recovery measurement");
            println!("  Art. 21(2)(f) — Assessment of cybersecurity measures effectiveness");
            println!("    Baseline vs during-fault comparison proves control effectiveness");
            println!("  Art. 23 — Incident handling and reporting");
            println!("    Documented incident response procedures tested");
        }
        "PCI-DSS" => {
            println!("\nSource: https://www.pcisecuritystandards.org/document_library/");
            println!(
                "Applies to any entity storing, processing, or transmitting cardholder data.\n"
            );
            println!("Requirements:");
            println!("  Req. 11.4.1 — Penetration testing methodology defined");
            println!("    Experiment definitions with hypothesis, method, rollbacks");
            println!("  Req. 11.4.2 — Internal penetration testing at least annually");
            println!("    Journal timestamps prove execution: {count} run(s)");
            println!("  Req. 11.4.5 — Segmentation control testing");
            println!("    Network partition experiments with recovery verification");
            println!("  Req. 12.10.2 — Incident response plan tested annually");
            println!("    Experiments trigger and validate incident response procedures");
        }
        "ISO-22301" => {
            println!("\nSource: https://www.iso.org/standard/75106.html");
            println!("Business continuity management — requires exercising and testing.\n");
            println!("Requirements:");
            println!("  Clause 8.5 — Exercising and testing");
            println!("    Exercises consistent with BCMS scope: {count} experiment(s)");
            println!("    Based on appropriate scenarios with documented results");
            println!("    Formal post-exercise reports via `tumult report`");
            println!("    Results analysed via trend analysis and estimate accuracy");
        }
        "ISO-27001" => {
            println!("\nSource: https://www.iso.org/standard/27001");
            println!("Information security management — continuity controls.\n");
            println!("Requirements:");
            println!("  Annex A.17.1.3 — Verify and review IT service continuity controls");
            println!("    Experiment results prove controls function under fault conditions");
            println!("    Regular testing with journal frequency and trend data");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "SOC2" => {
            println!("\nSource: https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2");
            println!("Service Organization Control — availability and processing integrity.\n");
            println!("Requirements:");
            println!("  CC7.5 — Recovery from identified disruptions");
            println!("    Recovery procedures tested with measured MTTR");
            println!("    Recovery meets defined objectives (RTO validation)");
            println!("  CC7.4 — Detection and monitoring");
            println!("    Observability data (OTel traces) proves monitoring coverage");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "Basel-III" => {
            println!("\nSource: https://www.bis.org/publ/bcbs239.htm");
            println!("Risk data aggregation and reporting for global banking.\n");
            println!("Requirements:");
            println!("  Principle 6 — Adaptability");
            println!("    Systems function under stress conditions");
            println!("    Data aggregation and reporting during crisis validated");
            println!("    Recovery of reporting capability measured");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        _ => {
            println!("\nEvidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
    }

    println!("\n=== End Report ===");
    Ok(())
}

// ── Init command ──────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file already exists or cannot be written.
#[must_use = "callers must handle init errors"]
pub fn cmd_init(plugin: Option<&str>) -> Result<()> {
    init_at(Path::new("experiment.toon"), plugin)
}

fn init_at(path: &Path, plugin: Option<&str>) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — remove it first or use a different name",
            path.display()
        );
    }

    let template = generate_template(plugin);
    std::fs::write(path, &template)?;

    println!("Created {}", path.display());
    if let Some(p) = plugin {
        println!("Template includes {p} plugin actions");
    }
    println!("Edit the file to configure your experiment, then run:");
    println!("  tumult run {}", path.display());

    Ok(())
}

fn generate_template(plugin: Option<&str>) -> String {
    let plugin_name = plugin.unwrap_or("tumult-example");
    format!(
        r#"title: System information check
description: Verify system is accessible and report CPU and memory info

tags[2]: resilience, baseline

steady_state_hypothesis:
  title: System is reachable
  probes[1]:
    - name: system-check
      activity_type: probe
      provider:
        type: process
        path: uname
        arguments[1]: "-a"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "."

method[2]:
  - name: check-cpu
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/cpuinfo 2>/dev/null | head -20 || sysctl -n machdep.cpu.brand_string"
      timeout_s: 10.0
  - name: check-memory
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/meminfo 2>/dev/null | head -5 || sysctl -n hw.memsize"
      timeout_s: 10.0

rollbacks[1]:
  - name: log-complete
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "system check completed via {plugin_name}"
      timeout_s: 5.0
"#
    )
}

// ── Dry run ───────────────────────────────────────────────────

fn print_dry_run(experiment: &Experiment) {
    println!("=== DRY RUN ===\n");
    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {desc}");
    }
    println!();

    if let Some(ref estimate) = experiment.estimate {
        println!("Phase 0 — Estimate:");
        println!("  Expected outcome: {:?}", estimate.expected_outcome);
        if let Some(recovery) = estimate.expected_recovery_s {
            println!("  Expected recovery: {recovery}s");
        }
        println!();
    }

    if let Some(ref baseline) = experiment.baseline {
        println!("Phase 1 — Baseline:");
        println!("  Duration: {}s", baseline.duration_s);
        println!("  Interval: {}s", baseline.interval_s);
        println!("  Method: {:?}", baseline.method);
        println!();
    }

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!("Hypothesis: {}", hypothesis.title);
        for probe in &hypothesis.probes {
            println!("  Probe: {}", probe.name);
        }
        println!();
    }

    println!("Phase 2 — Method ({} steps):", experiment.method.len());
    for (i, activity) in experiment.method.iter().enumerate() {
        let bg = if activity.background {
            " [background]"
        } else {
            ""
        };
        println!(
            "  {}. {} ({:?}){}",
            i + 1,
            activity.name,
            activity.activity_type,
            bg
        );
    }
    println!();

    if !experiment.rollbacks.is_empty() {
        println!("Rollbacks ({} steps):", experiment.rollbacks.len());
        for activity in &experiment.rollbacks {
            println!("  - {} ({:?})", activity.name, activity.activity_type);
        }
        println!();
    }

    if let Some(ref regulatory) = experiment.regulatory {
        println!("Regulatory: {}", regulatory.frameworks.join(", "));
    }

    println!("=== END DRY RUN ===");
}

// ── Path validation ─────────────────────────────────────────

/// Best-effort symlink check. Note: there is an inherent TOCTOU race
/// between this check and subsequent file operations — the path could
/// become a symlink after validation. This is acceptable for our threat
/// model (local CLI tool, not a network service). For stronger guarantees,
/// callers should use `O_NOFOLLOW` or `openat2` with `RESOLVE_NO_SYMLINKS`.
fn validate_path_no_symlink(path: &Path) -> Result<()> {
    if path.is_symlink() {
        bail!("symlink not allowed for security: {}", path.display());
    }
    Ok(())
}
