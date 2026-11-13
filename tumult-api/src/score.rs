//! Shared state of the DORA score endpoints.
//!
//! One [`ScoreRuntime`] per daemon holds what every score request needs
//! and what must not be read again on each of them:
//!
//! * the house parameters of the org file's `score:` section, with where
//!   they came from. An invalid file scores with the defaults, and every
//!   response says so (`params.fallback` with the reason) and lists each
//!   parameter that differs from the default, so a lowered gate is in
//!   front of the reader;
//! * the state of the function register: `ok` (the file is valid and
//!   written to the lake), `stale` (valid, not yet written), `invalid`
//!   (the lake keeps the previous register) or `none` (no org file);
//! * the workloads of the manifests directory, read when the runtime is
//!   built and on each reload, never per request;
//! * a short response cache. An entry is served for at most the TTL and
//!   only while the generation it was built under is current. A build
//!   that started before a reload or a stored snapshot is not kept.
//!
//! The daemon's score job calls [`ScoreRuntime::check`] on an interval,
//! writes the register it returns to the lake and reports back with
//! [`ScoreRuntime::synced`] or [`ScoreRuntime::sync_failed`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use serde_json::{json, Value};
use tumult_compliance::dora::model::Workload;
use tumult_compliance::dora::params::{HouseParams, Override};
use tumult_compliance::dora::register;
use tumult_core::compliance::truncated;
use tumult_lake::IctRegister;

/// Environment variable setting how long a score response is reused, in
/// seconds; `0` turns the cache off.
pub const CACHE_TTL_ENV: &str = "KRONIKA_SCORE_CACHE_TTL";
/// A score is at most this far behind the lake.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(5);
/// Most responses kept; past it the cache is emptied.
const CACHE_MAX_ENTRIES: usize = 256;
/// Most bytes of a reason or a manifest note in one log event.
const LOG_VALUE_MAX_LEN: usize = 512;

const SOURCE_DEFAULTS: &str = "defaults";
const SOURCE_ORG_FILE: &str = "org_file";
const NOT_WRITTEN: &str = "the org file is valid and not yet written to the lake";

/// Where the function register stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterState {
    /// No org file is configured, or there is none at the path.
    None,
    /// The file is valid and the lake holds it.
    Ok,
    /// The file is valid and the lake does not hold it yet.
    Stale,
    /// The file is refused; the lake keeps the register it had.
    Invalid,
}

impl RegisterState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ok => "ok",
            Self::Stale => "stale",
            Self::Invalid => "invalid",
        }
    }
}

struct Org {
    params: Arc<HouseParams>,
    source: &'static str,
    fallback: bool,
    reason: Option<String>,
    overridden: Vec<Override>,
    register: RegisterState,
    register_reason: Option<String>,
    checked_at_ns: Option<i64>,
    synced_at_ns: Option<i64>,
    /// The file as last read.
    seen: Option<Vec<u8>>,
    /// The file as last written to the lake.
    synced: Option<Vec<u8>>,
    /// The register waiting to be written.
    pending: Option<IctRegister>,
}

impl Org {
    fn defaults() -> Self {
        Self {
            params: Arc::default(),
            source: SOURCE_DEFAULTS,
            fallback: false,
            reason: None,
            overridden: Vec::new(),
            register: RegisterState::None,
            register_reason: Some("no org file".to_string()),
            checked_at_ns: None,
            synced_at_ns: None,
            seen: None,
            synced: None,
            pending: None,
        }
    }

    fn with_params(params: HouseParams) -> Self {
        Self {
            overridden: params.overrides(),
            params: Arc::new(params),
            ..Self::defaults()
        }
    }

    fn use_defaults(&mut self, fallback: Option<String>) {
        self.params = Arc::default();
        self.source = SOURCE_DEFAULTS;
        self.fallback = fallback.is_some();
        self.reason = fallback;
        self.overridden = Vec::new();
    }

    fn absent(&mut self) {
        self.use_defaults(None);
        self.register = RegisterState::None;
        self.register_reason = Some("no org file".to_string());
        self.seen = None;
        self.pending = None;
    }

    fn refused(&mut self, content: Option<Vec<u8>>, reason: String) {
        self.use_defaults(Some(reason.clone()));
        self.register = RegisterState::Invalid;
        self.register_reason = Some(reason);
        self.seen = content;
        self.pending = None;
    }

    fn accepted(&mut self, content: Vec<u8>, declared: register::Register) {
        self.overridden = declared.params.overrides();
        self.params = Arc::new(declared.params);
        self.source = SOURCE_ORG_FILE;
        self.fallback = false;
        self.reason = None;
        let written = self.synced.as_ref() == Some(&content);
        self.seen = Some(content);
        if written {
            self.register = RegisterState::Ok;
            self.register_reason = None;
            self.pending = None;
            return;
        }
        self.register = RegisterState::Stale;
        self.register_reason = Some(NOT_WRITTEN.to_string());
        self.pending = Some(declared.ict);
    }
}

#[derive(Default)]
struct Manifests {
    workloads: Arc<Vec<Workload>>,
    notes: Vec<String>,
    error: Option<String>,
}

struct Entry {
    generation: u64,
    stored_at_ns: i64,
    value: Value,
}

/// See the module documentation.
pub struct ScoreRuntime {
    org_file: Option<PathBuf>,
    manifests_dir: Option<PathBuf>,
    org: RwLock<Org>,
    manifests: RwLock<Manifests>,
    generation: AtomicU64,
    cache: Mutex<HashMap<String, Entry>>,
    ttl_ns: i64,
}

impl Default for ScoreRuntime {
    fn default() -> Self {
        Self::build(None, None, Org::defaults(), DEFAULT_CACHE_TTL)
    }
}

fn ttl_ns(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_nanos()).unwrap_or(i64::MAX)
}

/// The cache TTL: `KRONIKA_SCORE_CACHE_TTL` in whole seconds, the default
/// when it is unset or not a number.
#[must_use]
pub fn cache_ttl_from_env() -> Duration {
    let Ok(raw) = std::env::var(CACHE_TTL_ENV) else {
        return DEFAULT_CACHE_TTL;
    };
    let Ok(seconds) = raw.trim().parse::<u64>() else {
        tracing::warn!(
            "config.key" = CACHE_TTL_ENV,
            "config.value" = ?truncated(&raw, LOG_VALUE_MAX_LEN),
            "invalid cache TTL; using the default"
        );
        return DEFAULT_CACHE_TTL;
    };
    Duration::from_secs(seconds)
}

/// Said once per refused content, not on every reload.
fn log_refused(reason: &str) {
    tracing::warn!(
        "error.type" = "org_file_refused",
        "error.message" = ?truncated(reason, LOG_VALUE_MAX_LEN),
        "org file refused; scoring with the default house parameters, the lake keeps its register"
    );
}

/// The notes of `current` that `previous` did not already have.
fn new_notes<'a>(previous: &[String], current: &'a [String]) -> Vec<&'a String> {
    current
        .iter()
        .filter(|note| !previous.contains(note))
        .collect()
}

impl ScoreRuntime {
    fn build(
        org_file: Option<PathBuf>,
        manifests_dir: Option<PathBuf>,
        org: Org,
        ttl: Duration,
    ) -> Self {
        Self {
            org_file,
            manifests_dir,
            org: RwLock::new(org),
            manifests: RwLock::default(),
            generation: AtomicU64::new(0),
            cache: Mutex::default(),
            ttl_ns: ttl_ns(ttl),
        }
    }

    /// A runtime that scores with `params` and has no org file.
    #[must_use]
    pub fn with_params(params: HouseParams) -> Self {
        Self::build(None, None, Org::with_params(params), DEFAULT_CACHE_TTL)
    }

    /// A runtime over the org file and the manifests directory, both read
    /// once here.
    #[must_use]
    pub fn open(
        org_file: Option<PathBuf>,
        manifests_dir: Option<PathBuf>,
        ttl: Duration,
        now_ns: i64,
    ) -> Self {
        let runtime = Self::build(org_file, manifests_dir, Org::defaults(), ttl);
        runtime.check(now_ns);
        runtime.refresh_workloads();
        runtime
    }

    /// The same runtime with another cache TTL.
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl_ns = ttl_ns(ttl);
        self
    }

    /// The org file this runtime reads, when one is configured.
    #[must_use]
    pub fn org_file(&self) -> Option<&Path> {
        self.org_file.as_deref()
    }

    fn org(&self) -> std::sync::RwLockReadGuard<'_, Org> {
        self.org.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn org_mut(&self) -> std::sync::RwLockWriteGuard<'_, Org> {
        self.org.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// The house parameters in force.
    #[must_use]
    pub fn params(&self) -> Arc<HouseParams> {
        Arc::clone(&self.org().params)
    }

    /// Where the register stands.
    #[must_use]
    pub fn register_state(&self) -> RegisterState {
        self.org().register
    }

    /// Read the org file again. Returns the register to write to the lake
    /// when the file is valid and the lake does not hold it; the caller
    /// answers with [`Self::synced`] or [`Self::sync_failed`].
    ///
    /// An invalid file leaves the lake alone, sets the state to `invalid`
    /// and the parameters to the defaults, flagged as a fallback.
    pub fn check(&self, now_ns: i64) -> Option<IctRegister> {
        let path = self.org_file.as_deref()?;
        let read = std::fs::read(path);
        let mut org = self.org_mut();
        org.checked_at_ns = Some(now_ns);
        let content = match read {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if org.register != RegisterState::None {
                    org.absent();
                    self.invalidate();
                }
                return None;
            }
            Err(e) => {
                let reason = format!("cannot read the org file: {e}");
                let known = org.register == RegisterState::Invalid
                    && org.seen.is_none()
                    && org.register_reason.as_deref() == Some(reason.as_str());
                if !known {
                    log_refused(&reason);
                    org.refused(None, reason);
                    self.invalidate();
                }
                return None;
            }
        };
        if org.seen.as_ref() == Some(&content) {
            return org.pending.clone();
        }
        let parsed = String::from_utf8(content.clone())
            .map_err(|_| "the org file is not UTF-8".to_string())
            .and_then(|text| register::parse(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(declared) => org.accepted(content, declared),
            Err(reason) => {
                log_refused(&reason);
                org.refused(Some(content), reason);
            }
        }
        self.invalidate();
        org.pending.clone()
    }

    /// The register [`Self::check`] returned is in the lake.
    pub fn synced(&self, now_ns: i64) {
        let mut org = self.org_mut();
        if org.register != RegisterState::Stale {
            return;
        }
        org.synced = org.seen.clone();
        org.synced_at_ns = Some(now_ns);
        org.register = RegisterState::Ok;
        org.register_reason = None;
        org.pending = None;
        self.invalidate();
    }

    /// Writing the register failed; it stays `stale` and is offered again
    /// on the next [`Self::check`].
    pub fn sync_failed(&self, reason: &str) {
        let mut org = self.org_mut();
        if org.register != RegisterState::Stale {
            return;
        }
        org.register_reason = Some(format!("{NOT_WRITTEN}: {reason}"));
        self.invalidate();
    }

    /// The parameters in force together with what a document built with
    /// them says about them, read in one step: a reload between the two
    /// can not make a document name parameters it was not computed with.
    #[must_use]
    pub fn view(&self) -> ScoreView {
        let org = self.org();
        ScoreView {
            params: Arc::clone(&org.params),
            stamp: Stamp {
                params: json!({
                    "source": org.source,
                    "fallback": org.fallback,
                    "reason": org.reason,
                    "overridden": org.overridden,
                }),
                register_state: org.register.as_str(),
                register_reason: org.register_reason.clone(),
                checked_at_ns: org.checked_at_ns,
                synced_at_ns: org.synced_at_ns,
            },
        }
    }

    /// Add the `params` block and the register state as they stand now.
    /// A document that was computed is stamped from the [`Self::view`] it
    /// was computed with instead.
    pub fn stamp(&self, doc: &mut Value) {
        self.view().stamp.apply(doc);
    }

    /// The workloads of the manifests directory, as last read.
    ///
    /// # Errors
    /// Returns the message of the last read when it failed.
    pub fn workloads(&self) -> Result<Arc<Vec<Workload>>, String> {
        let manifests = self
            .manifests
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        match &manifests.error {
            Some(error) => Err(error.clone()),
            None => Ok(Arc::clone(&manifests.workloads)),
        }
    }

    /// Read the manifests directory again. A skipped document is logged
    /// the first time it is seen, not on every read.
    pub fn refresh_workloads(&self) {
        let read = crate::handlers::dora::read_manifests(self.manifests_dir.as_deref());
        let mut manifests = self
            .manifests
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let (workloads, notes, error) = match read {
            Ok((workloads, notes)) => (workloads, notes, None),
            Err(error) => (Vec::new(), Vec::new(), Some(error)),
        };
        for note in new_notes(&manifests.notes, &notes) {
            tracing::warn!(
                "manifest.note" = ?truncated(note, LOG_VALUE_MAX_LEN),
                "manifest document skipped"
            );
        }
        if let Some(message) = error
            .as_ref()
            .filter(|e| manifests.error.as_ref() != Some(*e))
        {
            tracing::warn!(
                "error.type" = "manifests_unreadable",
                "error.message" = ?truncated(message, LOG_VALUE_MAX_LEN),
                "manifests directory not read"
            );
        }
        let changed = *manifests.workloads != workloads || manifests.error != error;
        manifests.workloads = Arc::new(workloads);
        manifests.notes = notes;
        manifests.error = error;
        if changed {
            self.invalidate();
        }
    }

    /// The generation responses are built under. Read it before a build
    /// and hand it to [`Self::store`].
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// What the lake or the configuration says has changed: no response
    /// built before this call is served or stored after it.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The response stored for `key`, while it is fresh and current.
    #[must_use]
    pub fn cached(&self, key: &str, now_ns: i64) -> Option<Value> {
        let cache = self.cache();
        let entry = cache.get(key)?;
        let fresh = now_ns.saturating_sub(entry.stored_at_ns) < self.ttl_ns;
        (fresh && entry.generation == self.generation()).then(|| entry.value.clone())
    }

    /// Keep `value` for `key`, unless the generation moved on since the
    /// build read it: such a response may describe the state before the
    /// change and is dropped.
    pub fn store(&self, key: String, generation: u64, value: &Value, now_ns: i64) {
        if self.ttl_ns == 0 {
            return;
        }
        let mut cache = self.cache();
        if generation != self.generation() {
            return;
        }
        if cache.len() >= CACHE_MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(
            key,
            Entry {
                generation,
                stored_at_ns: now_ns,
                value: value.clone(),
            },
        );
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache().len()
    }
}

/// The house parameters and the stamp that names them.
#[derive(Debug, Clone)]
pub struct ScoreView {
    /// The parameters a document is computed with.
    pub params: Arc<HouseParams>,
    /// What that document says about them and about the register.
    pub stamp: Stamp,
}

/// The `params` block and the register state of one score document.
#[derive(Debug, Clone)]
pub struct Stamp {
    params: Value,
    register_state: &'static str,
    register_reason: Option<String>,
    checked_at_ns: Option<i64>,
    synced_at_ns: Option<i64>,
}

impl Stamp {
    /// Add the blocks to a score document.
    pub fn apply(&self, doc: &mut Value) {
        let Some(map) = doc.as_object_mut() else {
            return;
        };
        map.insert("params".into(), self.params.clone());
        map.insert("register_state".into(), json!(self.register_state));
        map.insert("register_reason".into(), json!(self.register_reason));
        map.insert("register_checked_at_ns".into(), json!(self.checked_at_ns));
        map.insert("register_synced_at_ns".into(), json!(self.synced_at_ns));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_000_000_000;
    const REGISTER: &str = "
legal_entities:
  - {lei: 529900T8BM49AURSDO55, name: Demo Firm, profile: simplified}
functions:
  - {id: cif-0001, name: Payments, tier: critical, services: [payments-api]}
";
    const DEPLOYMENT: &str = "
apiVersion: apps/v1
kind: Deployment
metadata: {name: checkout, namespace: shop, labels: {app: checkout}}
spec:
  replicas: 1
  template:
    spec:
      containers: [{name: app, image: example/checkout}]
";

    struct Files {
        dir: tempfile::TempDir,
    }

    impl Files {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn org(&self) -> PathBuf {
            self.dir.path().join("org.yaml")
        }

        fn write_org(&self, text: &str) {
            std::fs::write(self.org(), text).unwrap();
        }

        fn runtime(&self) -> ScoreRuntime {
            ScoreRuntime::open(Some(self.org()), None, DEFAULT_CACHE_TTL, 1)
        }
    }

    fn stamped(runtime: &ScoreRuntime) -> Value {
        let mut doc = json!({});
        runtime.stamp(&mut doc);
        doc
    }

    /// A refused `score:` section must not pass for the house's choice:
    /// the defaults are in force and the response says why.
    #[test]
    fn an_invalid_score_section_scores_with_the_defaults_and_says_why() {
        let files = Files::new();
        files.write_org(&format!("{REGISTER}score:\n  band_good: 7\n"));
        let runtime = files.runtime();
        assert_eq!(*runtime.params(), HouseParams::default());
        let doc = stamped(&runtime);
        assert_eq!(doc["params"]["source"], "defaults");
        assert_eq!(doc["params"]["fallback"], true);
        let reason = doc["params"]["reason"].as_str().expect("a reason");
        assert!(reason.contains("band_good"), "{reason}");
        assert_eq!(doc["params"]["overridden"], json!([]));
        assert_eq!(doc["register_state"], "invalid");
        assert_eq!(doc["register_reason"], reason);
        assert_eq!(runtime.check(2), None, "nothing to write to the lake");
    }

    /// A lowered gate is the house's right, and the reader sees it.
    #[test]
    fn every_overridden_parameter_is_listed_with_its_value_and_the_default() {
        let files = Files::new();
        files.write_org(&format!("{REGISTER}score:\n  band_good: 0.6\n"));
        let runtime = files.runtime();
        assert_eq!(runtime.params().band_good, 0.6);
        let doc = stamped(&runtime);
        assert_eq!(doc["params"]["source"], "org_file");
        assert_eq!(doc["params"]["fallback"], false);
        assert_eq!(doc["params"]["reason"], Value::Null);
        let default = HouseParams::default().band_good;
        assert_eq!(
            doc["params"]["overridden"],
            json!([{"key": "band_good", "value": 0.6, "default": default}])
        );
    }

    /// A request that started before a reload computes with the old
    /// parameters; its document must name those, not the new ones.
    #[test]
    fn a_document_names_the_parameters_it_was_computed_with() {
        let files = Files::new();
        files.write_org(REGISTER);
        let runtime = files.runtime();
        let view = runtime.view();
        files.write_org(&format!("{REGISTER}score:\n  operating_days: 30\n"));
        runtime.check(2);
        assert_eq!(runtime.params().operating_days, 30, "the reload is in");
        assert_eq!(view.params.operating_days, 180);
        let mut doc = json!({});
        view.stamp.apply(&mut doc);
        assert_eq!(doc["params"]["overridden"], json!([]));
        assert_eq!(
            stamped(&runtime)["params"]["overridden"][0]["key"],
            "operating_days"
        );
    }

    #[test]
    fn no_org_file_means_the_defaults_and_no_register() {
        let files = Files::new();
        let runtime = files.runtime();
        let doc = stamped(&runtime);
        assert_eq!(doc["params"]["source"], "defaults");
        assert_eq!(doc["params"]["fallback"], false);
        assert_eq!(doc["register_state"], "none");
        assert_eq!(runtime.check(2), None);
        let plain = stamped(&ScoreRuntime::default());
        assert_eq!(plain["register_state"], "none");
    }

    #[test]
    fn the_register_is_stale_until_written_and_follows_the_file_after_that() {
        let files = Files::new();
        files.write_org(REGISTER);
        let runtime = files.runtime();
        assert_eq!(runtime.register_state(), RegisterState::Stale);
        assert_eq!(stamped(&runtime)["register_reason"], NOT_WRITTEN);

        // The job is offered the register until it reports it written.
        let offered = runtime.check(2).expect("a register to write");
        assert_eq!(offered.functions.len(), 1);
        runtime.sync_failed("disk full");
        assert_eq!(runtime.register_state(), RegisterState::Stale);
        let reason = stamped(&runtime)["register_reason"].to_string();
        assert!(reason.contains("disk full"), "{reason}");
        assert!(runtime.check(3).is_some(), "offered again");
        runtime.synced(4);
        assert_eq!(runtime.register_state(), RegisterState::Ok);
        let doc = stamped(&runtime);
        assert_eq!(doc["register_reason"], Value::Null);
        assert_eq!(doc["register_synced_at_ns"], 4);
        assert_eq!(runtime.check(5), None, "an unchanged file is not rewritten");
        assert_eq!(stamped(&runtime)["register_checked_at_ns"], 5);

        // The file changes: stale again, with the new register on offer.
        let two = format!("{REGISTER}  - {{id: cif-0002, name: Reporting, tier: important}}\n");
        files.write_org(&two);
        let offered = runtime.check(6).expect("the changed register");
        assert_eq!(offered.functions.len(), 2);
        assert_eq!(runtime.register_state(), RegisterState::Stale);
        runtime.synced(7);

        // The file breaks: invalid, nothing offered, defaults flagged.
        files.write_org("functions: [{id: NOT-AN-ID}]\n");
        assert_eq!(runtime.check(8), None);
        assert_eq!(runtime.register_state(), RegisterState::Invalid);
        assert_eq!(stamped(&runtime)["params"]["fallback"], true);

        // Repaired to what the lake already holds: ok without a write.
        files.write_org(&two);
        assert_eq!(runtime.check(9), None);
        assert_eq!(runtime.register_state(), RegisterState::Ok);

        // Removed: no register, no fallback.
        std::fs::remove_file(files.org()).unwrap();
        assert_eq!(runtime.check(10), None);
        assert_eq!(runtime.register_state(), RegisterState::None);
        assert_eq!(stamped(&runtime)["params"]["fallback"], false);
    }

    #[test]
    fn a_response_is_reused_only_while_fresh() {
        let runtime = ScoreRuntime::default();
        let generation = runtime.generation();
        assert_eq!(runtime.cached("k", 0), None);
        runtime.store("k".into(), generation, &json!({"level": 2}), 10 * SECOND);
        assert_eq!(runtime.cached("k", 10 * SECOND), Some(json!({"level": 2})));
        assert_eq!(
            runtime.cached("k", 15 * SECOND - 1),
            Some(json!({"level": 2}))
        );
        assert_eq!(runtime.cached("k", 15 * SECOND), None, "past the TTL");
        assert_eq!(runtime.cached("other", 10 * SECOND), None);
    }

    /// The race the generation exists for: a build reads the lake, the
    /// register is reloaded, then the build finishes. Its result
    /// describes the old register and must not be kept.
    #[test]
    fn a_build_that_started_before_an_invalidation_is_not_stored() {
        let runtime = ScoreRuntime::default();
        let started_under = runtime.generation();
        runtime.invalidate();
        runtime.store("k".into(), started_under, &json!({"level": 5}), SECOND);
        assert_eq!(runtime.cached("k", SECOND), None);

        // And one stored before the invalidation is no longer served.
        let generation = runtime.generation();
        runtime.store("k".into(), generation, &json!({"level": 2}), SECOND);
        assert!(runtime.cached("k", SECOND).is_some());
        runtime.invalidate();
        assert_eq!(runtime.cached("k", SECOND), None);
    }

    #[test]
    fn every_change_of_state_starts_a_new_generation() {
        let files = Files::new();
        files.write_org(REGISTER);
        let runtime = files.runtime();
        let mut last = runtime.generation();
        let mut moved = |what: &str| {
            let now = runtime.generation();
            assert!(now > last, "{what} did not invalidate");
            last = now;
        };
        runtime.sync_failed("disk full");
        moved("a failed write");
        runtime.synced(2);
        moved("a written register");
        files.write_org(&format!("{REGISTER}score:\n  band_good: 0.6\n"));
        runtime.check(3);
        moved("a changed file");
        runtime.check(4);
        assert_eq!(
            runtime.generation(),
            last,
            "an unchanged file invalidates nothing"
        );
    }

    #[test]
    fn the_cache_is_off_at_ttl_zero_and_never_grows_past_its_bound() {
        let off = ScoreRuntime::default().with_ttl(Duration::ZERO);
        off.store("k".into(), off.generation(), &json!(1), SECOND);
        assert_eq!(off.cached("k", SECOND), None);
        assert_eq!(off.cache_len(), 0);

        let runtime = ScoreRuntime::default();
        for n in 0..(CACHE_MAX_ENTRIES * 2 + 10) {
            runtime.store(format!("k{n}"), runtime.generation(), &json!(n), SECOND);
            assert!(runtime.cache_len() <= CACHE_MAX_ENTRIES);
        }
    }

    /// The manifests are read when the runtime is built and on a
    /// refresh, never by a request.
    #[test]
    fn workloads_are_read_once_and_again_only_on_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.yaml");
        std::fs::write(&file, DEPLOYMENT).unwrap();
        let runtime =
            ScoreRuntime::open(None, Some(dir.path().to_path_buf()), DEFAULT_CACHE_TTL, 1);
        assert_eq!(runtime.workloads().unwrap().len(), 1);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(runtime.workloads().unwrap().len(), 1, "no read per request");
        let before = runtime.generation();
        runtime.refresh_workloads();
        assert_eq!(runtime.workloads().unwrap().len(), 0);
        assert!(runtime.generation() > before);
        runtime.refresh_workloads();
        assert_eq!(runtime.generation(), before + 1, "nothing changed");

        let missing =
            ScoreRuntime::open(None, Some(dir.path().join("nowhere")), DEFAULT_CACHE_TTL, 1);
        assert!(missing.workloads().is_err());
    }

    #[test]
    fn a_skipped_manifest_is_reported_the_first_time_only() {
        let first = vec!["a.yaml: not a mapping".to_string()];
        let second = vec![
            "a.yaml: not a mapping".to_string(),
            "b.yaml: unreadable".to_string(),
        ];
        assert_eq!(new_notes(&[], &first), [&first[0]]);
        assert_eq!(new_notes(&first, &first), Vec::<&String>::new());
        assert_eq!(new_notes(&first, &second), [&second[1]]);
    }
}
