//! The metric digest of the recorded runs is the one kept with the tests.
//!
//! The store holds what the `tumult` binary sent and wrote on five runs of
//! a lab (see `tumult_test_utils::recorded`). The digest is built the way
//! the daemon builds its weekly one: every metric shipped in `metrics`,
//! over the week before a fixed time.
//!
//! When the digest changes on purpose, read the new text and keep it:
//! `TUMULT_BLESS=1` writes the file.

use std::path::Path;
use std::time::Duration;

use tumult_lake::Store;
use tumult_report::{build_report, digest_title, render_html};
use tumult_test_utils::{assert_kept, recorded};

const WEEK: Duration = Duration::from_secs(7 * 86_400);

#[test]
fn the_weekly_digest_of_the_recorded_runs_is_the_one_kept_with_the_tests() {
    let dir = tempfile::tempdir().expect("a directory");
    let store = Store::open(&dir.path().join("lake.duckdb")).expect("a store");
    let journals = recorded::load(&store.writer().expect("a writer"));
    let made_at = recorded::day_after(&journals);
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../metrics");
    let metrics = tumult_metrics::load_dir(&shipped).expect("the shipped metrics");

    let week_ns = i64::try_from(WEEK.as_nanos()).expect("a week in nanoseconds");

    let mut digest = build_report(
        &store.read_only().expect("a reader"),
        &metrics,
        &digest_title(WEEK),
        Some((made_at - week_ns, made_at)),
    )
    .expect("the digest");
    // The builder stamps the digest with the time of the clock.
    digest.generated_at_ns = made_at;

    let kept_at = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/weekly-digest.html");
    assert_kept(&kept_at, &render_html(&digest));
}
