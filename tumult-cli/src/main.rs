use std::path::Path;
use std::process::ExitCode;

use tumult_cli::commands;
use tumult_cli::commands::{build_load_override, parse_var_args};

use anyhow::Context;
use clap::Parser;
use tumult_otel::logging::{self, LogOptions};

use cli::{
    AgenticAction, AutopilotAction, ChaosGraphAction, Cli, Commands, GameDayAction, GateFailOn,
    GraphFormat, McpAction, McpTransport, OutputFormat, PerfFailOnArg, RollbackStrategy,
    StoreAction, TopologyAction,
};

mod cli;

/// Drop guard that flushes OpenTelemetry spans on every exit path from
/// `main` — including `?` early returns — so telemetry is not lost exactly
/// on the failed runs where it matters most. `TumultTelemetry::shutdown`
/// takes `&self`, so the guard can call it directly from `Drop`.
struct TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry);

impl Drop for TelemetryShutdown {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Diagnostics are structured events on stderr; stdout carries command
    // output only. Interactive runs stay quiet: without an OTLP endpoint the
    // level defaults to `warn`, with one it is `info`; `RUST_LOG` overrides
    // either.
    let otel_config = tumult_otel::config::TelemetryConfig::from_env();
    if let Err(error) = logging::init(LogOptions::for_tool(&otel_config)) {
        logging::report_init_failure(&otel_config.service_name, &error);
        return ExitCode::FAILURE;
    }

    // Initialize OpenTelemetry from environment. The guard flushes spans and
    // logs on every exit path below, including the fatal event.
    let _telemetry_guard =
        TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry::new(otel_config));

    match dispatch(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logging::log_fatal(&error);
            exit_code_for(&error)
        }
    }
}

/// The exit code of a failed command: that of its verdict for a gate that
/// did not pass, the general failure otherwise.
fn exit_code_for(error: &anyhow::Error) -> ExitCode {
    commands::exit_code_of(error)
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
}

/// Prints the journal at `path` to stdout as pretty JSON
/// (`run --output-format json`).
fn print_journal_json(path: &Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read journal: {}", path.display()))?;
    let journal: tumult_core::types::Journal = toon_format::decode_default(&content)
        .with_context(|| format!("failed to decode journal: {}", path.display()))?;
    let json = serde_json::to_string_pretty(&journal)
        .with_context(|| format!("failed to encode journal as JSON: {}", path.display()))?;
    tumult_cli::outln!("{json}");
    Ok(())
}

/// Dispatch a parsed command to its handler. Split from `main` so the wiring
/// (argument mapping, enum conversions, output shaping) is exercisable from
/// tests without spawning the binary.
#[allow(clippy::too_many_lines)]
async fn dispatch(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Run {
            experiment,
            journal_path,
            force,
            dry_run,
            rollback_strategy,
            baseline_mode,
            no_ingest,
            output_format,
            vars,
            target_version,
            load,
            load_script,
            load_vus,
            load_duration,
        } => {
            let strategy = match rollback_strategy {
                RollbackStrategy::Always => tumult_core::execution::RollbackStrategy::Always,
                RollbackStrategy::OnDeviation => {
                    tumult_core::execution::RollbackStrategy::OnDeviation
                }
                RollbackStrategy::Never => tumult_core::execution::RollbackStrategy::Never,
            };
            let var_map = parse_var_args(&vars)?;
            let load_override = build_load_override(load, load_script, load_vus, load_duration)?;
            commands::cmd_run_with_baseline_mode(
                &experiment,
                &journal_path,
                force,
                dry_run,
                strategy,
                !no_ingest,
                var_map,
                load_override,
                match baseline_mode {
                    cli::BaselineMode::Full => tumult_core::runner::BaselineMode::Full,
                    cli::BaselineMode::Skip => tumult_core::runner::BaselineMode::Skip,
                    cli::BaselineMode::Only => tumult_core::runner::BaselineMode::Only,
                },
                target_version.as_deref(),
            )
            .await?;
            // If --output-format json was specified, print the journal as JSON to stdout
            if matches!(output_format, Some(OutputFormat::Json)) {
                print_journal_json(&journal_path)?;
            }
        }
        Commands::Validate { experiment } => {
            commands::cmd_validate(&experiment)?;
        }
        Commands::Discover { plugin } => {
            commands::cmd_discover(plugin.as_deref())?;
        }
        Commands::Init { plugin } => {
            commands::cmd_init(plugin.as_deref())?;
        }
        Commands::New { from, set, out } => {
            commands::cmd_new(from.as_deref(), &set, out.as_deref())?;
        }
        Commands::Templates => {
            commands::cmd_templates()?;
        }
        Commands::Analyze {
            journals,
            query,
            last,
            all,
        } => {
            commands::cmd_analyze(journals.as_deref(), query.as_deref(), last, all)?;
        }
        Commands::Export { journal, format } => {
            commands::cmd_export(&journal, format)?;
        }
        Commands::Compliance {
            journals,
            framework,
            sources,
        } => {
            commands::cmd_compliance(journals.as_deref(), framework, sources)?;
        }
        Commands::Trend {
            journals,
            metric,
            last,
            target,
        } => {
            commands::cmd_trend(&journals, &metric, last.as_deref(), target.as_deref())?;
        }
        Commands::Report {
            journal,
            output,
            format,
            trace_ui_base,
        } => {
            commands::cmd_report(
                &journal,
                output.as_deref(),
                format,
                trace_ui_base.as_deref(),
            )?;
        }
        Commands::Import { parquet_dir } => {
            commands::cmd_import(&parquet_dir)?;
        }
        Commands::Store { action } => match action {
            StoreAction::Stats => commands::cmd_store_stats()?,
            StoreAction::Backup { output } => commands::cmd_store_backup(&output)?,
            StoreAction::Export { output } => commands::cmd_store_export(&output)?,
            StoreAction::Restore { input, output } => commands::cmd_store_restore(&input, &output)?,
            StoreAction::Purge { older_than_days } => commands::cmd_store_purge(older_than_days)?,
            StoreAction::Path => commands::cmd_store_path()?,
            StoreAction::Migrate => commands::cmd_store_migrate().await?,
            StoreAction::ImportLegacy {
                analytics_db,
                kronika_db,
                store,
            } => commands::cmd_store_import_legacy(
                analytics_db.as_deref(),
                kronika_db.as_deref(),
                store.as_deref(),
            )?,
        },
        Commands::Recommend {
            goal,
            store_path,
            model,
            no_draft,
            format,
            agent,
            agent_model,
            agent_timeout,
            generate_experiments,
        } => {
            let options = tumult_intelligence::RecommendOptions {
                store_path: match store_path {
                    Some(path) => path,
                    None => tumult_lake::AnalyticsStore::default_path()
                        .context("failed to determine analytics store path")?,
                },
                goal,
                model,
                include_draft: !no_draft,
                format: format.into(),
            };
            let agent_args = agent.map(|agent| commands::AgentArgs {
                agent,
                model: agent_model,
                timeout_secs: agent_timeout,
                generate_dir: generate_experiments,
            });
            tumult_cli::outln!(
                "{}",
                commands::cmd_recommend(&options, agent_args.as_ref())?
            );
        }
        Commands::Agents => {
            tumult_cli::out!("{}", commands::cmd_agents());
        }
        Commands::Agentic { action } => match action {
            AgenticAction::ListPacks => {
                tumult_cli::out!("{}", commands::cmd_agentic_list_scenario_packs()?);
            }
            AgenticAction::Smoke { journal } => {
                tumult_cli::out!("{}", commands::cmd_agentic_smoke(&journal)?);
            }
            AgenticAction::Run { scenario, journal } => {
                tumult_cli::out!(
                    "{}",
                    commands::cmd_agentic_run_scenario(&scenario, &journal)?
                );
            }
            AgenticAction::Trajectory { pack, journal } => {
                tumult_cli::out!("{}", commands::cmd_agentic_trajectory(&pack, &journal)?);
            }
            AgenticAction::Replay { fixture, journal } => {
                tumult_cli::out!("{}", commands::cmd_agentic_replay(&fixture, &journal)?);
            }
            AgenticAction::Proxy {
                listen,
                upstream,
                scenario,
                journal,
                seed,
                client,
            } => {
                commands::cmd_agentic_proxy(
                    &listen,
                    &upstream,
                    &scenario,
                    journal.as_deref(),
                    seed,
                    &client,
                )
                .await?;
            }
            AgenticAction::RunLive {
                prompt,
                scenario,
                base_url,
                otlp,
                client,
            } => {
                tumult_cli::out!(
                    "{}",
                    commands::cmd_agentic_run_live(
                        &prompt,
                        &scenario,
                        &base_url,
                        otlp.as_deref(),
                        &client,
                    )?
                );
            }
        },
        Commands::GameDay { action } => match action {
            GameDayAction::Create {
                name,
                experiments,
                load,
                load_script,
                load_vus,
                framework,
            } => {
                commands::cmd_gameday_create(
                    &name,
                    &experiments,
                    load,
                    load_script.as_deref(),
                    load_vus,
                    framework,
                )?;
            }
            GameDayAction::Run { gameday } => {
                commands::cmd_gameday_run(&gameday)?;
            }
            GameDayAction::Analyze { gameday } => {
                commands::cmd_gameday_analyze(&gameday)?;
            }
        },
        Commands::Mcp { action } => match action {
            McpAction::Serve {
                transport,
                host,
                port,
                health_port,
                token,
                auth_config,
            } => {
                let transport = match transport {
                    McpTransport::Stdio => commands::McpTransportKind::Stdio,
                    McpTransport::Http => commands::McpTransportKind::Http,
                };
                commands::cmd_mcp_serve(transport, host, port, health_port, token, auth_config)
                    .await?;
            }
        },
        Commands::ChaosGraph { action } => match action {
            ChaosGraphAction::Query {
                kind,
                filter,
                format,
                store,
            } => {
                commands::cmd_chaosgraph_query(
                    store.as_deref(),
                    &kind,
                    filter.as_deref(),
                    matches!(format, GraphFormat::Json),
                )?;
            }
            ChaosGraphAction::Neighbors {
                node,
                rel,
                depth,
                format,
                store,
            } => {
                commands::cmd_chaosgraph_neighbors(
                    store.as_deref(),
                    &node,
                    rel.as_deref(),
                    depth,
                    matches!(format, GraphFormat::Json),
                )?;
            }
            ChaosGraphAction::CoverageGaps {
                framework,
                domain,
                refresh,
                format,
                store,
            } => {
                commands::cmd_chaosgraph_coverage_gaps(
                    store.as_deref(),
                    framework.as_deref(),
                    domain.as_deref(),
                    refresh,
                    matches!(format, GraphFormat::Json),
                )?;
            }
        },
        Commands::Topology { action } => match action {
            TopologyAction::Import { path, store } => {
                commands::cmd_topology_import(store.as_deref(), &path, false)?;
            }
            TopologyAction::DiscoverK8s { namespace, output } => {
                commands::cmd_topology_discover_k8s(&namespace, output.as_deref()).await?;
            }
            TopologyAction::Map {
                framework,
                control,
                format,
                no_recommend,
                limit,
                store,
            } => {
                commands::cmd_topology_map(
                    store.as_deref(),
                    framework.as_deref(),
                    control.as_deref(),
                    format.as_str(),
                    !no_recommend,
                    limit,
                )?;
            }
            TopologyAction::Lineage {
                framework,
                control,
                service,
                format,
                store,
            } => {
                commands::cmd_topology_lineage(
                    store.as_deref(),
                    framework.as_deref(),
                    control.as_deref(),
                    service.as_deref(),
                    format.as_str(),
                )?;
            }
            TopologyAction::Recommend {
                framework,
                limit,
                format,
                store,
            } => {
                commands::cmd_topology_recommend(
                    store.as_deref(),
                    framework.as_deref(),
                    limit,
                    format.as_str(),
                )?;
            }
        },
        Commands::Autopilot { action } => match action {
            AutopilotAction::Once {
                policy,
                execute,
                limit,
                store,
            } => {
                commands::cmd_autopilot_once(store.as_deref(), &policy, execute, limit)?;
            }
            AutopilotAction::Status {
                verdict,
                limit,
                format,
                store,
            } => {
                commands::cmd_autopilot_status(
                    store.as_deref(),
                    verdict.as_deref(),
                    limit,
                    matches!(format, GraphFormat::Json),
                )?;
            }
            AutopilotAction::Approve { id, policy, store } => {
                commands::cmd_autopilot_respond(store.as_deref(), &id, true, None, Some(&policy))?;
            }
            AutopilotAction::Deny { id, reason, store } => {
                commands::cmd_autopilot_respond(
                    store.as_deref(),
                    &id,
                    false,
                    reason.as_deref(),
                    None,
                )?;
            }
            AutopilotAction::NotifyChange {
                service,
                source,
                detail,
                store,
            } => {
                commands::cmd_autopilot_notify_change(
                    store.as_deref(),
                    &service,
                    &source,
                    detail.as_deref(),
                )?;
            }
            AutopilotAction::Export { dir, store } => {
                commands::cmd_autopilot_export(store.as_deref(), &dir)?;
            }
        },
        Commands::Gate {
            target,
            min_score,
            max_age_days,
            env,
            daemon_url,
            fail_on,
            json,
        } => {
            let fail_on = match fail_on {
                GateFailOn::Fail => commands::FailOn::Fail,
                GateFailOn::Warn => commands::FailOn::Warn,
                GateFailOn::Never => commands::FailOn::Never,
            };
            let ok = commands::cmd_gate(&commands::GateArgs {
                target: target.clone(),
                min_score,
                max_age_days,
                env,
                daemon_url,
                fail_on,
                json,
            })
            .await?;
            if !ok {
                anyhow::bail!("gate did not pass for target {target}");
            }
        }
        Commands::Perf {
            target,
            probe,
            quantile,
            min_samples,
            min_runs,
            seed,
            margin_pct,
            env,
            baseline,
            candidate,
            range,
            include,
            store,
            daemon_url,
            fail_on,
            json,
        } => {
            let listed: Vec<_> = fail_on
                .iter()
                .map(|one| match one {
                    PerfFailOnArg::Regressed => commands::PerfFailOn::Regressed,
                    PerfFailOnArg::Inconclusive => commands::PerfFailOn::Inconclusive,
                    PerfFailOnArg::Never => commands::PerfFailOn::Never,
                })
                .collect();
            let fail_on = commands::PerfFailOn::of(&listed).map_err(|e| anyhow::anyhow!(e))?;
            commands::cmd_perf(&commands::PerfArgs {
                target,
                probe,
                quantile: quantile.map(|q| q.as_str().to_string()),
                min_samples,
                min_runs,
                seed,
                margin_pct,
                env,
                baseline,
                candidate,
                range,
                include_partial: include.is_some(),
                store,
                daemon_url,
                fail_on,
                json,
            })
            .await?;
        }
        Commands::Tui {
            store,
            refresh_secs,
        } => {
            // The TUI drives crossterm on a blocking terminal loop; keep it off
            // the async reactor so the runtime stays responsive.
            tokio::task::spawn_blocking(move || tumult_tui::run(store, refresh_secs)).await??;
        }
    }

    Ok(())
}

#[cfg(test)]
mod dispatch_tests;
