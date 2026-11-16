//! Test-only tool of the DORA score proof
//! (`scripts/proof/dora-score/proof.sh`): store the daily status snapshot
//! for a range of past days in a private lake.
//!
//! The Operating status needs 180 stored days, and a proof cannot wait 180
//! days. This tool runs what the daemon's daily job runs (the same
//! `report::snapshot_all` and the same `Snapshot::store`), once per day of
//! the range, with the clock set to that day. It is not part of the daemon
//! and no endpoint does this: on a real lake a day that was not stored
//! stays not stored.
//!
//! Every row it writes has the source `proof`, not `snapshot`. The score
//! ignores such days unless the daemon is started with
//! `KRONIKA_SCORE_PROOF_DAYS=1`, and then every score response and the
//! page say that proof days are in use.
//!
//! ```text
//! DORA_PROOF_PRIVATE_LAKE=<lake> dora_store_status_days <lake> <from> <to> [skip]
//! ```
//!
//! Stores the days from `from` days ago down to `to` days ago, leaving out
//! the day `skip` days ago. The daemon must be stopped (one writer per
//! lake). The lake path must be repeated in `DORA_PROOF_PRIVATE_LAKE`, so
//! the tool is never pointed at a lake by an inherited `TUMULT_LAKE_PATH`.
//! House parameters come from the `score:` section of `KRONIKA_ORG_FILE`
//! and workloads from `KRONIKA_MANIFESTS_DIR`, as in the daemon.

#![allow(clippy::print_stdout)] // an example: the count of stored days is its output

use std::error::Error;
use std::path::{Path, PathBuf};

use tumult_api::handlers::dora::read_manifests;
use tumult_compliance::dora::params::HouseParams;
use tumult_compliance::dora::{register, report};
use tumult_lake::Store;

const DAY_NS: i64 = 86_400 * 1_000_000_000;
const USAGE: &str = "usage: dora_store_status_days <lake> <from days ago> <to days ago> \
                     [skip days ago]";

fn now_ns() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_nanos()).unwrap_or(i64::MAX)
}

fn days(arg: Option<String>) -> Result<i64, Box<dyn Error>> {
    let text = arg.ok_or(USAGE)?;
    let days: i64 = text.parse().map_err(|_| USAGE)?;
    if !(1..=1000).contains(&days) {
        return Err("days ago must be within 1..=1000".into());
    }
    Ok(days)
}

fn params() -> Result<HouseParams, Box<dyn Error>> {
    let Some(org) = std::env::var_os("KRONIKA_ORG_FILE") else {
        return Ok(HouseParams::default());
    };
    Ok(register::load(Path::new(&org))?.params)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let lake = PathBuf::from(args.next().ok_or(USAGE)?);
    let from = days(args.next())?;
    let to = days(args.next())?;
    let skip = args.next().map(|s| days(Some(s))).transpose()?;
    if std::env::var_os("DORA_PROOF_PRIVATE_LAKE").map(PathBuf::from) != Some(lake.clone()) {
        return Err("set DORA_PROOF_PRIVATE_LAKE to the same lake path: test lakes only".into());
    }
    if !lake.is_file() {
        return Err(format!("{} does not exist", lake.display()).into());
    }
    let manifests = std::env::var_os("KRONIKA_MANIFESTS_DIR").map(PathBuf::from);
    let (workloads, _notes) = read_manifests(manifests.as_deref())?;
    let params = params()?;
    let store = Store::open(&lake)?;
    let writer = store.writer()?;
    let now = now_ns();
    let mut stored = 0;
    for ago in (to..=from).rev().filter(|ago| Some(*ago) != skip) {
        let as_of_ns = now - ago * DAY_NS;
        let snapshot = {
            let reader = store.read_only()?;
            // Each day is scored over the proof days stored before it.
            report::snapshot_all(&reader, &params, &workloads, as_of_ns, true)?
        };
        snapshot.as_proof().store(&writer, as_of_ns)?;
        stored += 1;
    }
    println!("stored {stored} days with source 'proof'");
    Ok(())
}
