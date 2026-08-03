//! `GameDay` tools: create, run, analyze, and list multi-experiment
//! `GameDay` sessions.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::listing::extract_title;
use crate::tools::StructuredReport;

/// Parameters for [`gameday_create`].
pub struct GameDayCreateRequest<'a> {
    /// Where the `.gameday.toon` file is written (already
    /// resolved/contained by the caller).
    pub output_path: &'a Path,
    /// `GameDay` title.
    pub name: &'a str,
    /// Experiment `.toon` paths referenced by the campaign.
    pub experiments: &'a [String],
    /// Load tool: `k6`, `jmeter`, or `none`/absent for no load.
    pub load_tool: Option<&'a str>,
    /// Load script path recorded in the load config.
    pub load_script: Option<&'a str>,
    /// Virtual users for the load test.
    pub load_vus: Option<u32>,
    /// Compliance framework CLI name (e.g. `dora`), if mapped.
    pub framework: Option<&'a str>,
}

/// Create a `.gameday.toon` campaign file (mirrors `gameday create`,
/// sharing the template via `tumult_core::types::gameday_toon_template`).
///
/// # Errors
///
/// Returns a [`ToolError`] if the load tool or framework is invalid, the
/// file already exists, or it cannot be written.
pub fn gameday_create(request: &GameDayCreateRequest<'_>) -> Result<StructuredReport, ToolError> {
    use tumult_core::types::{gameday_toon_template, GameDayTemplateSpec, LoadTool};

    let load_tool = match request.load_tool {
        None | Some("none") => None,
        Some("k6") => Some(LoadTool::K6),
        Some("jmeter") => Some(LoadTool::Jmeter),
        Some(other) => {
            return Err(ToolError::InvalidInput(format!(
                "unknown load_tool '{other}'; valid values: k6, jmeter, none"
            )))
        }
    };
    // LoadConfig.script is a required field: without it the created file
    // would not parse back as a GameDay, so refuse instead of writing it.
    if load_tool.is_some() && request.load_script.is_none() {
        return Err(ToolError::InvalidInput(
            "load_script is required when load_tool is set (the gameday load config \
             cannot be parsed without a script path)"
                .into(),
        ));
    }
    let framework_report_str = request
        .framework
        .map(tumult_core::compliance::ComplianceFramework::parse)
        .transpose()
        .map_err(ToolError::InvalidInput)?
        .map(tumult_core::compliance::ComplianceFramework::as_report_str);

    if request.output_path.exists() {
        return Err(ToolError::AlreadyExists(format!(
            "{} already exists",
            request.output_path.display()
        )));
    }

    let experiments: Vec<std::path::PathBuf> = request
        .experiments
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let content = gameday_toon_template(&GameDayTemplateSpec {
        name: request.name,
        experiments: &experiments,
        load_tool,
        load_script: request.load_script.map(Path::new),
        load_vus: request.load_vus,
        framework_report_str,
    });
    std::fs::write(request.output_path, &content)?;

    let path_str = request.output_path.display().to_string();
    let mut structured = serde_json::Map::new();
    structured.insert("path".into(), serde_json::json!(path_str));
    structured.insert(
        "experiments".into(),
        serde_json::json!(request.experiments.len()),
    );

    let mut text = String::new();
    writeln!(text, "Created: {path_str}").ok();
    writeln!(
        text,
        "Edit the file to add compliance_maps and regulatory requirements."
    )
    .ok();
    writeln!(text, "Run with the tumult_gameday_run tool.").ok();

    Ok(StructuredReport { text, structured })
}

/// Runs a `GameDay` — all experiments under shared load.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `GameDay` cannot be read, parsed,
/// or any experiment fails to execute.
#[allow(clippy::too_many_lines)] // GameDay orchestration spans load setup, multi-experiment execution, and result aggregation
pub fn gameday_run(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::controls::{ControlRegistry, ProviderControl};
    use tumult_core::engine::{
        apply_template_vars, build_config_env, build_secret_env, flatten_secrets, parse_experiment,
        resolve_config, resolve_secrets, validate_experiment,
    };
    use tumult_core::runner::{run_gameday_with_wiring, ExperimentWiring, RunConfig};
    use tumult_core::types::GameDay;

    let path = Path::new(gameday_path);
    let content = std::fs::read_to_string(path)?;

    let gameday: GameDay = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse gameday: {e}")))?;

    let gameday_dir = path.parent().unwrap_or(Path::new("."));

    // Each experiment gets the same configuration/secrets semantics as
    // `tumult run`: values resolve up front (fatal on a missing env var or
    // secret file), template substitution applies, resolved pairs inject
    // into subprocesses as TUMULT_CONFIG_* / TUMULT_SECRET_*, and declared
    // controls are registered so they fire at lifecycle events.
    let mut experiments = Vec::new();
    let mut wirings = Vec::new();
    for gd_exp in &gameday.experiments {
        let exp_path = if gd_exp.path.is_absolute() {
            gd_exp.path.clone()
        } else {
            gameday_dir.join(&gd_exp.path)
        };
        let exp_content = std::fs::read_to_string(&exp_path)?;
        let experiment = parse_experiment(&exp_content).map_err(|e| {
            ToolError::Parse(format!("failed to parse {}: {e}", exp_path.display()))
        })?;

        let config = resolve_config(&experiment.configuration).map_err(|e| {
            ToolError::Validation(format!(
                "failed to resolve configuration for {}: {e}",
                exp_path.display()
            ))
        })?;
        let secrets = resolve_secrets(&experiment.secrets).map_err(|e| {
            ToolError::Validation(format!(
                "failed to resolve secrets for {}: {e}",
                exp_path.display()
            ))
        })?;
        let secrets_flat = flatten_secrets(&secrets);
        let experiment = if config.is_empty() && secrets_flat.is_empty() {
            experiment
        } else {
            apply_template_vars(
                &experiment,
                &std::collections::HashMap::new(),
                &config,
                &secrets_flat,
            )
            .map_err(|e| {
                ToolError::Validation(format!(
                    "failed to apply template variables for {}: {e}",
                    exp_path.display()
                ))
            })?
        };
        validate_experiment(&experiment).map_err(|e| {
            ToolError::Validation(format!("invalid experiment {}: {e}", exp_path.display()))
        })?;

        let (config_env, skipped_config) = build_config_env(&config);
        let (secret_env, skipped_secrets) = build_secret_env(&secrets_flat);
        for key in skipped_config.iter().chain(&skipped_secrets) {
            tracing::warn!(
                key = %key,
                file = %exp_path.display(),
                "config/secret key does not form a valid env var name after uppercasing; \
                 usable in templates but not injected as TUMULT_*"
            );
        }
        let mut injected_env = config_env;
        injected_env.extend(secret_env);

        let executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
            std::sync::Arc::new(crate::handler::ProcessExecutor::with_injected_env(
                injected_env,
            ));
        let mut controls = ControlRegistry::new();
        for control in &experiment.controls {
            controls.register(Box::new(ProviderControl::new(
                control.clone(),
                executor.clone(),
            )));
        }
        wirings.push(ExperimentWiring {
            executor,
            controls: std::sync::Arc::new(controls),
        });
        experiments.push(experiment);
    }

    // Same load executor as `tumult gameday run` (CLI parity): a declared
    // load config runs through the shared k6 executor instead of being
    // silently dropped.
    let load_executor: Option<std::sync::Arc<dyn tumult_core::runner::LoadExecutor>> =
        gameday.load.is_some().then(|| {
            std::sync::Arc::new(tumult_core::runner::k6::K6LoadExecutor)
                as std::sync::Arc<dyn tumult_core::runner::LoadExecutor>
        });
    let config = RunConfig {
        load_executor,
        ..RunConfig::default()
    };

    let journal = run_gameday_with_wiring(
        &gameday,
        &experiments,
        &|index, _| wirings[index].clone(),
        &config,
    )
    .map_err(|e| ToolError::Execution(format!("gameday failed: {e}")))?;

    // Write journal
    let journal_path = path.with_extension("journal.toon");
    let toon_out = toon_format::encode_default(&journal)
        .map_err(|e| ToolError::Execution(format!("failed to encode journal: {e}")))?;
    std::fs::write(&journal_path, &toon_out)?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(
        output,
        "Resilience Score: {:.2}",
        journal.resilience_score.overall
    )
    .ok();
    writeln!(
        output,
        "Experiments: {}/{} passed",
        journal
            .experiment_journals
            .iter()
            .filter(|j| j.status == tumult_core::types::ExperimentStatus::Completed)
            .count(),
        journal.experiment_journals.len()
    )
    .ok();
    if let Some(ref lr) = journal.load_result {
        writeln!(
            output,
            "Load ({}): {} requests, p95={}ms, error_rate={:.4}",
            lr.tool, lr.total_requests, lr.latency_p95_ms, lr.error_rate
        )
        .ok();
    } else if gameday.load.is_some() {
        // Report the gap rather than stay silent: the campaign declared load but no result
        // came back (e.g. the k6 binary is not installed on this host).
        writeln!(
            output,
            "Load: declared but produced no result (load tool failed to start; see server logs)"
        )
        .ok();
    }
    writeln!(output, "Journal: {}", journal_path.display()).ok();

    Ok(output)
}

/// Analyzes a completed `GameDay` journal.
///
/// # Errors
///
/// Returns a [`ToolError`] if the journal cannot be read or parsed.
pub fn gameday_analyze(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::types::GameDayJournal;

    let path = Path::new(gameday_path);
    let journal_path = path.with_extension("journal.toon");
    let content = std::fs::read_to_string(&journal_path)?;

    let journal: GameDayJournal = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse: {e}")))?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(output, "Score: {:.2}", journal.resilience_score.overall).ok();
    writeln!(
        output,
        "  Pass rate: {:.2}",
        journal.resilience_score.pass_rate
    )
    .ok();
    writeln!(
        output,
        "  Recovery: {:.2}",
        journal.resilience_score.recovery_compliance
    )
    .ok();
    writeln!(
        output,
        "  Load impact: {:.2}",
        journal.resilience_score.load_impact_tolerance
    )
    .ok();
    writeln!(
        output,
        "  Compliance: {:.2}",
        journal.resilience_score.compliance_coverage
    )
    .ok();

    for (i, ej) in journal.experiment_journals.iter().enumerate() {
        let icon = if ej.status == tumult_core::types::ExperimentStatus::Completed {
            "PASS"
        } else {
            "FAIL"
        };
        writeln!(
            output,
            "  #{} [{}] {} ({}ms)",
            i + 1,
            icon,
            ej.experiment_title,
            ej.duration_ms
        )
        .ok();
    }

    Ok(output)
}

/// Lists `.gameday.toon` files found recursively under `search_root`,
/// sorted by path.
///
/// Returns one page of `limit` entries starting at `offset`. The structured
/// object is `{items, total, offset, limit}` with `{path, title}` items;
/// the text keeps the legacy `title  (path)` line per returned entry.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if `search_root` is not a directory.
pub fn gameday_list(
    search_root: &str,
    limit: usize,
    offset: usize,
) -> Result<StructuredReport, ToolError> {
    let root = Path::new(search_root);
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(format!(
            "not a directory: {search_root}"
        )));
    }

    let mut entries = Vec::new();
    collect_gameday_files(root, &mut entries);
    entries.sort();

    let total = entries.len();
    let page: Vec<(String, String)> = entries.into_iter().skip(offset).take(limit).collect();

    let mut text = String::new();
    let items: Vec<serde_json::Value> = page
        .iter()
        .map(|(path, title)| {
            writeln!(text, "{title}  ({path})").ok();
            serde_json::json!({ "path": path, "title": title })
        })
        .collect();
    if total == 0 {
        text = "No .gameday.toon files found.".to_string();
    }

    let mut structured = serde_json::Map::new();
    structured.insert("items".into(), serde_json::json!(items));
    structured.insert("total".into(), serde_json::json!(total));
    structured.insert("offset".into(), serde_json::json!(offset));
    structured.insert("limit".into(), serde_json::json!(limit));
    Ok(StructuredReport { text, structured })
}

fn collect_gameday_files(dir: &Path, entries: &mut Vec<(String, String)>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gameday_files(&path, entries);
        } else if path.extension().and_then(|e| e.to_str()) == Some("toon")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".gameday"))
        {
            let title = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| extract_title(&c))
                .unwrap_or_else(|| "(untitled)".to_string());
            entries.push((path.display().to_string(), title));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_request<'a>(
        output_path: &'a Path,
        experiments: &'a [String],
        load_tool: Option<&'a str>,
        framework: Option<&'a str>,
    ) -> GameDayCreateRequest<'a> {
        GameDayCreateRequest {
            output_path,
            name: "unit-gd",
            experiments,
            load_tool,
            load_script: None,
            load_vus: None,
            framework,
        }
    }

    #[test]
    fn gameday_create_rejects_unknown_load_tool() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["a.toon".to_string()];
        let err = gameday_create(&create_request(&out, &experiments, Some("locust"), None))
            .expect_err("unknown load tool must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("locust"), "must name the bad value: {msg}");
        assert!(
            msg.contains("k6") && msg.contains("jmeter") && msg.contains("none"),
            "must list valid values: {msg}"
        );
        assert!(!out.exists(), "no file must be written");
    }

    #[test]
    fn gameday_create_rejects_unknown_framework_before_writing() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["a.toon".to_string()];
        let err = gameday_create(&create_request(&out, &experiments, None, Some("hipaa")))
            .expect_err("unknown framework must be rejected");
        assert!(err.to_string().contains("dora"), "got: {err}");
        assert!(!out.exists(), "no file must be written");
    }

    #[test]
    fn gameday_list_paginates_sorted_entries_with_totals() {
        let dir = TempDir::new().unwrap();
        for name in ["b", "a", "c"] {
            std::fs::write(
                dir.path().join(format!("{name}.gameday.toon")),
                format!("title: GD {name}\n"),
            )
            .unwrap();
        }
        let report = gameday_list(dir.path().to_str().unwrap(), 1, 1).unwrap();
        assert_eq!(report.structured["total"], 3);
        assert_eq!(report.structured["offset"], 1);
        assert_eq!(report.structured["limit"], 1);
        let items = report.structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("b.gameday.toon"));
        assert_eq!(items[0]["title"], "GD b");
        assert!(report.text.contains("GD b"));

        let empty = gameday_list(dir.path().to_str().unwrap(), 10, 10).unwrap();
        assert_eq!(empty.structured["total"], 3);
        assert!(empty.structured["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn gameday_create_treats_none_load_tool_as_no_load() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["a.toon".to_string()];
        let report =
            gameday_create(&create_request(&out, &experiments, Some("none"), None)).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(!content.contains("load:"), "no load block expected");
        assert_eq!(report.structured["experiments"], 1);
        assert_eq!(
            report.structured["path"],
            out.display().to_string().as_str()
        );
    }

    #[test]
    fn gameday_create_with_k6_load_writes_load_block_and_framework() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("loaded.gameday.toon");
        let experiments = vec!["a.toon".to_string(), "b.toon".to_string()];
        let report = gameday_create(&GameDayCreateRequest {
            output_path: &out,
            name: "loaded-gd",
            experiments: &experiments,
            load_tool: Some("k6"),
            load_script: Some("load.js"),
            load_vus: Some(10),
            framework: Some("dora"),
        })
        .unwrap();

        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("tool: k6"), "{content}");
        assert!(content.contains("script: load.js"), "{content}");
        assert!(content.contains("vus: 10"), "{content}");
        assert!(content.contains("frameworks[1]: DORA"), "{content}");
        assert!(content.contains("- path: a.toon"), "{content}");
        assert_eq!(report.structured["experiments"], 2);
        assert!(report.text.contains("Created:"), "{}", report.text);
        assert!(
            report.text.contains("tumult_gameday_run"),
            "{}",
            report.text
        );
    }

    #[test]
    fn gameday_create_with_jmeter_load_tool() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("jmeter.gameday.toon");
        let experiments = vec!["a.toon".to_string()];
        gameday_create(&GameDayCreateRequest {
            output_path: &out,
            name: "jmeter-gd",
            experiments: &experiments,
            load_tool: Some("jmeter"),
            load_script: Some("plan.jmx"),
            load_vus: None,
            framework: None,
        })
        .unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("tool: jmeter"), "{content}");
        assert!(content.contains("script: plan.jmx"), "{content}");
        assert!(!content.contains("vus:"), "no vus expected: {content}");
        assert!(!content.contains("regulatory:"), "no framework: {content}");
    }

    #[test]
    fn gameday_create_requires_script_when_load_tool_set() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["a.toon".to_string()];
        let err = gameday_create(&create_request(&out, &experiments, Some("k6"), None))
            .expect_err("load tool without a script must be rejected");
        assert!(err.to_string().contains("load_script"), "got: {err}");
        assert!(!out.exists(), "no file must be written");
    }

    #[test]
    fn gameday_create_refuses_to_overwrite_existing_file() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        std::fs::write(&out, "original").unwrap();
        let experiments = vec!["a.toon".to_string()];
        let err = gameday_create(&create_request(&out, &experiments, None, None))
            .expect_err("existing file must not be overwritten");
        assert!(matches!(err, ToolError::AlreadyExists(_)), "got: {err}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "original");
    }

    /// Write a runnable campaign: one echo-backed experiment plus the
    /// `.gameday.toon` referencing it (relative path, resolved against the
    /// gameday's directory). Returns the gameday path.
    fn write_runnable_gameday(dir: &Path) -> std::path::PathBuf {
        crate::tools::test_support::write_valid_experiment(dir);
        let out = dir.join("unit-gd.gameday.toon");
        let experiments = vec!["test.toon".to_string()];
        gameday_create(&create_request(&out, &experiments, None, None)).unwrap();
        out
    }

    #[test]
    fn gameday_run_executes_campaign_and_writes_journal() {
        let dir = TempDir::new().unwrap();
        let out = write_runnable_gameday(dir.path());

        let report = gameday_run(out.to_str().unwrap()).unwrap();
        assert!(report.contains("GameDay: unit-gd"), "{report}");
        assert!(report.contains("Status:"), "{report}");
        assert!(report.contains("Resilience Score:"), "{report}");
        assert!(
            report.contains("Experiments: 1/1 passed"),
            "echo experiment must pass: {report}"
        );
        let journal_path = out.with_extension("journal.toon");
        assert!(journal_path.exists(), "journal must be written");
        assert!(report.contains(&journal_path.display().to_string()));
    }

    #[test]
    fn gameday_analyze_reports_experiment_outcomes() {
        let dir = TempDir::new().unwrap();
        let out = write_runnable_gameday(dir.path());
        gameday_run(out.to_str().unwrap()).unwrap();

        let analysis = gameday_analyze(out.to_str().unwrap()).unwrap();
        assert!(analysis.contains("GameDay: unit-gd"), "{analysis}");
        assert!(analysis.contains("Pass rate:"), "{analysis}");
        assert!(analysis.contains("Recovery:"), "{analysis}");
        assert!(analysis.contains("Load impact:"), "{analysis}");
        assert!(analysis.contains("Compliance:"), "{analysis}");
        assert!(
            analysis.contains("#1 [PASS] MCP test experiment"),
            "{analysis}"
        );
    }

    #[test]
    fn gameday_run_missing_file_is_an_io_error() {
        let err = gameday_run("/nonexistent/unit-gd.gameday.toon")
            .expect_err("a missing gameday file must fail");
        assert!(matches!(err, ToolError::Io(_)), "got: {err}");
    }

    #[test]
    fn gameday_run_rejects_unparseable_gameday() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("bad.gameday.toon");
        std::fs::write(&out, "title: [unterminated").unwrap();
        let err = gameday_run(out.to_str().unwrap()).expect_err("garbage must not parse");
        assert!(
            err.to_string().contains("failed to parse gameday"),
            "got: {err}"
        );
    }

    #[test]
    fn gameday_run_rejects_missing_experiment_file() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["no-such-experiment.toon".to_string()];
        gameday_create(&create_request(&out, &experiments, None, None)).unwrap();
        let err =
            gameday_run(out.to_str().unwrap()).expect_err("a missing experiment file must fail");
        assert!(matches!(err, ToolError::Io(_)), "got: {err}");
    }

    #[test]
    fn gameday_run_rejects_unparseable_experiment() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("broken.toon"), "title: [unterminated").unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        let experiments = vec!["broken.toon".to_string()];
        gameday_create(&create_request(&out, &experiments, None, None)).unwrap();
        let err =
            gameday_run(out.to_str().unwrap()).expect_err("an unparseable experiment must fail");
        let msg = err.to_string();
        assert!(msg.contains("failed to parse"), "got: {msg}");
        assert!(msg.contains("broken.toon"), "must name the file: {msg}");
    }

    #[test]
    fn gameday_analyze_missing_journal_is_an_io_error() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        std::fs::write(&out, "title: gd\n").unwrap();
        let err = gameday_analyze(out.to_str().unwrap())
            .expect_err("analyze without a journal must fail");
        assert!(matches!(err, ToolError::Io(_)), "got: {err}");
    }

    #[test]
    fn gameday_analyze_rejects_unparseable_journal() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("unit-gd.gameday.toon");
        std::fs::write(&out, "title: gd\n").unwrap();
        std::fs::write(out.with_extension("journal.toon"), "title: [unterminated").unwrap();
        let err =
            gameday_analyze(out.to_str().unwrap()).expect_err("an unparseable journal must fail");
        assert!(err.to_string().contains("failed to parse"), "got: {err}");
    }

    #[test]
    fn gameday_list_rejects_non_directory_root() {
        let err = gameday_list("/nonexistent/search-root", 10, 0)
            .expect_err("a non-directory search root must be rejected");
        assert!(err.to_string().contains("not a directory"), "got: {err}");
    }

    #[test]
    fn gameday_list_reports_empty_result() {
        let dir = TempDir::new().unwrap();
        let report = gameday_list(dir.path().to_str().unwrap(), 10, 0).unwrap();
        assert_eq!(report.structured["total"], 0);
        assert_eq!(report.text, "No .gameday.toon files found.");
    }

    #[test]
    fn gameday_list_finds_nested_files_and_falls_back_to_untitled() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        // No title line: the listing falls back to "(untitled)".
        std::fs::write(nested.join("x.gameday.toon"), "description: no title\n").unwrap();
        let report = gameday_list(dir.path().to_str().unwrap(), 10, 0).unwrap();
        assert_eq!(report.structured["total"], 1);
        let items = report.structured["items"].as_array().unwrap();
        assert_eq!(items[0]["title"], "(untitled)");
        assert!(items[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("nested/x.gameday.toon"));
    }
}
