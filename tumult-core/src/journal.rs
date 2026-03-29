//! Journal writer — serializes experiment results to TOON format.

use std::path::Path;

use crate::types::Journal;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum JournalError {
    #[error("failed to encode journal to TOON: {0}")]
    EncodeError(String),
    #[error("failed to write journal file: {0}")]
    WriteError(#[from] std::io::Error),
}

/// Encode a journal to a TOON string.
pub fn encode_journal(journal: &Journal) -> Result<String, JournalError> {
    toon_format::encode_default(journal).map_err(|e| JournalError::EncodeError(e.to_string()))
}

/// Write a journal to a file in TOON format.
pub fn write_journal(journal: &Journal, path: &Path) -> Result<(), JournalError> {
    let toon = encode_journal(journal)?;
    std::fs::write(path, toon)?;
    Ok(())
}

/// Read a journal from a TOON file.
pub fn read_journal(path: &Path) -> Result<Journal, JournalError> {
    let content = std::fs::read_to_string(path)?;
    toon_format::decode_default(&content).map_err(|e| JournalError::EncodeError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use tempfile::TempDir;

    fn minimal_journal() -> Journal {
        Journal {
            experiment_title: "test experiment".into(),
            experiment_id: "test-id-001".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1774980000000000000,
            ended_at_ns: 1774980300000000000,
            duration_ms: 300000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        }
    }

    fn journal_with_results() -> Journal {
        let mut j = minimal_journal();
        j.method_results = vec![ActivityResult {
            name: "kill-pod".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1774980135000000000,
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
    fn read_nonexistent_file_returns_error() {
        let result = read_journal(Path::new("/nonexistent/journal.toon"));
        assert!(result.is_err());
    }
}
