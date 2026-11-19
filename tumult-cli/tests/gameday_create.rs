//! What `tumult gameday create` writes.
//!
//! The binary writes the game day file in the lab. Each file it writes must
//! read back as a game day, as `tumult gameday run` reads it. A set of
//! options that cannot make such a file is refused, and nothing is written.
#![cfg(unix)]

mod lab;

use std::path::PathBuf;
use std::process::Output;

use lab::{fatal_error, Lab};
use tumult_core::types::GameDay;

/// `tumult gameday create <name> --experiments a.toon` with `options`.
fn create(lab: &Lab, name: &str, options: &[&str]) -> Output {
    let mut args = vec!["gameday", "create", name, "--experiments", "a.toon"];
    args.extend_from_slice(options);
    lab.tumult(&args)
}

/// The game day file `<name>.gameday.toon` in the lab.
fn file(lab: &Lab, name: &str) -> PathBuf {
    lab.path(&format!("{name}.gameday.toon"))
}

#[test]
fn a_load_tool_without_a_script_is_refused_and_writes_no_file() {
    let lab = Lab::with_state("ok");

    let output = create(&lab, "x", &["--load", "k6"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        fatal_error(&output.stderr),
        "--load k6 needs --load-script: the k6 `.js` script, or the URL to hit for oha"
    );
    assert!(!file(&lab, "x").exists());
}

#[test]
fn a_load_script_or_a_vu_count_without_a_load_tool_is_refused() {
    let lab = Lab::with_state("ok");

    for options in [
        &["--load-script", "load.js"][..],
        &["--load-vus", "5"],
        &["--load", "none", "--load-script", "load.js"],
    ] {
        let output = create(&lab, "x", options);

        assert_eq!(output.status.code(), Some(1), "{options:?}");
        assert_eq!(
            fatal_error(&output.stderr),
            "--load-script and --load-vus need --load k6 or --load oha",
            "{options:?}"
        );
        assert!(!file(&lab, "x").exists(), "{options:?}");
    }
}

#[test]
fn every_game_day_file_that_create_writes_reads_back() {
    let lab = Lab::with_state("ok");
    // The tool options, and the script that goes with the tool.
    let tools: [(&[&str], &str); 4] = [
        (&[], "load.js"),
        (&["--load", "none"], "load.js"),
        (&["--load", "k6"], "load.js"),
        (&["--load", "oha"], "http://127.0.0.1:9/health"),
    ];
    let mut written = 0;
    for (index, (tool, script)) in tools.iter().enumerate() {
        let tool_name = tool.get(1).copied().filter(|name| *name != "none");
        for with_script in [false, true] {
            for with_vus in [false, true] {
                let name = format!("gd-{index}-{with_script}-{with_vus}");
                let mut options = tool.to_vec();
                if with_script {
                    options.extend(["--load-script", script]);
                }
                if with_vus {
                    options.extend(["--load-vus", "5"]);
                }

                let output = create(&lab, &name, &options);

                let valid = match tool_name {
                    Some(_) => with_script,
                    None => !with_script && !with_vus,
                };
                assert_eq!(
                    output.status.success(),
                    valid,
                    "{options:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if !valid {
                    assert!(!file(&lab, &name).exists(), "{options:?}");
                    continue;
                }
                let content = std::fs::read_to_string(file(&lab, &name)).expect("game day file");
                let gameday: GameDay = toon_format::decode_default(&content).unwrap_or_else(|e| {
                    panic!("{options:?} wrote a file that does not read back ({e}):\n{content}")
                });
                let load = gameday.load.map(|load| {
                    (
                        load.tool.to_string(),
                        load.script.display().to_string(),
                        load.vus,
                    )
                });
                let expected = tool_name.map(|tool| {
                    (
                        tool.to_string(),
                        (*script).to_string(),
                        with_vus.then_some(5),
                    )
                });
                assert_eq!(load, expected, "{options:?}:\n{content}");
                written += 1;
            }
        }
    }
    // No tool and nothing else, twice; a tool with its script, with and
    // without VUs, for k6 and for oha.
    assert_eq!(written, 6);
}

#[test]
fn a_second_create_of_the_same_name_is_refused_and_keeps_the_first_file() {
    let lab = Lab::with_state("ok");
    assert!(create(&lab, "x", &[]).status.success());
    let first = std::fs::read(file(&lab, "x")).expect("game day file");

    let output = create(&lab, "x", &["--load", "k6", "--load-script", "load.js"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        fatal_error(&output.stderr),
        "x.gameday.toon already exists — remove it first or use a different name"
    );
    assert_eq!(
        std::fs::read(file(&lab, "x")).expect("game day file"),
        first
    );
}

#[test]
fn the_help_of_the_load_script_says_that_for_oha_it_is_the_url_to_hit() {
    let lab = Lab::with_state("ok");

    let output = lab.tumult(&["gameday", "create", "--help"]);

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{help}");
    // The help may wrap a long line: compare the words.
    let words = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains(
            "--load-script <LOAD_SCRIPT> Path to load test script (k6 `.js`), or the URL to hit for oha"
        ),
        "{help}"
    );
}
