use tumult_cli::commands;
use tumult_cli::commands::{build_load_override, parse_var_args};

use clap::Parser;

use cli::{
    AgenticAction, Cli, Commands, GameDayAction, OutputFormat, RollbackStrategy, StoreAction,
};

// The crate root cannot use directory-based module resolution, so the CLI
// definitions and parser tests are wired in explicitly via `#[path]`.
#[path = "main/cli.rs"]
mod cli;

#[cfg(test)]
#[path = "main/tests/cli_and_run.rs"]
mod tests_cli_and_run;

#[cfg(test)]
#[path = "main/tests/commands.rs"]
mod tests_commands;

#[cfg(test)]
#[path = "main/tests/agentic_store.rs"]
mod tests_agentic_store;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
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
        } => {
            commands::cmd_compliance(&journals, framework)?;
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
            commands::cmd_report(&journal, output.as_deref(), format, trace_ui_base.as_deref())?;
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
        } => {
            let options = tumult_intelligence::RecommendOptions {
                store_path: store_path
                    .unwrap_or_else(tumult_analytics::AnalyticsStore::default_path),
                goal,
                model,
                include_draft: !no_draft,
                format: format.into(),
            };
            println!("{}", tumult_intelligence::recommend(&options)?);
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
    }

    // Flush OTel spans before exit
    telemetry.shutdown();

    Ok(())
}
