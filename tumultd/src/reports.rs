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

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use std::sync::Arc;
    use std::time::Duration;

    /// One metric definition over the `spans` table, mirroring
    /// `metrics/experiment_count.yaml`.
    const METRIC_YAML: &str = r#"
name: experiment_count
description: Count of experiment runs in the window, per experiment.
source_table: spans
time_col: ts_ns
measure:
  type: count
dimensions: [experiment_name]
condition: { column: span_name, equals: "resilience.experiment" }
"#;

    /// An initialised (empty) store and a metrics dir with one definition,
    /// both inside one tempdir.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lake.duckdb");
        Store::open(&db_path).unwrap();
        let metrics_dir = dir.path().join("metrics");
        std::fs::create_dir_all(&metrics_dir).unwrap();
        std::fs::write(metrics_dir.join("experiment_count.yaml"), METRIC_YAML).unwrap();
        (dir, db_path, metrics_dir)
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // -- parse_interval ------------------------------------------------------

    #[test]
    fn parse_interval_accepts_every_unit() {
        assert_eq!(parse_interval("45s"), Some(Duration::from_secs(45)));
        assert_eq!(parse_interval("30m"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(parse_interval("1h"), Some(Duration::from_secs(3_600)));
        assert_eq!(parse_interval("2d"), Some(Duration::from_secs(2 * 86_400)));
        // Whitespace around the number is tolerated.
        assert_eq!(parse_interval(" 10m"), Some(Duration::from_secs(600)));
    }

    #[test]
    fn parse_interval_rejects_zero_garbage_and_overflow() {
        assert_eq!(parse_interval(""), None);
        assert_eq!(parse_interval("s"), None);
        assert_eq!(parse_interval("0s"), None);
        assert_eq!(parse_interval("0h"), None);
        assert_eq!(parse_interval("10x"), None);
        assert_eq!(parse_interval("abc"), None);
        assert_eq!(parse_interval("-5m"), None);
        // Seconds pass through unscaled; scaling u64::MAX must not wrap.
        assert_eq!(
            parse_interval(&format!("{}s", u64::MAX)),
            Some(Duration::from_secs(u64::MAX))
        );
        assert_eq!(parse_interval(&format!("{}m", u64::MAX)), None);
        assert_eq!(parse_interval(&format!("{}h", u64::MAX)), None);
        assert_eq!(parse_interval(&format!("{}d", u64::MAX)), None);
    }

    // -- report_interval_from_env --------------------------------------------

    #[test]
    fn report_interval_from_env_off_unless_set_to_a_valid_interval() {
        let _guard = env_lock();
        std::env::remove_var("KRONIKA_REPORT_INTERVAL");
        assert_eq!(report_interval_from_env(), None);
        for off in ["", "0", "off", "OFF", "Off"] {
            std::env::set_var("KRONIKA_REPORT_INTERVAL", off);
            assert_eq!(report_interval_from_env(), None, "{off:?} must disable");
        }
        std::env::set_var("KRONIKA_REPORT_INTERVAL", "30m");
        assert_eq!(report_interval_from_env(), Some(Duration::from_secs(1_800)));
        // Invalid values disable reporting rather than guessing.
        std::env::set_var("KRONIKA_REPORT_INTERVAL", "bogus");
        assert_eq!(report_interval_from_env(), None);
        std::env::remove_var("KRONIKA_REPORT_INTERVAL");
    }

    // -- render_metric_report -------------------------------------------------

    #[test]
    fn render_metric_report_unknown_metric_lists_available_names() {
        let (_dir, db, mdir) = fixture();
        match render_metric_report(&db, &mdir, "nope").unwrap() {
            ReportLookup::UnknownMetric(msg) => {
                assert!(msg.contains("\"nope\" not found"), "{msg}");
                assert!(msg.contains("experiment_count"), "{msg}");
            }
            ReportLookup::Html(_) => panic!("expected UnknownMetric for an unknown name"),
        }
    }

    #[test]
    fn render_metric_report_renders_html_for_a_known_metric() {
        let (_dir, db, mdir) = fixture();
        match render_metric_report(&db, &mdir, "experiment_count").unwrap() {
            ReportLookup::Html(html) => {
                assert!(html.contains("Tumult — experiment_count"), "title missing");
                assert!(html.contains("kpi"), "headline section missing");
            }
            ReportLookup::UnknownMetric(msg) => panic!("unexpected unknown metric: {msg}"),
        }
    }

    #[test]
    fn render_metric_report_fails_when_the_metrics_dir_is_unreadable() {
        let (_dir, db, _mdir) = fixture();
        let result = render_metric_report(
            &db,
            std::path::Path::new("/nonexistent/metrics"),
            "experiment_count",
        );
        let Err(err) = result else {
            panic!("an unreadable metrics dir must fail");
        };
        assert!(format!("{err:#}").contains("load metrics"), "{err:#}");
    }

    // -- report_handler --------------------------------------------------------

    #[tokio::test]
    async fn report_handler_rejects_a_missing_metric_parameter() {
        let (_dir, db_path, metrics_dir) = fixture();
        let resp = report_handler(
            State(ReportState {
                db_path,
                metrics_dir,
            }),
            Query(HashMap::new()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(body_string(resp).await.contains("missing query parameter"));
    }

    #[tokio::test]
    async fn report_handler_unknown_metric_is_not_found() {
        let (_dir, db_path, metrics_dir) = fixture();
        let params = HashMap::from([("metric".to_string(), "nope".to_string())]);
        let resp = report_handler(
            State(ReportState {
                db_path,
                metrics_dir,
            }),
            Query(params),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(body_string(resp).await.contains("not found"));
    }

    #[tokio::test]
    async fn report_handler_serves_html_for_a_known_metric() {
        let (_dir, db_path, metrics_dir) = fixture();
        let params = HashMap::from([("metric".to_string(), "experiment_count".to_string())]);
        let resp = report_handler(
            State(ReportState {
                db_path,
                metrics_dir,
            }),
            Query(params),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with("text/html"), "{content_type}");
        assert!(body_string(resp)
            .await
            .contains("Tumult — experiment_count"));
    }

    // -- report subcommand -----------------------------------------------------

    #[test]
    fn report_subcommand_writes_out_file_and_rejects_unknown_metrics() {
        let _guard = env_lock();
        let (dir, db_path, metrics_dir) = fixture();
        std::env::set_var("TUMULT_LAKE_PATH", &db_path);
        std::env::set_var("KRONIKA_METRICS_DIR", &metrics_dir);

        let out = dir.path().join("report.html");
        report("experiment_count".to_string(), Some(out.clone())).unwrap();
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("Tumult — experiment_count"));

        // Without --out the report goes to stdout; it must still succeed.
        report("experiment_count".to_string(), None).unwrap();

        let err = report("nope".to_string(), None).unwrap_err();
        assert!(format!("{err:#}").contains("not found"), "{err:#}");

        std::env::remove_var("TUMULT_LAKE_PATH");
        std::env::remove_var("KRONIKA_METRICS_DIR");
    }

    // -- scheduled digests -----------------------------------------------------

    /// An LLM that is never reachable: the digest must fall back to the
    /// deterministic report rather than fail.
    struct OfflineLlm;

    #[async_trait::async_trait]
    impl tumult_intelligence::llm::Llm for OfflineLlm {
        async fn chat(
            &self,
            _messages: &[tumult_intelligence::llm::Message],
        ) -> std::result::Result<String, tumult_intelligence::llm::AiError> {
            Err(tumult_intelligence::llm::AiError::EmptyResponse)
        }
    }

    #[tokio::test]
    async fn write_digest_writes_the_report_when_the_llm_is_unreachable() {
        let (dir, db_path, metrics_dir) = fixture();
        let reports_dir = dir.path().join("reports");
        let path = write_digest(
            &db_path,
            &metrics_dir,
            &reports_dir,
            Duration::from_secs(3_600),
            Arc::new(OfflineLlm),
        )
        .await
        .unwrap();
        assert!(path.starts_with(&reports_dir));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("report_") && name.ends_with(".html"),
            "{name}"
        );
        let html = std::fs::read_to_string(&path).unwrap();
        assert!(
            html.contains("Tumult digest — last 3600s"),
            "digest title missing"
        );
    }

    #[tokio::test]
    async fn report_scheduler_writes_a_digest_after_one_interval() {
        let (dir, db_path, metrics_dir) = fixture();
        let reports_dir = dir.path().join("reports");
        spawn_report_scheduler(
            db_path,
            metrics_dir,
            reports_dir.clone(),
            Duration::from_millis(50),
            Arc::new(OfflineLlm),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let written = std::fs::read_dir(&reports_dir).map_or(0, |rd| rd.count());
            if written > 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no digest appeared in {} within 30s",
                reports_dir.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
