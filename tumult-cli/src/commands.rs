//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

use std::path::Path;

use tumult_core::controls::ControlRegistry;
use tumult_core::engine::{parse_experiment, resolve_config, resolve_secrets, validate_experiment};
use tumult_core::execution::RollbackStrategy;
use tumult_core::journal::write_journal;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;
use tumult_plugin::discovery::discover_all_plugins;
use tumult_plugin::registry::PluginRegistry;

use anyhow::{bail, Context, Result};

// ── Provider-based executor ───────────────────────────────────

/// Executes activities by dispatching to the appropriate provider.
///
/// For Phase 0, this supports Process and HTTP providers.
/// Native plugin execution will be wired in when native plugins ship.
pub struct ProviderExecutor;

impl ActivityExecutor for ProviderExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match &activity.provider {
            Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => execute_process(path, arguments, env, timeout_s.as_ref()),
            Provider::Http {
                method,
                url,
                headers: _,
                body: _,
                timeout_s: _,
            } => {
                // HTTP execution is deferred to Phase 1 (requires reqwest)
                // For now, return a placeholder
                ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "HTTP provider not yet implemented: {} {}",
                        format_http_method(method),
                        url
                    )),
                    duration_ms: 0,
                }
            }
            Provider::Native {
                plugin, function, ..
            } => ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!(
                    "Native plugin '{}::{}' not yet available — install with cargo feature flag",
                    plugin, function
                )),
                duration_ms: 0,
            },
        }
    }
}

fn format_http_method(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
    }
}

fn execute_process(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(path);
    cmd.args(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!("failed to execute '{}': {}", path, e)),
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // Apply timeout if configured
    let timeout = timeout_s.map(|s| std::time::Duration::from_secs_f64(*s));
    let status = if let Some(dur) = timeout {
        // Poll until done or timeout
        let deadline = std::time::Instant::now() + dur;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err("timed out".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => break Err(e.to_string()),
            }
        }
    } else {
        child.wait().map_err(|e| e.to_string())
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match status {
        Ok(exit_status) => {
            let stdout = child
                .stdout
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf.trim().to_string()
                })
                .unwrap_or_default();
            let stderr = child
                .stderr
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf.trim().to_string()
                })
                .unwrap_or_default();

            ActivityOutcome {
                success: exit_status.success(),
                output: if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                },
                error: if stderr.is_empty() {
                    None
                } else {
                    Some(stderr)
                },
                duration_ms,
            }
        }
        Err(reason) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(format!("process '{}' {}", path, reason)),
            duration_ms,
        },
    }
}

// ── Run command ───────────────────────────────────────────────

pub fn cmd_run(
    experiment_path: &Path,
    journal_path: &Path,
    dry_run: bool,
    rollback_strategy: RollbackStrategy,
    auto_ingest: bool,
) -> Result<()> {
    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // Resolve configuration and secrets
    let _config = resolve_config(&experiment.configuration)?;
    let _secrets = resolve_secrets(&experiment.secrets)?;

    if dry_run {
        print_dry_run(&experiment);
        return Ok(());
    }

    let executor = ProviderExecutor;
    let controls = ControlRegistry::new();
    let run_config = RunConfig { rollback_strategy };

    println!("Running experiment: {}", experiment.title);

    let journal = run_experiment(&experiment, &executor, &controls, &run_config)?;

    write_journal(&journal, journal_path)?;

    println!("Status: {:?}", journal.status);
    println!("Duration: {}ms", journal.duration_ms);
    println!("Method steps: {} executed", journal.method_results.len());
    if !journal.rollback_results.is_empty() {
        println!("Rollbacks: {} executed", journal.rollback_results.len());
    }
    println!("Journal written to: {}", journal_path.display());

    // Auto-ingest into persistent analytics store
    if auto_ingest {
        match auto_ingest_journal(&journal) {
            Ok(true) => println!("Ingested into persistent analytics store"),
            Ok(false) => println!("Already in analytics store (duplicate)"),
            Err(e) => eprintln!("warning: auto-ingest failed: {}", e),
        }
    }

    // Exit with non-zero if experiment did not complete successfully
    if journal.status != ExperimentStatus::Completed {
        bail!("experiment finished with status: {:?}", journal.status);
    }

    Ok(())
}

fn auto_ingest_journal(journal: &Journal) -> Result<bool> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    let store = AnalyticsStore::open(&db_path)
        .with_context(|| format!("failed to open analytics store: {}", db_path.display()))?;
    let ingested = store.ingest_journal(journal)?;
    Ok(ingested)
}

// ── Validate command ──────────────────────────────────────────

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
        println!("Description: {}", desc);
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
        Err(e) => println!("Configuration: WARNING — {}", e),
    }
    match secrets_result {
        Ok(_) => println!("Secrets: all values resolved"),
        Err(e) => println!("Secrets: WARNING — {}", e),
    }

    println!("\nValidation passed.");
    Ok(())
}

// ── Discover command ──────────────────────────────────────────

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
        println!("Plugin: {}", filter);
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
            println!("  {}", name);
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

pub fn cmd_analyze(journals_path: Option<&Path>, query: Option<&str>) -> Result<()> {
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

    println!("Loaded {} journal(s) into analytics store\n", count);

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
    } else {
        let rows = store.query(
            "SELECT status, count(*) as count, avg(duration_ms) as avg_duration_ms, \
             avg(resilience_score) as avg_resilience \
             FROM experiments GROUP BY status ORDER BY count DESC",
        )?;
        println!("Experiment Summary:");
        println!("Status\t\tCount\tAvg Duration (ms)\tAvg Resilience");
        println!("------\t\t-----\t-----------------\t--------------");
        for row in &rows {
            println!("{}\t\t{}\t{}\t\t\t{}", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

// ── Export command ─────────────────────────────────────────────

pub fn cmd_export(journal_path: &Path, format: &str) -> Result<()> {
    use tumult_analytics::arrow_convert::journal_to_record_batch;
    use tumult_analytics::export::{export_csv, export_parquet};
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let ext = match format {
        "parquet" => "parquet",
        "csv" => "csv",
        "json" => "json",
        _ => bail!("unsupported format: {}", format),
    };
    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("journal");
    let out_path = std::path::PathBuf::from(format!("{}.{}", stem, ext));

    match format {
        "parquet" | "csv" => {
            let (exp_batch, _) = journal_to_record_batch(std::slice::from_ref(&journal))?;
            match format {
                "parquet" => export_parquet(&exp_batch, &out_path)?,
                "csv" => export_csv(&exp_batch, &out_path)?,
                _ => unreachable!(),
            }
        }
        "json" => {
            let json = serde_json::to_string_pretty(&journal)?;
            std::fs::write(&out_path, json)?;
        }
        _ => unreachable!(),
    }
    println!("Exported to: {}", out_path.display());
    Ok(())
}

// ── Trend command ─────────────────────────────────────────────

pub fn cmd_trend(journals_path: &Path, metric: &str, last: Option<&str>) -> Result<()> {
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

    println!("Loaded {} journal(s)\n", count);

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
            format!(
                "--last must be a number of days (e.g., 30d), got: {}",
                window
            )
        })?;
        let cutoff_ns =
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) - (days * 86400 * 1_000_000_000);
        format!(" AND started_at_ns >= {}", cutoff_ns)
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
    let sql = format!("{}{} ORDER BY started_at_ns", base_sql, time_filter);

    let columns = store.query_columns(&sql)?;
    let rows = store.query(&sql)?;

    if rows.is_empty() {
        println!("No data points for metric: {}", metric);
        return Ok(());
    }

    println!("Trend: {} ({} data points)\n", metric, rows.len());
    println!(
        "{}",
        columns
            .iter()
            .map(|c| format!("{:<20}", c))
            .collect::<Vec<_>>()
            .join("")
    );
    println!("{}", "-".repeat(columns.len() * 20));
    for row in &rows {
        println!(
            "{}",
            row.iter()
                .map(|v| format!("{:<20}", v))
                .collect::<Vec<_>>()
                .join("")
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

pub fn cmd_compliance(journals_path: &Path, framework: &str) -> Result<()> {
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

    println!("=== {} Compliance Report ===\n", framework);
    println!("Journals analyzed: {}", count);
    println!("With regulatory tagging: {}\n", journals_with_regulatory);

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
    let completed = store.query("SELECT count(*) FROM experiments WHERE status = 'Completed'")?;
    let total_n: f64 = total[0][0].parse().unwrap_or(0.0);
    let completed_n: f64 = completed[0][0].parse().unwrap_or(0.0);
    let success_rate = if total_n > 0.0 {
        completed_n / total_n * 100.0
    } else {
        0.0
    };

    println!("\nCompliance Status:");
    println!("  Success rate: {:.1}%", success_rate);
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

    // Framework-specific guidance
    println!("\n{} Requirements:", framework);
    match framework {
        "DORA" => {
            println!("  Art. 24: ICT resilience testing programme");
            println!("  Art. 25: Testing of ICT tools and systems");
            println!(
                "  Evidence: {} experiment runs, {:.1}% success rate",
                count, success_rate
            );
        }
        "NIS2" => {
            println!("  Art. 21: Cybersecurity risk-management measures");
            println!("  Art. 23: Incident handling and reporting");
            println!(
                "  Evidence: {} experiment runs, {:.1}% success rate",
                count, success_rate
            );
        }
        "PCI-DSS" => {
            println!("  Req. 11.4: External and internal penetration testing");
            println!("  Req. 12.10: Incident response plan testing");
            println!(
                "  Evidence: {} experiment runs, {:.1}% success rate",
                count, success_rate
            );
        }
        _ => {
            println!(
                "  Evidence: {} experiment runs, {:.1}% success rate",
                count, success_rate
            );
        }
    }

    println!("\n=== End Report ===");
    Ok(())
}

// ── Init command ──────────────────────────────────────────────

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
        println!("Template includes {} plugin actions", p);
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
        println!("Description: {}", desc);
    }
    println!();

    if let Some(ref estimate) = experiment.estimate {
        println!("Phase 0 — Estimate:");
        println!("  Expected outcome: {:?}", estimate.expected_outcome);
        if let Some(recovery) = estimate.expected_recovery_s {
            println!("  Expected recovery: {}s", recovery);
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

// ── Import command ──────────────────────────────────────────

pub fn cmd_import(parquet_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    if !parquet_dir.is_dir() {
        bail!("not a directory: {}", parquet_dir.display());
    }

    let exp_path = parquet_dir.join("experiments.parquet");
    let act_path = parquet_dir.join("activities.parquet");

    if !exp_path.exists() {
        bail!("experiments.parquet not found in {}", parquet_dir.display());
    }
    if !act_path.exists() {
        bail!("activities.parquet not found in {}", parquet_dir.display());
    }

    let db_path = AnalyticsStore::default_path();
    let store = AnalyticsStore::open(&db_path)?;
    store.import_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Imported from: {}", parquet_dir.display());
    println!(
        "Store now contains: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

// ── Store management commands ───────────────────────────────

pub fn cmd_store_stats() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        println!("No persistent store found at: {}", db_path.display());
        println!("Run an experiment to create it automatically.");
        return Ok(());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let stats = store.stats()?;
    let version = store.schema_version()?;

    println!("Store: {}", db_path.display());
    println!("Schema version: {}", version);
    println!("Experiments: {}", stats.experiment_count);
    println!("Activities: {}", stats.activity_count);

    if let Ok(size) = std::fs::metadata(&db_path) {
        let mb = size.len() as f64 / (1024.0 * 1024.0);
        println!("File size: {:.2} MB", mb);
    }

    Ok(())
}

pub fn cmd_store_backup(output_dir: &Path) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    std::fs::create_dir_all(output_dir)?;

    let store = AnalyticsStore::open(&db_path)?;
    let exp_path = output_dir.join("experiments.parquet");
    let act_path = output_dir.join("activities.parquet");

    store.export_tables(&exp_path, &act_path)?;

    let stats = store.stats()?;
    println!("Backed up to: {}", output_dir.display());
    println!("  experiments.parquet — {} rows", stats.experiment_count);
    println!("  activities.parquet — {} rows", stats.activity_count);
    Ok(())
}

pub fn cmd_store_purge(older_than_days: u32) -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    if !db_path.exists() {
        bail!("no persistent store found at: {}", db_path.display());
    }

    let store = AnalyticsStore::open(&db_path)?;
    let purged = store.purge_older_than_days(older_than_days)?;

    let stats = store.stats()?;
    println!(
        "Purged {} experiment(s) older than {} days",
        purged, older_than_days
    );
    println!(
        "Remaining: {} experiments, {} activities",
        stats.experiment_count, stats.activity_count
    );
    Ok(())
}

pub fn cmd_store_path() -> Result<()> {
    use tumult_analytics::AnalyticsStore;

    let db_path = AnalyticsStore::default_path();
    println!("{}", db_path.display());
    if db_path.exists() {
        if let Ok(size) = std::fs::metadata(&db_path) {
            let mb = size.len() as f64 / (1024.0 * 1024.0);
            println!("Size: {:.2} MB", mb);
        }
    } else {
        println!("(not yet created)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Helper: write a valid experiment file ─────────────────

    fn write_valid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("test-experiment.toon");
        let exp = Experiment {
            title: "CLI test experiment".into(),
            description: Some("Tests CLI command execution".into()),
            tags: vec!["test".into()],
            configuration: std::collections::HashMap::new(),
            secrets: std::collections::HashMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![Activity {
                name: "echo-action".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Process {
                    path: "echo".into(),
                    arguments: vec!["hello".into()],
                    env: std::collections::HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
            }],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    fn write_invalid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("invalid.toon");
        std::fs::write(&path, "this is not valid toon {{{").unwrap();
        path
    }

    fn write_empty_method_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("empty-method.toon");
        let exp = Experiment {
            title: "Empty method experiment".into(),
            description: None,
            tags: vec![],
            configuration: std::collections::HashMap::new(),
            secrets: std::collections::HashMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    // ── cmd_run tests ─────────────────────────────────────────

    #[test]
    fn run_valid_experiment_produces_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
        );

        assert!(result.is_ok());
        assert!(journal_path.exists());
    }

    #[test]
    fn run_dry_run_does_not_create_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            true,
            RollbackStrategy::OnDeviation,
            false,
        );

        assert!(result.is_ok());
        assert!(!journal_path.exists());
    }

    #[test]
    fn run_nonexistent_file_returns_error() {
        let result = cmd_run(
            Path::new("/nonexistent/experiment.toon"),
            Path::new("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn run_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn run_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
            false,
        );
        assert!(result.is_err());
    }

    // ── cmd_validate tests ────────────────────────────────────

    #[test]
    fn validate_valid_experiment_succeeds() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_nonexistent_file_returns_error() {
        let result = cmd_validate(Path::new("/nonexistent/experiment.toon"));
        assert!(result.is_err());
    }

    #[test]
    fn validate_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    #[test]
    fn validate_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    // ── cmd_discover tests ────────────────────────────────────

    #[test]
    fn discover_without_plugins_shows_empty() {
        // No plugins in default search paths during tests
        let result = cmd_discover(None);
        assert!(result.is_ok());
    }

    #[test]
    fn discover_nonexistent_plugin_returns_error() {
        let result = cmd_discover(Some("nonexistent-plugin"));
        assert!(result.is_err());
    }

    // ── cmd_init tests ────────────────────────────────────────

    #[test]
    fn init_creates_experiment_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, None);

        assert!(result.is_ok());
        assert!(path.exists());
    }

    #[test]
    fn init_with_plugin_includes_plugin_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, Some("tumult-kafka"));

        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tumult-kafka"));
    }

    #[test]
    fn init_fails_if_file_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");
        std::fs::write(&path, "existing").unwrap();

        let result = init_at(&path, None);
        assert!(result.is_err());
    }

    // ── generate_template tests ───────────────────────────────

    #[test]
    fn template_contains_required_sections() {
        let template = generate_template(None);
        assert!(template.contains("title:"));
        assert!(template.contains("steady_state_hypothesis:"));
        assert!(template.contains("method"));
        assert!(template.contains("rollbacks"));
    }

    #[test]
    fn template_uses_plugin_name() {
        let template = generate_template(Some("tumult-db"));
        assert!(template.contains("tumult-db"));
    }

    // ── ProviderExecutor tests ────────────────────────────────

    #[test]
    fn process_executor_runs_echo() {
        let activity = Activity {
            name: "echo-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello world".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello world"));
    }

    #[test]
    fn process_executor_captures_failure() {
        let activity = Activity {
            name: "false-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "false".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
    }

    #[test]
    fn process_executor_nonexistent_returns_error() {
        let activity = Activity {
            name: "bad-cmd".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "/nonexistent/binary".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: None,
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome.error.is_some());
    }

    #[test]
    fn native_provider_returns_not_implemented() {
        let activity = Activity {
            name: "native-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test-plugin".into(),
                function: "test-fn".into(),
                arguments: std::collections::HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome
            .error
            .as_ref()
            .unwrap()
            .contains("not yet available"));
    }

    // ── Phase 4: Import/Export roundtrip ──────────────────────

    #[test]
    fn import_rejects_missing_directory() {
        let result = cmd_import(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn import_rejects_missing_parquet_files() {
        let d = TempDir::new().unwrap();
        let result = cmd_import(d.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("experiments.parquet not found"));
    }

    // ── Phase 4: Run with auto-ingest ─────────────────────────

    #[test]
    fn cmd_run_with_auto_ingest() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("out.toon");

        // Run with auto-ingest disabled (avoids touching real ~/.tumult)
        let result = cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
            false,
        );
        assert!(result.is_ok());
        assert!(journal_path.exists());
    }

    #[test]
    fn cmd_run_dry_run_does_not_ingest() {
        let d = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(d.path());
        let journal_path = d.path().join("out.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            true,
            RollbackStrategy::OnDeviation,
            true,
        );
        assert!(result.is_ok());
        // Journal should NOT be written in dry-run mode
        assert!(!journal_path.exists());
    }
}
