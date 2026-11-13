//! Which version of the target a run measured.
//!
//! `--target-version` declares it on the experiment's target. Left
//! undeclared, the runner asks once, when the run ends, whoever holds the
//! target's telemetry: the daemon named by `TUMULT_DAEMON_URL`
//! (`GET /api/perf/{target}/version`), or else the local store. Whatever
//! comes back is frozen in the journal; no answer leaves the run without
//! a version, and it is then read from the target's spans when a verdict
//! is asked for.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;
use tumult_core::runner::{RunWindow, TargetVersionResolver};
use tumult_core::types::Experiment;

/// Env var naming the daemon base URL (shared with `tumult gate`).
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";
/// How often the daemon is asked before the run goes without a version.
/// The target's exporter batches its spans, so the last of them can reach
/// the daemon a moment after the run has ended.
const ATTEMPTS: u32 = 3;
/// The wait between two questions.
const PAUSE: Duration = Duration::from_secs(1);
/// How long one question may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Put a version given on the command line on the experiment's target,
/// over one the file declares.
///
/// # Errors
///
/// Returns an error when the version is blank or the experiment declares
/// no target to attach it to.
pub(crate) fn declare(experiment: &mut Experiment, version: Option<&str>) -> Result<()> {
    let Some(version) = version else {
        return Ok(());
    };
    let version = version.trim();
    if version.is_empty() {
        bail!("--target-version must name a version");
    }
    let Some(target) = experiment.target.as_mut() else {
        bail!(
            "--target-version {version}: the experiment declares no `target` block to attach \
             the version to"
        );
    };
    target.version = Some(version.to_string());
    Ok(())
}

/// Who is asked which version served a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The daemon at this base URL.
    Daemon(String),
    /// The store at this path, read directly.
    Lake(PathBuf),
}

/// The daemon when a URL is configured, else the local store when there
/// is one, else nobody. Pure but for the existence check.
pub(crate) fn source(daemon_url: Option<String>, lake: Option<PathBuf>) -> Option<Source> {
    if let Some(url) = daemon_url.filter(|url| !url.trim().is_empty()) {
        return Some(Source::Daemon(url));
    }
    lake.filter(|path| path.exists()).map(Source::Lake)
}

/// The resolver for this process's environment.
pub(crate) fn resolver() -> Option<Arc<dyn TargetVersionResolver>> {
    let lake = tumult_lake::AnalyticsStore::default_path().ok();
    match source(std::env::var(DAEMON_URL_ENV).ok(), lake)? {
        Source::Daemon(base) => {
            let token = std::env::var(DAEMON_TOKEN_ENV).ok();
            Some(Arc::new(DaemonVersions::new(base, token)))
        }
        Source::Lake(path) => Some(Arc::new(tumult_lake::StoredVersions::at(&path))),
    }
}

/// The URL and query of the daemon's version route for a run. Pure.
pub(crate) fn request(base: &str, window: &RunWindow) -> (String, Vec<(&'static str, String)>) {
    let url = format!(
        "{}/api/perf/{}/version",
        base.trim_end_matches('/'),
        super::gate::encode_segment(&window.system)
    );
    let mut query = Vec::new();
    if let Some(env) = &window.environment {
        query.push(("env", env.clone()));
    }
    query.push(("from_ns", window.started_at_ns.to_string()));
    query.push(("to_ns", window.ended_at_ns.to_string()));
    (url, query)
}

/// The version in the daemon's answer, when it names one.
pub(crate) fn version_in(body: &Value) -> Option<String> {
    let version = body.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// Asks a daemon which version served a run.
#[derive(Debug, Clone)]
pub(crate) struct DaemonVersions {
    base: String,
    token: Option<String>,
    attempts: u32,
    pause: Duration,
}

impl DaemonVersions {
    pub(crate) fn new(base: String, token: Option<String>) -> Self {
        Self {
            base,
            token: token.filter(|t| !t.is_empty()),
            attempts: ATTEMPTS,
            pause: PAUSE,
        }
    }

    /// One question.
    async fn ask(
        &self,
        client: &reqwest::Client,
        window: &RunWindow,
    ) -> Result<Option<String>, String> {
        let (url, query) = request(&self.base, window);
        let mut asked = client.get(&url).query(&query).timeout(TIMEOUT);
        if let Some(token) = &self.token {
            asked = asked.bearer_auth(token);
        }
        let answer = asked
            .send()
            .await
            .map_err(|e| format!("version request to {url} failed: {e}"))?;
        let status = answer.status();
        let body: Value = answer.json().await.map_err(|e| {
            format!("version request to {url}: HTTP {status}, unreadable body: {e}")
        })?;
        if !status.is_success() {
            let why = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            return Err(format!("version request to {url}: HTTP {status}: {why}"));
        }
        Ok(version_in(&body))
    }

    /// Ask until the daemon names a version or the attempts are used up.
    async fn ask_until_answered(&self, window: &RunWindow) -> Result<Option<String>, String> {
        let client = reqwest::Client::new();
        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(self.pause).await;
            }
            if let Some(version) = self.ask(&client, window).await? {
                return Ok(Some(version));
            }
        }
        Ok(None)
    }
}

impl TargetVersionResolver for DaemonVersions {
    /// The runner calls this from synchronous code that may sit inside an
    /// async runtime, so the request runs on a thread of its own.
    fn resolve(&self, window: &RunWindow) -> Result<Option<String>, String> {
        let asked = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| format!("version request: no runtime: {e}"))?;
                    runtime.block_on(self.ask_until_answered(window))
                })
                .join()
        });
        asked.unwrap_or_else(|_| Err("version request panicked".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    use super::*;
    use tumult_core::types::ExperimentTarget;

    fn experiment(target: Option<&str>) -> Experiment {
        let mut toon = String::from("title: t\n");
        if let Some(version) = target {
            toon.push_str("target:\n  system: checkout\n  environment: demo\n");
            if !version.is_empty() {
                toon.push_str(&format!("  version: {version}\n"));
            }
        }
        tumult_core::engine::parse_experiment(&toon).unwrap()
    }

    fn declared(experiment: &Experiment) -> Option<&str> {
        experiment
            .target
            .as_ref()
            .and_then(ExperimentTarget::declared_version)
    }

    #[test]
    fn a_version_given_on_the_command_line_lands_on_the_target() {
        let mut bare = experiment(Some(""));
        declare(&mut bare, Some(" 1.5.0 ")).unwrap();
        assert_eq!(declared(&bare), Some("1.5.0"));
        // It wins over the file's.
        let mut filed = experiment(Some("1.4.0"));
        declare(&mut filed, Some("1.5.0")).unwrap();
        assert_eq!(declared(&filed), Some("1.5.0"));
        // Not given: the file is left as it is.
        let mut untouched = experiment(Some("1.4.0"));
        declare(&mut untouched, None).unwrap();
        assert_eq!(declared(&untouched), Some("1.4.0"));
        let mut targetless = experiment(None);
        declare(&mut targetless, None).unwrap();
        assert!(targetless.target.is_none());
    }

    #[test]
    fn declaring_a_version_needs_a_value_and_a_target() {
        let blank = declare(&mut experiment(Some("")), Some("  ")).unwrap_err();
        assert!(blank.to_string().contains("must name a version"), "{blank}");
        let targetless = declare(&mut experiment(None), Some("1.5.0")).unwrap_err();
        assert!(
            targetless.to_string().contains("no `target` block"),
            "{targetless}"
        );
    }

    #[test]
    fn the_daemon_is_asked_when_one_is_configured_else_a_store_that_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let lake = dir.path().join("lake.duckdb");
        let url = || Some("http://d:1".to_string());
        assert_eq!(source(None, Some(lake.clone())), None, "no store yet");
        std::fs::write(&lake, b"").unwrap();
        assert_eq!(
            source(None, Some(lake.clone())),
            Some(Source::Lake(lake.clone()))
        );
        assert_eq!(
            source(url(), Some(lake.clone())),
            Some(Source::Daemon("http://d:1".into()))
        );
        assert_eq!(
            source(Some("  ".into()), Some(lake.clone())),
            Some(Source::Lake(lake))
        );
        assert_eq!(source(None, None), None);
    }

    fn window(environment: Option<&str>) -> RunWindow {
        RunWindow {
            system: "check out".into(),
            environment: environment.map(str::to_string),
            started_at_ns: 10,
            ended_at_ns: 20,
        }
    }

    #[test]
    fn the_question_names_the_window_and_the_environment_of_the_run() {
        let (url, query) = request("http://d:1/", &window(Some("demo")));
        assert_eq!(url, "http://d:1/api/perf/check%20out/version");
        assert_eq!(
            query,
            vec![
                ("env", "demo".to_string()),
                ("from_ns", "10".to_string()),
                ("to_ns", "20".to_string()),
            ]
        );
        // No environment: the daemon reads the spans that name none.
        let (_, query) = request("http://d:1", &window(None));
        assert_eq!(
            query,
            vec![("from_ns", "10".to_string()), ("to_ns", "20".to_string())]
        );
    }

    #[test]
    fn an_answer_names_a_version_or_none() {
        let named = serde_json::json!({"target": "checkout", "version": " 1.5.0 "});
        assert_eq!(version_in(&named).as_deref(), Some("1.5.0"));
        assert_eq!(version_in(&serde_json::json!({"version": null})), None);
        assert_eq!(version_in(&serde_json::json!({"version": ""})), None);
        assert_eq!(version_in(&serde_json::json!({})), None);
    }

    /// A daemon that answers each request with the next canned response
    /// and keeps the request heads it was sent.
    struct Daemon {
        base: String,
        heads: Arc<Mutex<Vec<String>>>,
    }

    fn head_of(stream: &std::net::TcpStream) -> String {
        let mut head = String::new();
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).unwrap_or(0);
            if read == 0 || line == "\r\n" {
                return head;
            }
            head.push_str(&line);
        }
    }

    fn daemon(answers: Vec<(u16, &'static str)>) -> Daemon {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let heads = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&heads);
        std::thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                kept.lock().unwrap().push(head_of(&stream));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        Daemon { base, heads }
    }

    fn asking(daemon: &Daemon, token: Option<&str>) -> DaemonVersions {
        DaemonVersions {
            pause: Duration::from_millis(5),
            ..DaemonVersions::new(daemon.base.clone(), token.map(str::to_string))
        }
    }

    #[test]
    fn the_daemon_is_asked_again_until_the_target_has_spoken() {
        let daemon = daemon(vec![
            (200, r#"{"version":null}"#),
            (200, r#"{"version":"1.5.0"}"#),
            (200, r#"{"version":"9.9.9"}"#),
        ]);
        let versions = asking(&daemon, Some("kro_secret"));
        let seen = versions.resolve(&window(Some("demo")));
        assert_eq!(seen, Ok(Some("1.5.0".to_string())));
        let heads = daemon.heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 2, "{heads:?}");
        for head in &heads {
            assert!(
                head.starts_with(
                    "GET /api/perf/check%20out/version?env=demo&from_ns=10&to_ns=20 HTTP/1.1"
                ),
                "{head}"
            );
            assert!(
                head.to_lowercase()
                    .contains("authorization: bearer kro_secret"),
                "{head}"
            );
        }
    }

    #[test]
    fn a_target_that_stays_silent_costs_a_bounded_number_of_questions() {
        let null = r#"{"version":null}"#;
        let daemon = daemon(vec![(200, null), (200, null), (200, null), (200, null)]);
        let versions = asking(&daemon, None);
        assert_eq!(versions.resolve(&window(None)), Ok(None));
        let heads = daemon.heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 3, "{heads:?}");
        assert!(
            !heads[0].to_lowercase().contains("authorization"),
            "{heads:?}"
        );
    }

    #[test]
    fn a_refusal_or_a_dead_daemon_is_an_error_and_is_not_asked_again() {
        let daemon = daemon(vec![
            (
                403,
                r#"{"error":"environment demo is outside the principal's scopes"}"#,
            ),
            (200, r#"{"version":"1.5.0"}"#),
        ]);
        let error = asking(&daemon, None)
            .resolve(&window(Some("demo")))
            .unwrap_err();
        assert!(error.contains("403"), "{error}");
        assert!(error.contains("outside the principal's scopes"), "{error}");
        assert_eq!(daemon.heads.lock().unwrap().len(), 1);

        // Nothing listens here: the port was bound and released.
        let gone = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", gone.local_addr().unwrap());
        drop(gone);
        let dead = Daemon {
            base,
            heads: Arc::default(),
        };
        let error = asking(&dead, None).resolve(&window(None)).unwrap_err();
        assert!(error.contains("version request"), "{error}");
    }

    /// The runner calls the resolver from inside the CLI's async runtime.
    #[tokio::test]
    async fn the_question_can_be_asked_from_inside_a_runtime() {
        let daemon = daemon(vec![(200, r#"{"version":"1.5.0"}"#)]);
        let seen = asking(&daemon, None).resolve(&window(Some("demo")));
        assert_eq!(seen, Ok(Some("1.5.0".to_string())));
    }
}
