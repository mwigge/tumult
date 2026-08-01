use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tumult_ingest::Config;
use tumult_lake::Store;

pub(crate) fn report(metric: String, out: Option<PathBuf>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    match render_metric_report(&config.db_path, &config.metrics_dir, &metric)? {
        ReportLookup::Html(html) => match out {
            Some(path) => {
                std::fs::write(&path, &html)
                    .with_context(|| format!("write report to {}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            None => print!("{html}"),
        },
        ReportLookup::UnknownMetric(msg) => anyhow::bail!(msg),
    }
    Ok(())
}

/// Outcome of looking up and rendering one metric report.
enum ReportLookup {
    Html(String),
    UnknownMetric(String),
}

/// Load metric definitions, find `metric`, and render its HTML report against
/// the store at `db_path` (opened read-only). Shared by the `report`
/// subcommand and the live `GET /report` endpoint.
fn render_metric_report(
    db_path: &std::path::Path,
    metrics_dir: &std::path::Path,
    metric: &str,
) -> Result<ReportLookup> {
    let defs = tumult_metrics::load_dir(metrics_dir)
        .with_context(|| format!("load metrics from {}", metrics_dir.display()))?;
    let Some(def) = defs.iter().find(|d| d.name == metric) else {
        return Ok(ReportLookup::UnknownMetric(format!(
            "metric {metric:?} not found; available: {}",
            defs.iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };

    // Cross-process, DuckDB allows only one process with the store open
    // read-write — so this fails while another daemon holds `db_path`. Inside
    // the daemon process (the /report endpoint) the read-only connection
    // shares the in-process instance and coexists with the ingest writer.
    let store = Store::at(db_path);
    let reader = store.read_only().context("open store read-only")?;
    let report = tumult_report::build_report(
        &reader,
        std::slice::from_ref(def),
        &format!("Tumult — {metric}"),
        None,
    )?;
    Ok(ReportLookup::Html(tumult_report::render_html(&report)))
}

/// State for the live report endpoint: where the store and metric
/// definitions live.
#[derive(Clone)]
pub(crate) struct ReportState {
    pub(crate) db_path: PathBuf,
    pub(crate) metrics_dir: PathBuf,
}

/// `GET /report?metric=<name>` — render a metric report from the live store
/// while the daemon is running (used by the docker demo's report step).
pub(crate) fn report_router(state: ReportState) -> Router {
    Router::new()
        .route("/report", get(report_handler))
        .with_state(state)
}

async fn report_handler(
    State(state): State<ReportState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(metric) = params.get("metric").cloned() else {
        return (StatusCode::BAD_REQUEST, "missing query parameter: metric").into_response();
    };
    let result = tokio::task::spawn_blocking(move || {
        render_metric_report(&state.db_path, &state.metrics_dir, &metric)
    })
    .await;
    match result {
        Ok(Ok(ReportLookup::Html(html))) => Html(html).into_response(),
        Ok(Ok(ReportLookup::UnknownMetric(msg))) => (StatusCode::NOT_FOUND, msg).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("report task failed: {e}"),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Automatic reporting (KRONIKA_REPORT_INTERVAL)

/// Parse an interval value (`45s`, `30m`, `1h`, `1d`, …).
pub(crate) fn parse_interval(raw: &str) -> Option<std::time::Duration> {
    let (num, unit) = raw.split_at(raw.len().checked_sub(1)?);
    let n: u64 = num.trim().parse().ok()?;
    if n == 0 {
        return None;
    }
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3_600)?,
        "d" => n.checked_mul(86_400)?,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(secs))
}

/// `None` when the env var is unset, empty, `0` or `off`; invalid values are
/// warned about and treated as off.
pub(crate) fn report_interval_from_env() -> Option<std::time::Duration> {
    let raw = std::env::var("KRONIKA_REPORT_INTERVAL").ok()?;
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    match parse_interval(raw) {
        Some(d) => {
            tracing::info!(interval = ?d, "automatic reporting enabled");
            Some(d)
        }
        None => {
            tracing::warn!(
                value = raw,
                "invalid KRONIKA_REPORT_INTERVAL (want e.g. 30m or 1h); automatic reporting disabled"
            );
            None
        }
    }
}

/// Render one digest for the trailing `interval` window and write it to
/// `reports_dir/report_<epoch>.html`. When the LLM is reachable, a grounded
/// narrative section is prepended (see `tumult_report::narrative`).
async fn write_digest(
    db_path: &std::path::Path,
    metrics_dir: &std::path::Path,
    reports_dir: &std::path::Path,
    interval: std::time::Duration,
    llm: std::sync::Arc<dyn tumult_intelligence::llm::Llm>,
) -> Result<PathBuf> {
    let (db, mdir) = (db_path.to_path_buf(), metrics_dir.to_path_buf());
    let report = tokio::task::spawn_blocking(move || -> Result<tumult_report::Report> {
        let defs = tumult_metrics::load_dir(&mdir)
            .with_context(|| format!("load metrics from {}", mdir.display()))?;
        let store = Store::at(&db);
        let reader = store.read_only().context("open store read-only")?;
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let from_ns = (now_s - interval.as_secs()) as i64 * 1_000_000_000;
        let to_ns = now_s as i64 * 1_000_000_000;
        Ok(tumult_report::build_report(
            &reader,
            &defs,
            &format!("Tumult digest — last {}s", interval.as_secs()),
            Some((from_ns, to_ns)),
        )?)
    })
    .await??;
    // Best-effort LLM narrative: unreachable LLM, timeout or a reply with no
    // grounded sentences leaves the digest unchanged.
    let report =
        tumult_report::narrative::narrate(&llm, report, std::time::Duration::from_secs(30)).await;
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::create_dir_all(reports_dir)?;
    let path = reports_dir.join(format!("report_{now_s}.html"));
    std::fs::write(&path, tumult_report::render_html(&report))
        .with_context(|| format!("write digest to {}", path.display()))?;
    Ok(path)
}

/// Spawn the report scheduler: one digest per interval, written into
/// `<db dir>/reports/` where `/api/reports` picks it up. Failures are logged
/// and the schedule continues.
pub(crate) fn spawn_report_scheduler(
    db_path: PathBuf,
    metrics_dir: PathBuf,
    reports_dir: PathBuf,
    interval: std::time::Duration,
    llm: std::sync::Arc<dyn tumult_intelligence::llm::Llm>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick: produce the first digest after one
        // full interval, once ingest has had time to land data.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let (db, mdir, rdir, llm) = (
                db_path.clone(),
                metrics_dir.clone(),
                reports_dir.clone(),
                llm.clone(),
            );
            match write_digest(&db, &mdir, &rdir, interval, llm).await {
                Ok(path) => {
                    tracing::info!(path = %path.display(), "scheduled digest written")
                }
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "scheduled digest failed"),
            }
        }
    });
}
