//! Which version of the target a run measured.
//!
//! `--target-version` declares it on the experiment's target. Left
//! undeclared, the runner asks, when the run ends, whoever holds the
//! target's telemetry: the daemon named by `TUMULT_DAEMON_URL`
//! (`GET /api/perf/{target}/version`), or else the local store. It asks
//! again until the count of requests has settled, and freezes the answer
//! in the journal: the version that served nearly all of the window, or
//! `mixed` with the shares. No answer leaves the run without a version,
//! and it is then read from the target's spans when a verdict is asked
//! for.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;
use tumult_core::runner::{RunWindow, TargetVersionResolver, VersionSettle};
use tumult_core::types::{check_version_text, Experiment, VersionShare};

/// Env var naming the daemon base URL (shared with `tumult gate`).
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";
/// How long one question may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Put a version given on the command line on the experiment's target,
/// over one the file declares.
///
/// # Errors
///
/// Returns an error when the version is blank, longer than
/// [`tumult_core::types::MAX_VERSION_LEN`] bytes or holds a control
/// character, or when the experiment declares no target to attach it to.
pub(crate) fn declare(experiment: &mut Experiment, version: Option<&str>) -> Result<()> {
    let Some(version) = version else {
        return Ok(());
    };
    let version = version.trim();
    if version.is_empty() {
        bail!("--target-version must name a version");
    }
    if let Err(e) = tumult_core::types::check_version_text(version) {
        bail!("--target-version: {e}");
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

/// The daemon when a URL is configured, whether or not the run is
/// ingested: the daemon holds the target's telemetry either way. Else the
/// local store, when there is one and the run reports to it. Else nobody.
/// Pure but for the existence check.
pub(crate) fn source(
    daemon_url: Option<String>,
    lake: Option<PathBuf>,
    ingests: bool,
) -> Option<Source> {
    if let Some(url) = daemon_url.filter(|url| !url.trim().is_empty()) {
        return Some(Source::Daemon(url));
    }
    lake.filter(|path| ingests && path.exists())
        .map(Source::Lake)
}

/// The resolver for this process's environment and a run that is, or is
/// not, ingested into the local store.
pub(crate) fn resolver(ingests: bool) -> Option<Arc<dyn TargetVersionResolver>> {
    let lake = tumult_lake::AnalyticsStore::default_path().ok();
    match source(std::env::var(DAEMON_URL_ENV).ok(), lake, ingests)? {
        Source::Daemon(base) => {
            let token = std::env::var(DAEMON_TOKEN_ENV).ok();
            Some(Arc::new(DaemonVersions::new(base, token)))
        }
        Source::Lake(path) => Some(Arc::new(tumult_lake::StoredVersions::at(&path))),
    }
}

/// What to tell the operator when a run measures a target whose version
/// is neither declared nor going to be observed.
pub(crate) fn unobserved_notice(experiment: &Experiment, observed: bool) -> Option<String> {
    let target = experiment.target.as_ref()?;
    if observed || target.declared_version().is_some() {
        return None;
    }
    Some(format!(
        "notice: the version of {} this run measures is not declared and nobody is asked for \
         it (no {DAEMON_URL_ENV}, and no local store the run reports to); declare it with \
         --target-version or the run is attributed from the target's spans later, if at all",
        target.system
    ))
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
    if let Some(namespace) = &window.namespace {
        query.push(("namespace", namespace.clone()));
    }
    query.push(("from_ns", window.started_at_ns.to_string()));
    query.push(("to_ns", window.ended_at_ns.to_string()));
    (url, query)
}

/// The versions in the daemon's answer with the requests each served.
/// An entry that is not a version within the bounds is left out.
pub(crate) fn shares_in(body: &Value) -> Vec<VersionShare> {
    let Some(versions) = body.get("versions").and_then(Value::as_array) else {
        return Vec::new();
    };
    versions
        .iter()
        .filter_map(|entry| {
            let version = entry.get("version")?.as_str()?.trim();
            let requests = entry.get("requests")?.as_u64()?;
            let named = !version.is_empty() && check_version_text(version).is_ok();
            named.then(|| VersionShare {
                version: version.to_string(),
                requests,
            })
        })
        .collect()
}

/// Asks a daemon which version served a run.
#[derive(Debug, Clone)]
pub(crate) struct DaemonVersions {
    base: String,
    token: Option<String>,
    settle: VersionSettle,
}

impl DaemonVersions {
    pub(crate) fn new(base: String, token: Option<String>) -> Self {
        Self {
            base,
            token: token.filter(|t| !t.is_empty()),
            settle: VersionSettle::default(),
        }
    }

    /// One question.
    async fn ask(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String> {
        let (url, query) = request(&self.base, window);
        let mut asked = reqwest::Client::new()
            .get(&url)
            .query(&query)
            .timeout(TIMEOUT);
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
        Ok(shares_in(&body))
    }
}

impl TargetVersionResolver for DaemonVersions {
    /// The runner calls this from synchronous code that may sit inside an
    /// async runtime, so the request runs on a thread of its own.
    fn resolve(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String> {
        let asked = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| format!("version request: no runtime: {e}"))?;
                    runtime.block_on(self.ask(window))
                })
                .join()
        });
        asked.unwrap_or_else(|_| Err("version request panicked".to_string()))
    }

    fn settle(&self) -> VersionSettle {
        self.settle
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
    fn a_version_on_the_command_line_is_held_to_the_bounds() {
        let long = "9".repeat(tumult_core::types::MAX_VERSION_LEN + 1);
        let refused = declare(&mut experiment(Some("")), Some(&long)).unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("--target-version: a version is at most 128 bytes"),
            "{refused}"
        );
        let mut filed = experiment(Some("1.4.0"));
        let refused = declare(&mut filed, Some("1.5.0\u{7}")).unwrap_err();
        assert!(
            refused.to_string().contains("no control characters"),
            "{refused}"
        );
        assert_eq!(declared(&filed), Some("1.4.0"), "the file's is left alone");
    }

    #[test]
    fn the_daemon_is_asked_when_one_is_configured_else_a_store_that_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let lake = dir.path().join("lake.duckdb");
        let url = || Some("http://d:1".to_string());
        assert_eq!(source(None, Some(lake.clone()), true), None, "no store yet");
        std::fs::write(&lake, b"").unwrap();
        assert_eq!(
            source(None, Some(lake.clone()), true),
            Some(Source::Lake(lake.clone()))
        );
        assert_eq!(
            source(url(), Some(lake.clone()), true),
            Some(Source::Daemon("http://d:1".into()))
        );
        assert_eq!(
            source(Some("  ".into()), Some(lake.clone()), true),
            Some(Source::Lake(lake))
        );
        assert_eq!(source(None, None, true), None);
    }

    #[test]
    fn a_run_that_does_not_ingest_still_asks_a_daemon_and_never_the_local_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let lake = dir.path().join("lake.duckdb");
        std::fs::write(&lake, b"").unwrap();
        assert_eq!(
            source(Some("http://d:1".into()), Some(lake.clone()), false),
            Some(Source::Daemon("http://d:1".into()))
        );
        assert_eq!(source(None, Some(lake), false), None);
    }

    #[test]
    fn a_run_whose_version_nobody_will_observe_says_so() {
        let bare = experiment(Some(""));
        let notice = unobserved_notice(&bare, false).unwrap();
        assert!(notice.contains("checkout"), "{notice}");
        assert!(notice.contains("--target-version"), "{notice}");
        assert!(notice.contains("TUMULT_DAEMON_URL"), "{notice}");
        // Somebody is asked, the version is declared, or there is no target.
        assert_eq!(unobserved_notice(&bare, true), None);
        assert_eq!(unobserved_notice(&experiment(Some("1.4.0")), false), None);
        assert_eq!(unobserved_notice(&experiment(None), false), None);
    }

    fn window(environment: Option<&str>) -> RunWindow {
        RunWindow {
            system: "check out".into(),
            environment: environment.map(str::to_string),
            namespace: None,
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
    fn the_question_names_the_namespace_when_the_target_has_one() {
        let shop = RunWindow {
            namespace: Some("shop".into()),
            ..window(Some("demo"))
        };
        let (_, query) = request("http://d:1", &shop);
        assert_eq!(
            query,
            vec![
                ("env", "demo".to_string()),
                ("namespace", "shop".to_string()),
                ("from_ns", "10".to_string()),
                ("to_ns", "20".to_string()),
            ]
        );
    }

    fn share(version: &str, requests: u64) -> VersionShare {
        VersionShare {
            version: version.into(),
            requests,
        }
    }

    #[test]
    fn an_answer_lists_the_versions_with_their_requests() {
        let two = serde_json::json!({"version": null, "version_source": "mixed", "versions": [
            {"version": " 1.5.0 ", "requests": 12, "share": 0.6},
            {"version": "1.4.0", "requests": 8, "share": 0.4},
        ]});
        assert_eq!(shares_in(&two), vec![share("1.5.0", 12), share("1.4.0", 8)]);
        assert_eq!(shares_in(&serde_json::json!({"versions": []})), vec![]);
        assert_eq!(shares_in(&serde_json::json!({"version": "1.5.0"})), vec![]);
        // What is not a version within the bounds is not taken.
        let long = "9".repeat(tumult_core::types::MAX_VERSION_LEN + 1);
        let odd = serde_json::json!({"versions": [
            {"version": long, "requests": 5},
            {"version": "", "requests": 5},
            {"version": "1.5.0\u{7}", "requests": 5},
            {"version": "1.5.0"},
            {"version": "1.4.0", "requests": 1},
        ]});
        assert_eq!(shares_in(&odd), vec![share("1.4.0", 1)]);
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
            settle: VersionSettle {
                pause: Duration::from_millis(5),
                deadline: Duration::from_secs(2),
            },
            ..DaemonVersions::new(daemon.base.clone(), token.map(str::to_string))
        }
    }

    const NOTHING: &str = r#"{"version":null,"versions":[]}"#;
    const HALF: &str = r#"{"versions":[{"version":"1.5.0","requests":9}]}"#;
    const ALL: &str = r#"{"versions":[{"version":"1.5.0","requests":19}]}"#;

    #[test]
    fn one_question_is_one_request_with_the_token() {
        let daemon = daemon(vec![(200, ALL), (200, ALL)]);
        let versions = asking(&daemon, Some("kro_secret"));
        let seen = versions.resolve(&window(Some("demo")));
        assert_eq!(seen, Ok(vec![share("1.5.0", 19)]));
        let heads = daemon.heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 1, "{heads:?}");
        assert!(
            heads[0].starts_with(
                "GET /api/perf/check%20out/version?env=demo&from_ns=10&to_ns=20 HTTP/1.1"
            ),
            "{heads:?}"
        );
        assert!(
            heads[0]
                .to_lowercase()
                .contains("authorization: bearer kro_secret"),
            "{heads:?}"
        );
    }

    #[test]
    fn the_daemon_is_asked_again_until_its_count_has_settled() {
        let daemon = daemon(vec![
            (200, NOTHING),
            (200, HALF),
            (200, ALL),
            (200, ALL),
            (200, HALF),
        ]);
        let versions = asking(&daemon, None);
        let seen = tumult_core::runner::settled_versions(&versions, &window(None));
        assert_eq!(seen, Ok(vec![share("1.5.0", 19)]));
        let heads = daemon.heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 4, "{heads:?}");
        assert!(
            !heads[0].to_lowercase().contains("authorization"),
            "{heads:?}"
        );
    }

    #[test]
    fn a_target_that_stays_silent_is_asked_twice_and_has_no_version() {
        let daemon = daemon(vec![(200, NOTHING), (200, NOTHING), (200, ALL)]);
        let versions = asking(&daemon, None);
        let seen = tumult_core::runner::settled_versions(&versions, &window(None));
        assert_eq!(seen, Ok(vec![]));
        assert_eq!(daemon.heads.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_refusal_or_a_dead_daemon_is_an_error_and_is_not_asked_again() {
        let daemon = daemon(vec![
            (
                403,
                r#"{"error":"environment demo is outside the principal's scopes"}"#,
            ),
            (200, ALL),
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
        let daemon = daemon(vec![(200, ALL)]);
        let seen = asking(&daemon, None).resolve(&window(Some("demo")));
        assert_eq!(seen, Ok(vec![share("1.5.0", 19)]));
    }
}
