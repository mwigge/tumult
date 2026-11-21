//! `docs/guides/cli-reference.md` against the `--help` of the binary: every
//! command has a section, and each `## tumult ...` section names every long
//! flag of its command and of the subcommands below it and no other, every
//! default and possible value the help prints (in backticks), and a table
//! row for each subcommand. `tumult-mcp/tests/cli_reference.rs` checks
//! `tumult-mcp`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// Flags every command takes; the page names them once, at the top.
const COMMON: [&str; 2] = ["--help", "--version"];

fn page() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/guides/cli-reference.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `--help` of `tumult <command>`, with no environment but a home of
/// its own, so no variable of the machine shows up in the help.
fn help(home: &Path, command: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tumult"))
        .args(command)
        .arg("--help")
        .env_clear()
        .env("HOME", home)
        .env("TUMULT_LAKE_PATH", home.join("lake.duckdb"))
        .output()
        .expect("run tumult --help");
    assert!(
        output.status.success(),
        "tumult {} --help failed: {}",
        command.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the help is UTF-8")
}

/// Every command path below `tumult` with its help, the root as `[]`.
fn command_tree(home: &Path) -> BTreeMap<Vec<String>, String> {
    let mut tree = BTreeMap::new();
    let mut todo = vec![Vec::new()];
    while let Some(path) = todo.pop() {
        let text = help(home, &path);
        for sub in subcommands(&text) {
            let mut below = path.clone();
            below.push(sub);
            todo.push(below);
        }
        tree.insert(path, text);
    }
    tree
}

/// The lines of the part of a help text under `title`, such as `Options:`.
fn part<'a>(help: &'a str, title: &str) -> Vec<&'a str> {
    help.lines()
        .skip_while(|line| *line != title)
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' '))
        .collect()
}

/// The subcommands a help text lists, without `help`. A name is indented by
/// two spaces; a wrapped description is indented by more.
fn subcommands(help: &str) -> Vec<String> {
    part(help, "Commands:")
        .iter()
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|rest| !rest.starts_with(' '))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
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
    part(help, "Options:")
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
    let options = part(help, "Options:");
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

/// The `## tumult ...` sections of the page, by title.
fn sections(page: &str) -> BTreeMap<String, String> {
    page.split("\n## ")
        .skip(1)
        .filter_map(|chunk| chunk.split_once('\n'))
        .filter(|(title, _)| title.starts_with("tumult"))
        .map(|(title, body)| (title.trim().to_string(), body.to_string()))
        .collect()
}

/// What a section says that the help of its command does not, or the
/// other way round.
fn differences(title: &str, body: &str, tree: &BTreeMap<Vec<String>, String>) -> Vec<String> {
    let path: Vec<String> = title.split_whitespace().skip(1).map(String::from).collect();
    let Some(own) = tree.get(&path) else {
        return vec![format!("`{title}`: tumult has no such command")];
    };
    let below: Vec<&String> = tree
        .iter()
        .filter(|(command, _)| command.starts_with(&path))
        .map(|(_, text)| text)
        .collect();
    let want: BTreeSet<String> = below.iter().flat_map(|text| flags(text)).collect();
    let have = long_flags(body);
    let mut out = Vec::new();
    for flag in want.difference(&have) {
        out.push(format!("`{title}`: {flag} is in the help, not on the page"));
    }
    for flag in have.difference(&want) {
        out.push(format!("`{title}`: {flag} is on the page, not in the help"));
    }
    for sub in subcommands(own) {
        if !body.contains(&format!("| `{sub}`")) && !body.contains(&format!("| `{sub} ")) {
            out.push(format!("`{title}`: no table row for `{sub}`"));
        }
    }
    let values: BTreeSet<String> = below.iter().flat_map(|text| values(text)).collect();
    for value in values
        .iter()
        .filter(|value| !body.contains(&format!("`{value}`")))
    {
        out.push(format!(
            "`{title}`: the help prints `{value}`, the page does not"
        ));
    }
    out
}

#[test]
fn the_reference_page_matches_the_help_of_every_command() {
    let home = tempfile::tempdir().unwrap();
    let tree = command_tree(home.path());
    assert!(tree.len() > 20, "the walk found {} commands", tree.len());
    let page = page();
    let sections = sections(&page);
    let root: Vec<String> = Vec::new();
    let mut problems = Vec::new();
    for command in subcommands(&tree[&root]) {
        let title = format!("tumult {command}");
        let prefix = format!("{title} ");
        if !sections
            .keys()
            .any(|t| *t == title || t.starts_with(&prefix))
        {
            problems.push(format!("`{title}`: no section on the page"));
        }
    }
    for (title, body) in &sections {
        // A binary of its own, checked by the tests of tumult-mcp.
        if title != "tumult-mcp" {
            problems.extend(differences(title, body, &tree));
        }
    }
    assert!(
        problems.is_empty(),
        "docs/guides/cli-reference.md and the help differ:\n{}",
        problems.join("\n")
    );
}
