//! Journal writer — serializes experiment results to TOON format.

use std::path::Path;

use crate::types::Journal;

use thiserror::Error;

/// Errors produced while encoding, writing, or reading a journal.
#[derive(Error, Debug)]
pub enum JournalError {
    /// The journal could not be serialized to TOON.
    #[error("failed to encode journal to TOON: {0}")]
    EncodeError(String),
    /// The TOON content could not be deserialized into a journal.
    #[error("failed to decode journal from TOON: {0}")]
    DecodeError(String),
    /// The journal file could not be read or written.
    #[error("failed to write journal file: {0}")]
    WriteError(#[from] std::io::Error),
    /// A journal is already at the path, and the output keeps it.
    #[error("journal already exists: {}", .0.display())]
    Exists(std::path::PathBuf),
}

/// What a [`JournalOutput`] does with a journal that is already at its
/// path. A journal holds the evidence of exactly one run, so a run keeps
/// the journal of an earlier one unless its user asks to replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingJournal {
    /// Keep the journal and fail, at prepare and again at publish.
    Refuse,
    /// Put the new journal in its place.
    Replace,
}

impl ExistingJournal {
    /// [`Replace`](Self::Replace) when the user forced it, else
    /// [`Refuse`](Self::Refuse).
    #[must_use]
    pub fn replace_if(force: bool) -> Self {
        if force {
            return Self::Replace;
        }
        Self::Refuse
    }
}

/// Encode a journal to a TOON string.
///
/// # Errors
///
/// Returns [`JournalError::EncodeError`] if the journal cannot be serialized.
pub fn encode_journal(journal: &Journal) -> Result<String, JournalError> {
    toon_format::encode_default(journal).map_err(|e| JournalError::EncodeError(e.to_string()))
}

/// Write a journal to a file in TOON format, in the place of a file that is
/// already at `path`.
///
/// # Errors
///
/// Returns [`JournalError::EncodeError`] if the journal cannot be serialized.
/// Returns [`JournalError::WriteError`] if the file cannot be written.
pub fn write_journal(journal: &Journal, path: &Path) -> Result<(), JournalError> {
    JournalOutput::prepare(path, ExistingJournal::Replace)?.write(&encode_journal(journal)?)
}

/// Reserved writable journal output. The temporary file is owner-only and is
/// published atomically after execution; failed runs cannot truncate old evidence.
pub struct JournalOutput {
    file: tempfile::NamedTempFile,
    destination: std::path::PathBuf,
    existing: ExistingJournal,
}

impl JournalOutput {
    /// Checks the destination and reserves a file before any fault executes.
    /// `existing` says what to do with a journal already at `path`.
    ///
    /// # Errors
    /// Returns an error for a non-file destination or an unwritable parent,
    /// and [`JournalError::Exists`] for a journal at `path` that `existing`
    /// refuses to replace.
    pub fn prepare(path: &Path, existing: ExistingJournal) -> Result<Self, JournalError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "journal destination must be a regular file, not a directory or symlink",
                )
                .into());
            }
            Ok(_) if existing == ExistingJournal::Refuse => {
                return Err(JournalError::Exists(path.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            file: tempfile::NamedTempFile::new_in(parent)?,
            destination: path.to_path_buf(),
            existing,
        })
    }

    /// Writes encoded evidence and atomically publishes it at the reserved path.
    ///
    /// # Errors
    /// Returns an error if writing, syncing or publication fails, and
    /// [`JournalError::Exists`] for a journal that came to the path after
    /// [`prepare`](Self::prepare) and that the output refuses to replace:
    /// of two runs that publish to one path, one wins.
    pub fn write(mut self, content: &str) -> Result<(), JournalError> {
        use std::io::Write;
        self.file.write_all(content.as_bytes())?;
        self.file.as_file().sync_all()?;
        #[cfg(unix)]
        let parent = std::fs::File::open(
            self.destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        match self.existing {
            // Refused here too: a journal that came after prepare is kept.
            ExistingJournal::Refuse => self
                .file
                .persist_noclobber(&self.destination)
                .map_err(|error| refused(error.error, &self.destination))?,
            ExistingJournal::Replace => self
                .file
                .persist(&self.destination)
                .map_err(|error| error.error)?,
        };
        // Persist the directory entry as well as the temporary file's contents.
        #[cfg(unix)]
        parent.sync_all()?;
        Ok(())
    }
}

/// The error of a publish that was refused: a journal at `path` is
/// [`JournalError::Exists`], any other failure is the write error it is.
fn refused(error: std::io::Error, path: &Path) -> JournalError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return JournalError::Exists(path.to_path_buf());
    }
    error.into()
}

/// Read a journal from a TOON file.
///
/// # Errors
///
/// Returns [`JournalError::WriteError`] if the file cannot be read.
/// Returns [`JournalError::DecodeError`] if the TOON content is malformed.
pub fn read_journal(path: &Path) -> Result<Journal, JournalError> {
    let content = std::fs::read_to_string(path)?;
    toon_format::decode_default(&content).map_err(|e| JournalError::DecodeError(e.to_string()))
}

/// Which journals a view over many of them is about: the runs that started
/// at or after a time, on one target. Without either bound it is about all
/// of them.
#[derive(Debug, Default)]
pub struct JournalScope {
    since_ns: Option<i64>,
    target: Option<String>,
    without_target: usize,
}

impl JournalScope {
    /// The scope of the runs that started at or after `since_ns`, in epoch
    /// nanoseconds, on `target`: the system or the technology a journal
    /// names (see [`Journal::is_on`]).
    #[must_use]
    pub fn new(since_ns: Option<i64>, target: Option<&str>) -> Self {
        Self {
            since_ns,
            target: target.map(str::to_owned),
            without_target: 0,
        }
    }

    /// Whether `journal` is in the scope. When a target is asked for, a
    /// journal of the time window that names no target cannot be placed: it
    /// is left out, and counted in [`without_target`](Self::without_target).
    pub fn admit(&mut self, journal: &Journal) -> bool {
        if self
            .since_ns
            .is_some_and(|since_ns| journal.started_at_ns < since_ns)
        {
            return false;
        }
        let Some(target) = &self.target else {
            return true;
        };
        let Some(on_target) = journal.is_on(target) else {
            self.without_target += 1;
            return false;
        };
        on_target
    }

    /// How many journals were left out for naming no target.
    #[must_use]
    pub fn without_target(&self) -> usize {
        self.without_target
    }

    /// The line that says how many journals were left out for naming no
    /// target. `None` when none was.
    #[must_use]
    pub fn left_out_note(&self) -> Option<String> {
        if self.without_target == 0 {
            return None;
        }
        Some(format!(
            "Left out: {} journal(s) that name no target (a journal names the target its \
             experiment declares, since 3.0.2)",
            self.without_target
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use tempfile::TempDir;

    /// A journal of a run that started at `started_at_ns`, on the target
    /// `system` when one is given.
    fn started_on(started_at_ns: i64, system: Option<&str>) -> Journal {
        let mut journal = minimal_journal();
        journal.started_at_ns = started_at_ns;
        journal.target = system.map(|system| ExperimentTarget {
            system: system.into(),
            technology: Some("mysql".into()),
            environment: None,
            criticality: None,
            namespace: None,
            version: None,
        });
        journal
    }

    #[test]
    fn a_scope_without_bounds_admits_every_journal() {
        let mut scope = JournalScope::new(None, None);

        assert!(scope.admit(&started_on(i64::MIN, None)));
        assert!(scope.admit(&started_on(1, Some("orders"))));
        assert_eq!(scope.without_target(), 0);
        assert_eq!(scope.left_out_note(), None);
    }

    #[test]
    fn a_scope_admits_the_runs_that_started_at_or_after_its_start() {
        let mut scope = JournalScope::new(Some(1_000), None);

        assert!(!scope.admit(&started_on(999, None)));
        assert!(scope.admit(&started_on(1_000, None)));
        assert!(scope.admit(&started_on(1_001, None)));
    }

    #[test]
    fn a_scope_on_a_target_admits_the_runs_on_it() {
        let mut scope = JournalScope::new(None, Some("Orders"));

        assert!(scope.admit(&started_on(1, Some("orders"))));
        assert!(!scope.admit(&started_on(1, Some("sessions"))));
        // A run on another target is known not to be on this one: it is
        // not a journal that names no target.
        assert_eq!(scope.without_target(), 0);

        let mut by_technology = JournalScope::new(None, Some("mysql"));
        assert!(by_technology.admit(&started_on(1, Some("sessions"))));
    }

    #[test]
    fn a_scope_on_a_target_leaves_out_and_counts_the_journals_that_name_none() {
        let mut scope = JournalScope::new(Some(1_000), Some("orders"));

        assert!(!scope.admit(&started_on(1_000, None)));
        assert!(!scope.admit(&started_on(2_000, None)));
        // Before the window: out of the scope whatever it was on.
        assert!(!scope.admit(&started_on(999, None)));
        assert!(!scope.admit(&started_on(999, Some("orders"))));
        assert!(scope.admit(&started_on(1_000, Some("orders"))));

        assert_eq!(scope.without_target(), 2);
        assert_eq!(
            scope.left_out_note().as_deref(),
            Some(
                "Left out: 2 journal(s) that name no target (a journal names the target its \
                 experiment declares, since 3.0.2)"
            )
        );
    }

    fn minimal_journal() -> Journal {
        Journal {
            target_version: None,
            target: None,
            experiment_title: "test experiment".into(),
            experiment_id: "test-id-001".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            kind: TestKind::default(),
            objectives: None,
            probe_samples: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    fn journal_with_results() -> Journal {
        let mut j = minimal_journal();
        j.method_results = vec![ActivityResult {
            name: "kill-pod".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1_774_980_135_000_000_000,
            duration_ms: 342,
            output: Some("pod deleted".into()),
            error: None,
            trace_id: "trace-001".into(),
            span_id: "span-001".into(),
        }];
        j.estimate = Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(15.0),
            expected_degradation: None,
            expected_data_loss: None,
            confidence: Some(Confidence::High),
            rationale: None,
            prior_runs: Some(3),
        });
        j
    }

    #[test]
    fn encode_minimal_journal_produces_toon() {
        let journal = minimal_journal();
        let toon = encode_journal(&journal).unwrap();
        assert!(!toon.is_empty());
        assert!(toon.contains("test experiment"));
    }

    #[test]
    fn encode_decode_round_trip() {
        let journal = minimal_journal();
        let toon = encode_journal(&journal).unwrap();
        let decoded: Journal = toon_format::decode_default(&toon).unwrap();
        assert_eq!(decoded, journal);
    }

    #[test]
    fn encode_journal_with_results_round_trips() {
        let journal = journal_with_results();
        let toon = encode_journal(&journal).unwrap();
        let decoded: Journal = toon_format::decode_default(&toon).unwrap();
        assert_eq!(decoded, journal);
    }

    #[test]
    fn write_and_read_journal_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("journal.toon");

        let journal = journal_with_results();
        write_journal(&journal, &path).unwrap();

        assert!(path.exists());
        let loaded = read_journal(&path).unwrap();
        assert_eq!(loaded, journal);
    }

    #[test]
    fn write_journal_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("output.toon");

        write_journal(&minimal_journal(), &path).unwrap();
        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("test experiment"));
    }

    #[test]
    fn an_output_that_refuses_fails_at_prepare_on_an_existing_journal() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("journal.toon");
        std::fs::write(&path, "the first run").unwrap();

        let refused = JournalOutput::prepare(&path, ExistingJournal::Refuse);

        assert!(
            matches!(&refused, Err(JournalError::Exists(at)) if at == &path),
            "{:?}",
            refused.err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the first run");
    }

    #[test]
    fn an_output_that_refuses_fails_at_publish_on_a_journal_that_came_after_prepare() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("journal.toon");
        let output = JournalOutput::prepare(&path, ExistingJournal::Refuse).unwrap();
        // Another run publishes its journal first.
        std::fs::write(&path, "the other run").unwrap();

        let refused = output.write("this run");

        assert!(
            matches!(&refused, Err(JournalError::Exists(at)) if at == &path),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the other run");
        // The reserved file is gone with the refused output.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_output_that_refuses_publishes_where_no_journal_is() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("journal.toon");

        JournalOutput::prepare(&path, ExistingJournal::Refuse)
            .unwrap()
            .write("the first run")
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the first run");
    }

    #[test]
    fn an_output_that_replaces_puts_the_new_journal_in_place_of_the_old() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("journal.toon");
        std::fs::write(&path, "the first run").unwrap();

        JournalOutput::prepare(&path, ExistingJournal::Replace)
            .unwrap()
            .write("the second run")
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the second run");
    }

    #[test]
    fn a_forced_run_replaces_and_any_other_refuses() {
        assert_eq!(ExistingJournal::replace_if(true), ExistingJournal::Replace);
        assert_eq!(ExistingJournal::replace_if(false), ExistingJournal::Refuse);
    }

    #[test]
    fn read_nonexistent_file_returns_error() {
        let result = read_journal(Path::new("/nonexistent/journal.toon"));
        assert!(matches!(result, Err(JournalError::WriteError(_))));
    }

    /// A journal that 3.0.3 recorded holds none of the facts of the
    /// recovery clock. It reads, and it writes back as it was.
    #[test]
    fn a_journal_written_by_3_0_3_reads_and_writes_back_as_it_was() {
        let dir = TempDir::new().unwrap();
        let recorded = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/recorded/postgres-failover.journal.toon");

        let journal = read_journal(&recorded).unwrap();

        let post = journal.post_result.as_ref().unwrap();
        assert_eq!(post.mttr_s, Some(0.0));
        assert_eq!(post.impact_observed, None);
        assert_eq!(post.outage_started_at_ns, None);
        assert_eq!(post.recovered_at_ns, None);
        let objectives = journal.objectives.as_ref().unwrap();
        assert!(objectives.verdicts.iter().all(|v| v.reason.is_none()));
        let copy = dir.path().join("copy.journal.toon");
        write_journal(&journal, &copy).unwrap();
        assert_eq!(read_journal(&copy).unwrap(), journal);
        let written = std::fs::read_to_string(&copy).unwrap();
        for key in [
            "impact_observed",
            "outage_started_at_ns",
            "recovered_at_ns",
            "reason",
        ] {
            assert!(!written.contains(key), "{written}");
        }
    }

    /// The text of a journal that a run recorded, with each `(from, to)`
    /// edit made once.
    fn recorded_journal_with(edits: &[(&str, &str)]) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/recorded/api-saturation.journal.toon");
        let mut text = std::fs::read_to_string(path).unwrap();
        for (from, to) in edits {
            assert!(text.contains(from), "the recorded journal has no {from:?}");
            text = text.replacen(from, to, 1);
        }
        text
    }

    /// `text` written to a journal file in `dir` and read back.
    fn read_text(dir: &TempDir, name: &str, text: &str) -> Journal {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        read_journal(&path).unwrap()
    }

    /// The measurement keys of the during phase that 3.0.2 still had.
    const OLD_DURING_KEYS: &str = "  sample_interval_s: 1
  degradation_onset_s: 1.5
  degradation_peak_s: 4
  degradation_magnitude: 0.4
  graceful_degradation: true
";

    /// The measurement keys of the post phase that 3.0.2 still had.
    const OLD_POST_KEYS: &str = "  mttr_s: 0
  residual_degradation: 0.5
  data_integrity_verified: true
  data_loss_detected: false
";

    #[test]
    fn a_journal_that_still_has_the_removed_measurement_keys_reads_as_before() {
        let dir = TempDir::new().unwrap();
        let old = recorded_journal_with(&[
            ("  sample_interval_s: 1\n", OLD_DURING_KEYS),
            ("  mttr_s: 0\n", OLD_POST_KEYS),
        ]);

        let read = read_text(&dir, "old.journal.toon", &old);

        let without_keys = read_text(&dir, "new.journal.toon", &recorded_journal_with(&[]));
        assert_eq!(read, without_keys);
    }

    #[test]
    fn a_step_that_a_journal_stored_as_timeout_reads_as_failed() {
        let dir = TempDir::new().unwrap();
        let old = recorded_journal_with(&[("    status: succeeded\n", "    status: timeout\n")]);

        let read = read_text(&dir, "old.journal.toon", &old);

        assert_eq!(read.method_results[0].name, "process-still-up");
        assert_eq!(read.method_results[0].status, ActivityStatus::Failed);
    }

    #[test]
    fn read_corrupted_file_returns_decode_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("corrupted.toon");
        std::fs::write(&path, "this is not valid TOON content {{{").unwrap();

        let result = read_journal(&path);
        assert!(matches!(result, Err(JournalError::DecodeError(_))));
    }
}
