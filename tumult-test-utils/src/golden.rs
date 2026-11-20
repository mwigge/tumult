//! Text kept with the tests: a report, a page, a chart.
//!
//! A test makes the text with the real code and [`assert_kept`] compares it
//! with the file kept in the repository. When the text changes on purpose,
//! read the new text and keep it: `TUMULT_BLESS=1` writes the file.

use std::path::Path;

/// The variable that makes [`assert_kept`] write the file before it reads it.
pub const BLESS: &str = "TUMULT_BLESS";

/// Where `text` and `kept` first differ, for the message of a failed
/// comparison.
fn first_difference(text: &str, kept: &str) -> String {
    let mut kept_lines = kept.lines();
    for (number, line) in text.lines().enumerate() {
        let kept_line = kept_lines.next().unwrap_or("(no further line)");
        if line != kept_line {
            return format!("line {}:\n  now:  {line}\n  kept: {kept_line}", number + 1);
        }
    }
    match kept_lines.next() {
        Some(kept_line) => format!(
            "line {}:\n  now:  (no further line)\n  kept: {kept_line}",
            text.lines().count() + 1
        ),
        None => "the line ends differ".to_string(),
    }
}

/// Assert that `text` is the text of the file `kept_at`.
///
/// With `TUMULT_BLESS` set, `text` is written to the file first, so the
/// comparison holds and the new text shows in the diff of the commit.
///
/// # Panics
///
/// When the text is not the kept one, when there is no kept file, and when
/// the file cannot be written.
pub fn assert_kept(kept_at: &Path, text: &str) {
    if std::env::var_os(BLESS).is_some() {
        let dir = kept_at.parent().expect("the kept file is in a directory");
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        std::fs::write(kept_at, text).unwrap_or_else(|e| panic!("{}: {e}", kept_at.display()));
    }
    let kept = std::fs::read_to_string(kept_at).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\n{BLESS}=1 keeps the text of this run",
            kept_at.display()
        )
    });
    assert!(
        text == kept,
        "{} is not the text of this run, {}\n{BLESS}=1 keeps the text of this run",
        kept_at.display(),
        first_difference(text, &kept)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnvGuard;

    /// The message of the panic `check` ends with.
    fn panic_of(check: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let panic = std::panic::catch_unwind(check).expect_err("a panic");
        panic.downcast_ref::<String>().expect("a message").clone()
    }

    /// The environment of a run that only compares.
    fn without_bless() -> EnvGuard {
        let mut env = EnvGuard::lock();
        env.remove(BLESS);
        env
    }

    /// A directory with the kept file `kept.txt` that holds `text`.
    fn kept(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("kept.txt");
        std::fs::write(&file, text).expect("kept file");
        (dir, file)
    }

    #[test]
    fn the_kept_text_passes() {
        let _env = without_bless();
        let (_dir, file) = kept("one\ntwo\n");

        assert_kept(&file, "one\ntwo\n");
    }

    #[test]
    fn another_text_fails_with_the_first_line_that_differs() {
        let _env = without_bless();
        let (_dir, file) = kept("one\ntwo\nthree\n");

        let message = panic_of(|| assert_kept(&file, "one\n2\n3\n"));

        assert!(
            message.contains("line 2:\n  now:  2\n  kept: two"),
            "{message}"
        );
        assert!(message.contains("kept.txt"), "{message}");
        assert!(message.contains("TUMULT_BLESS=1"), "{message}");
        // The kept file is not touched.
        assert_eq!(
            std::fs::read_to_string(&file).expect("kept file"),
            "one\ntwo\nthree\n"
        );
    }

    #[test]
    fn a_longer_text_fails_at_its_first_further_line() {
        let _env = without_bless();
        let (_dir, file) = kept("one\n");

        let message = panic_of(|| assert_kept(&file, "one\ntwo\n"));

        assert!(
            message.contains("line 2:\n  now:  two\n  kept: (no further line)"),
            "{message}"
        );
    }

    #[test]
    fn a_shorter_text_fails_at_the_first_line_it_does_not_have() {
        let _env = without_bless();
        let (_dir, file) = kept("one\ntwo\nthree\n");

        let message = panic_of(|| assert_kept(&file, "one\n"));

        assert!(
            message.contains("line 2:\n  now:  (no further line)\n  kept: two"),
            "{message}"
        );
    }

    #[test]
    fn a_text_that_differs_only_in_its_last_line_end_fails() {
        let _env = without_bless();
        let (_dir, file) = kept("one\n");

        let message = panic_of(|| assert_kept(&file, "one"));

        assert!(message.contains("the line ends differ"), "{message}");
    }

    #[test]
    fn a_text_without_a_kept_file_fails_and_writes_nothing() {
        let _env = without_bless();
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("kept.txt");

        let message = panic_of(|| assert_kept(&file, "one\n"));

        assert!(message.contains("kept.txt"), "{message}");
        assert!(message.contains("TUMULT_BLESS=1"), "{message}");
        assert!(!file.exists());
    }

    #[test]
    fn bless_keeps_the_text_of_this_run() {
        let mut env = EnvGuard::lock();
        env.set(BLESS, "1");
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("reports/kept.txt");

        assert_kept(&file, "new\n");

        assert_eq!(std::fs::read_to_string(&file).expect("kept file"), "new\n");
    }
}
