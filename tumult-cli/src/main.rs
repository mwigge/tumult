use tumult_cli::commands;
use tumult_cli::commands::{build_load_override, parse_var_args};

use clap::Parser;

use cli::{
    AgenticAction, AutopilotAction, ChaosGraphAction, Cli, Commands, GameDayAction, GraphFormat,
    McpAction, McpTransport, OutputFormat, RollbackStrategy, StoreAction, TopologyAction,
};

mod cli;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    // Keep interactive runs quiet: without an OTLP endpoint there is nowhere for
    // spans to go, so the INFO-level tracing/telemetry lines (`experiment.started`,
    // `Global tracer provider is set`, …) are pure noise on the terminal. Default
    // the log level to `warn` in that case. When an endpoint IS configured, or the
    // operator sets `RUST_LOG` explicitly, we leave the level untouched so audit
    // logs and span export behave exactly as before.
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
        && std::env::var_os("RUST_LOG").is_none()
    {
        std::env::set_var("RUST_LOG", "warn");
    }

    // Initialize OpenTelemetry from environment
    let otel_config = tumult_otel::config::TelemetryConfig::from_env();
    let telemetry = tumult_otel::telemetry::TumultTelemetry::new(otel_config);

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            experiment,
            journal_path,
            dry_run,
            rollback_strategy,
            baseline_mode: _,
            no_ingest,
            output_format,
            vars,
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
            let load_override = build_load_override(load, load_script, load_vus, load_duration);
            commands::cmd_run(
                &experiment,
                &journal_path,
                dry_run,
                strategy,
                !no_ingest,
                var_map,
                load_override,
            )
            .await?;
            // If --output-format json was specified, print the journal as JSON to stdout
            if matches!(output_format, Some(OutputFormat::Json)) {
                if let Ok(content) = std::fs::read_to_string(&journal_path) {
                    if let Ok(journal) =
                        toon_format::decode_default::<tumult_core::types::Journal>(&content)
                    {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&journal)
                                .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")),
                        );
                    }
                }
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
            StoreAction::Purge { older_than_days } => commands::cmd_store_purge(older_than_days)?,
            StoreAction::Path => commands::cmd_store_path()?,
            StoreAction::Migrate => commands::cmd_store_migrate().await?,
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
                store_path: store_path
                    .unwrap_or_else(tumult_analytics::AnalyticsStore::default_path),
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
            println!(
                "{}",
                commands::cmd_recommend(&options, agent_args.as_ref())?
            );
        }
        Commands::Agents => {
            print!("{}", commands::cmd_agents());
        }
        Commands::Agentic { action } => match action {
            AgenticAction::ListPacks => {
                print!("{}", commands::cmd_agentic_list_scenario_packs()?);
            }
            AgenticAction::Smoke { journal } => {
                print!("{}", commands::cmd_agentic_smoke(&journal)?);
            }
            AgenticAction::Run { scenario, journal } => {
                print!(
                    "{}",
                    commands::cmd_agentic_run_scenario(&scenario, &journal)?
                );
            }
            AgenticAction::Trajectory { pack, journal } => {
                print!("{}", commands::cmd_agentic_trajectory(&pack, &journal)?);
            }
            AgenticAction::Replay { fixture, journal } => {
                print!("{}", commands::cmd_agentic_replay(&fixture, &journal)?);
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
                print!(
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
                    matches!(format, GraphFormat::Json),
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
                    matches!(format, GraphFormat::Json),
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
            AutopilotAction::Approve { id, store } => {
                commands::cmd_autopilot_respond(store.as_deref(), &id, true, None)?;
            }
            AutopilotAction::Deny { id, reason, store } => {
                commands::cmd_autopilot_respond(store.as_deref(), &id, false, reason.as_deref())?;
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
        Commands::Tui {
            store,
            refresh_secs,
        } => {
            // The TUI drives crossterm on a blocking terminal loop; keep it off
            // the async reactor so the runtime stays responsive.
            tokio::task::spawn_blocking(move || tumult_tui::run(store, refresh_secs)).await??;
        }
    }

    // Flush OTel spans before exit
    telemetry.shutdown();

    Ok(())
}
