//! `tumult-mcp` against `docs/guides/cli-reference.md`, which says that it
//! takes the same flags as `tumult mcp serve`: every long flag of its
//! `--help` is in the `tumult mcp serve` section and no other, and so is
//! every default and possible value (in backticks).
//! `tumult-cli/tests/cli_reference.rs` checks that section against
//! `tumult mcp serve --help`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// Flags every command takes; the page names them once, at the top.
const COMMON: [&str; 2] = ["--help", "--version"];

fn page() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/guides/cli-reference.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The body of the `## title` section of the page.
fn section(page: &str, title: &str) -> String {
    page.split("\n## ")
        .filter_map(|chunk| chunk.split_once('\n'))
        .find(|(head, _)| head.trim() == title)
        .map_or_else(
            || panic!("no `## {title}` section on the page"),
            |(_, body)| body.to_string(),
        )
}

/// The `--help` of `tumult-mcp`, with no environment but a home of its own,
/// so no variable of the machine shows up in the help.
fn help(home: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tumult-mcp"))
        .arg("--help")
        .env_clear()
        .env("HOME", home)
        .output()
        .expect("run tumult-mcp --help");
    assert!(
        output.status.success(),
        "tumult-mcp --help failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the help is UTF-8")
}

/// The lines of the `Options:` part of a help text.
fn options(help: &str) -> Vec<&str> {
    help.lines()
        .skip_while(|line| *line != "Options:")
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' '))
        .collect()
}

/// Every `--flag` of a text.
fn long_flags(text: &str) -> BTreeSet<String> {
    text.match_indices("--")
        .filter(|(at, _)| !text[..*at].ends_with('-'))
        .filter_map(|(at, _)| {
            let name: String = text[at + 2..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            name.starts_with(|c: char| c.is_ascii_lowercase())
                .then(|| format!("--{name}"))
        })
        .filter(|flag| !COMMON.contains(&flag.as_str()))
        .collect()
}

/// The long flags a help text defines. An option line is indented by two
/// or six spaces; its description, which may name other flags, by more.
fn flags(help: &str) -> BTreeSet<String> {
    options(help)
        .iter()
        .filter(|line| {
            let rest = line.trim_start();
            rest.starts_with('-') && line.len() - rest.len() <= 6
        })
        .filter_map(|line| line.split("  ").find(|cell| !cell.trim().is_empty()))
        .flat_map(long_flags)
        .collect()
}

/// The defaults and possible values a help text prints for its options.
fn values(help: &str) -> BTreeSet<String> {
    let options = options(help);
    let text = options.join("\n");
    let mut found = BTreeSet::new();
    for chunk in text.split("[default: ").skip(1) {
        found.extend(chunk.split(']').next().map(|v| v.trim().to_string()));
    }
    for chunk in text.split("[possible values: ").skip(1) {
        let list = chunk.split(']').next().unwrap_or_default();
        found.extend(list.split(',').map(|v| v.trim().to_string()));
    }
    // The long help lists values with a description one per line:
    // `- name: what it does`.
    for line in &options {
        let Some(rest) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        let tail = &rest[name.len()..];
        if !name.is_empty() && (tail.is_empty() || tail.starts_with(':')) {
            found.insert(name);
        }
    }
    found
}

#[test]
fn tumult_mcp_takes_the_flags_the_reference_page_gives_mcp_serve() {
    let home = tempfile::tempdir().unwrap();
    let help = help(home.path());
    let serve = section(&page(), "tumult mcp serve");
    assert_eq!(
        flags(&help),
        long_flags(&serve),
        "the flags of tumult-mcp --help, then those of `tumult mcp serve` on the page"
    );
    let missing: Vec<String> = values(&help)
        .into_iter()
        .filter(|value| !serve.contains(&format!("`{value}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "tumult-mcp prints defaults or values that `tumult mcp serve` on the page does not: {missing:?}"
    );
}
