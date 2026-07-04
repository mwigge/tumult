//! `winfault` — a tiny standalone runner for a single Windows fault.
//!
//! The orchestrator cross-compiles this binary to `x86_64-pc-windows-gnu` and
//! executes it inside the Windows 11 guest to validate one fault end to end. It
//! prints a JSON result to stdout and exits non-zero on failure. It parses argv
//! by hand (no `clap`) to stay dependency-light so it cross-compiles without
//! pulling heavy C dependencies.
//!
//! # Usage
//!
//! ```text
//! winfault process-kill --image notepad.exe
//! winfault process-kill --pid 4321
//! winfault cpu-stress --workers 4 --duration-secs 30
//! winfault network-blackhole --port 443
//! winfault network-blackhole --remote-host 10.0.0.5
//! winfault network-blackhole-rollback --rule-name tumult-blackhole-port-443
//! ```

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use tumult_windows::commands::BlackholeTarget;
use tumult_windows::faults;

/// Parse `--flag value` pairs from the remaining argv into a map.
fn parse_flags(rest: &[String]) -> Result<HashMap<String, String>, String> {
    let mut flags = HashMap::new();
    let mut iter = rest.iter();
    while let Some(token) = iter.next() {
        let Some(key) = token.strip_prefix("--") else {
            return Err(format!("expected a --flag, got `{token}`"));
        };
        let value = iter
            .next()
            .ok_or_else(|| format!("flag `--{key}` is missing a value"))?;
        flags.insert(key.to_string(), value.clone());
    }
    Ok(flags)
}

/// Run the selected fault, returning its JSON detail on success.
fn run(fault: &str, flags: &HashMap<String, String>) -> Result<serde_json::Value, String> {
    let flag = |k: &str| flags.get(k).map(String::as_str);
    let parse_num = |k: &str| -> Result<Option<u32>, String> {
        flags
            .get(k)
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| format!("`--{k}` must be a number"))
            })
            .transpose()
    };

    match fault {
        "process-kill" => {
            let pid = parse_num("pid")?;
            let report = faults::process_kill(flag("image"), pid).map_err(|e| e.to_string())?;
            Ok(report.to_json())
        }
        "cpu-stress" => {
            let workers = parse_num("workers")?
                .and_then(|w| usize::try_from(w).ok())
                .unwrap_or_else(faults::default_workers);
            let duration_secs = parse_num("duration-secs")?.map_or(10u64, u64::from);
            let report = faults::cpu_stress(workers, Duration::from_secs(duration_secs));
            Ok(report.to_json())
        }
        "network-blackhole" => {
            let port = flags
                .get("port")
                .map(|v| v.parse::<u16>().map_err(|_| "`--port` must be a port number".to_string()))
                .transpose()?;
            let target = BlackholeTarget::from_args(port, flag("remote-host"))
                .map_err(|e| e.to_string())?;
            let report = faults::network_blackhole(&target).map_err(|e| e.to_string())?;
            Ok(report.to_json())
        }
        "network-blackhole-rollback" => {
            let rule_name = flag("rule-name")
                .ok_or_else(|| "`--rule-name` is required".to_string())?;
            let stdout = faults::network_blackhole_rollback(rule_name).map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "rule_name": rule_name, "stdout": stdout }))
        }
        other => Err(format!(
            "unknown fault `{other}` (expected process-kill, cpu-stress, network-blackhole, or network-blackhole-rollback)"
        )),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((fault, rest)) = argv.split_first() else {
        eprintln!(
            "usage: winfault <process-kill|cpu-stress|network-blackhole|network-blackhole-rollback> [--flag value ...]"
        );
        return ExitCode::FAILURE;
    };

    let outcome = parse_flags(rest).and_then(|flags| run(fault, &flags));

    let (value, code) = match outcome {
        Ok(detail) => (
            serde_json::json!({ "success": true, "fault": fault, "result": detail }),
            ExitCode::SUCCESS,
        ),
        Err(error) => (
            serde_json::json!({ "success": false, "fault": fault, "error": error }),
            ExitCode::FAILURE,
        ),
    };

    println!("{value}");
    code
}
