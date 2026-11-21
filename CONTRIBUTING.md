# Contributing

Thank you for working on Tumult. This page says how to set up, how we test,
and what a change needs before it merges into `main`.

## Before you send a change

- Write the test first and see it fail. Then change the code and see the
  test pass.
- Run `make precommit`. It runs the checks of CI, see [the gates](#the-gates).
- Keep one rule in one place. Provider dispatch, scope checks, checksums
  and journal publication each have one implementation. Credentials and
  deployment targets belong in operator configuration, not in code.
- A commit message says what changed and how it was tested. Leave out
  development-tool and review-process metadata.
- Report a security issue in private, see [SECURITY.md](SECURITY.md).

## Set up

- Rust 1.92.0, Node 22.17 or later, and Docker.
- The web UI: `npm --prefix web ci`, then `npm --prefix web run build`.
  Build it before the Rust crates: `tumultd` embeds `web/build`.
- The Python checks: `python3 -m venv .quality-venv`, then
  `.quality-venv/bin/pip install -r requirements-quality.txt`.
- `make precommit` installs no tools. It needs cargo-nextest, cargo-machete,
  cargo-deny, cargo-mutants, protoc and the chromium of Playwright
  (`npm --prefix web exec -- playwright install chromium`).
- On a machine with many CPUs, set `CARGO_BUILD_JOBS=4` and pass
  `-- --test-threads=2` to Cargo tests, to bound the linker and DuckDB.

## How we test

### The gates

These jobs run on every pull request to `main`.

| Job | What it checks |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Test | Clippy with the workspace lints, the lake without default features, `cargo machete`, `cargo nextest run --all-features`, the HTML report and the run page in a browser, the OTel-Arrow code against its proto |
| Test (macOS) | `cargo test --workspace --all-features` |
| Demo crates | `make demo-test`: format, Clippy and tests of `demo/app`, `demo/control-panel` and `demo/traffic` |
| Cargo Deny | Advisories, licences and sources, with the exceptions in `deny.toml` |
| Documentation | `cargo doc` with warnings as errors |
| Documentation checks | `scripts/check-docs.py`, `scripts/check-citations.py`, `scripts/check-prose.py` |
| Web UI | Prettier, ESLint, `npm audit`, svelte-check, Vitest, the build, the report script in sync with its source, Playwright |
| Python, SQL and distribution checks | Black, Ruff, SQLFluff, the lake SQL lint, the log message check, the [test hygiene guard](#the-test-hygiene-guard) and the tests of the scripts |
| Forecast sidecar | Ruff and the unit tests of `forecastd` with a stub model |
| Mutation tests | [Mutation tests](#mutation-tests) of the Rust lines that changed |
| Pull request body | The [six headings](#prose-and-the-pull-request-body) |

Some workflows run only when their files change: the site preview for
`docs/` (it builds the site, runs `scripts/site-check.py` and takes pictures
at 1440 and 390 px), the examples proof (the examples and GameDays against a
lab) and the Chaos Lab. Coverage runs on `main` and every night, with a floor
of 90 %.

`make precommit` runs the checks of Format, Test, Demo crates, Cargo Deny and
Documentation checks, then the mutation tests of the changed lines.
`scripts/test-quality-gates.py` keeps it equal to `ci.yml`. The Clippy lints
are in `[workspace.lints]` of `Cargo.toml`, and every crate reads them.

### What a test must prove

A test exists to fail when behaviour we own changes.

- It asserts an outcome: a returned value, a stored row, a request sent, a
  span or metric, a rendered string, an error variant. "It did not panic"
  counts only when that is the contract: the name ends in
  `_does_not_panic` and a comment above the test names the panic.
- Break the code once and see the new test fail. A test that cannot fail
  has no value.
- Do not test derived `Debug`, `Clone` or `Default`, `thiserror` text, or
  serde round trips of plain derives. A format that leaves the process
  (journal and experiment TOON, API and MCP payloads, stored labels, CLI
  flags) is pinned with a literal of its encoded form.
- Fake only at I/O boundaries: network, clock, file system roots,
  environment, external programs. Assert on what was sent. Shared helpers
  live in `tumult-test-utils`, one definition each.
- One behaviour, one test, at the lowest level that can see it. When you
  delete a test, the commit names the test that still covers the behaviour.
- Names are `<subject>_<condition>_<outcome>` or a plain sentence, and the
  body asserts what the name says.

A behaviour a user can see has at least one test that goes the whole way:

| Behaviour of | The test goes through |
|---|---|
| A run | The `tumult` binary, or the runner with `tumult_exec::ProviderExecutor` and real `sh` steps |
| A command | The `tumult` binary: its exit code, its output, the files it writes |
| The lake | A store in a temporary directory, written by the production code |
| The API | The real router over a real store |
| The HTML report | A browser without a network opens what the binary writes: `npm run test:report` in `web/` |
| A report | The builder and renderer over the recorded runs in `tumult-test-utils/recorded`, compared with the file in `tests/golden` |
| A run page in Krönika | A browser on a real `tumultd` with its own lake: `npm run test:live` in `web/` |

The specs in `web/tests` answer `/api` with fixtures, so they prove how a
page draws what it gets, not that it is true. A scripted executor is allowed
for cases a real process cannot give on demand, never as the only proof.
Rows written by hand into a store prove the reader, not the writer.

Kept files: `tumult_test_utils::assert_kept(path, text)` compares a whole
report with its file and names the first line that differs.
`TUMULT_BLESS=1 cargo test -p <crate>` writes the files again: read the
difference before you commit it.

Time and isolation:

- No `sleep` to wait for something. Wait on the event or poll with a
  deadline, and use an injected clock or `tokio::time::pause` for timers. No
  upper bound on wall-clock time. A sleep that is not a wait has a
  `// sleep:` comment with a reason of three words or more.
- No fixed ports (bind to port 0), no real `$HOME`, no network beyond
  loopback.
- Change the environment only through `tumult_test_utils::EnvGuard`, one
  guard per test: it is not re-entrant.
- Write a stand-in program with `tumult_test_utils::fake_bin` or
  `write_executable`, not `std::fs::write`, or a spawn can fail with "Text
  file busy".
- Assert spans with `capture_spans` and metrics with `MetricCapture`. Do not
  install a global tracer or meter provider in a test.
- Assert what a command prints, not only that it returned `Ok`. In
  `tumult-cli`, `crate::output::testing::capture` returns what the calling
  thread printed.
- The suite passes under both `cargo nextest run` and `cargo test`.
- An ignored or skipped test carries a reason that names the CI job that
  runs it. `.only`, `fdescribe` and `fit` are never committed.

### Mutation tests

`cargo-mutants` changes the code one small step at a time (a `==` becomes
`!=`, a function returns a fixed value) and runs the tests of the crate. A
change that no test notices is a line the tests do not prove.

```sh
cargo install cargo-mutants --locked
make mutants                          # the lines changed since origin/main
make mutants ARGS="-p tumult-core"    # one crate
```

- The `Mutation tests` workflow does the same on each pull request, in
  sixteen parts. Its check must pass before a merge. A pull request that
  changes no Rust skips the parts.
- `MISSED` means no test failed: add a test that fails for that change, or
  remove the code if nothing needs it.
- `TIMEOUT` counts as caught only when a test would also fail without the
  timeout. Otherwise bound the loop.
- A mutant that cannot be told apart from the original goes under
  `exclude_re` in `.cargo/mutants.toml`, with a comment that says why. Never
  use it to silence a missed mutant.
- Test code and `tumult-test-utils` are not mutated.

### The test hygiene guard

`scripts/check-test-hygiene.py` fails on:

| Rule | Meaning |
|---|---|
| `no-assert` | A Rust test that asserts nothing |
| `bare-ignore` | `#[ignore]` without a reason |
| `bare-sleep` | A sleep in Rust test code without a `// sleep:` comment of three words or more (a tokio sleep on a paused clock is exempt) |
| `raw-env` | `env::set_var` or `env::remove_var` in test code outside `tumult-test-utils` |
| `focused-test` | `.only`, `fdescribe` or `fit` in JavaScript or TypeScript |
| `bare-skip` | A Python `skip`, `skipif` or `xfail` without a reason |
| `stale-allow`, `bad-allow` | An allowlist entry that matches nothing, or is not `path::name::rule` |

An assertion is an `assert*!` macro on a value (not on literals alone),
`unwrap_err`, `expect_err`, `panic!`, `unreachable!`, `#[should_panic]`,
`ensure!`, `bail!` or `return Err(..)` in a test that returns `Result`, the
assertions of clap, mockito and assert_cmd, or a call to an `assert_*`
helper of the same crate or `tumult-test-utils`. The guard is a text scan:
it cannot tell whether an assertion means something. Review does that.

`scripts/test-hygiene-allow.txt` lists old offenders, one
`path::test_name::rule  # reason` per line. Do not add to it. When a listed
test is fixed, delete its line.

```sh
python3 scripts/check-test-hygiene.py
python3 scripts/test-check-test-hygiene.py   # the guard's own tests
```

### Prose and the pull request body

- `scripts/check-prose.py` reads the lines a branch adds to markdown files
  and to the html of the site. It fails on an em or en dash and on a
  heading in Title Case. Write headings in sentence case: names, acronyms
  and code keep their capitals.
- The body of a pull request has the headings Why, What changed, Scope,
  Tradeoffs, Blast Radius and Verification, in this order, and at most 40
  lines. Leave out a heading with nothing to say, but never Why or
  Verification. The workflow `Pull request body` checks it with
  `scripts/check-pr-body.py`. Pull requests by Dependabot are skipped.

## What the checks do not prove

- A passing unit suite is not a coverage measurement, and not proof that a
  remote provider works.
- CI runs no fault against Kubernetes, a cloud, a Windows host or a live AI
  client. Those need a disposable target of their own.
- The release workflow builds the archives, the checksums and the CLI, MCP
  and daemon images, and runs bounded checks: `--version`,
  `mcp serve --help`, and the readiness of the daemon image with a read-only
  root and a persistent volume. They do not run faults or certify a
  deployment.
- Before you trust a deployment, start the exact image with its identity
  and mounts, run a harmless approved experiment, check the journal and the
  telemetry, then restart it and stop a run.
- `deny.toml` keeps narrow exceptions for known advisories, so a raw
  `cargo audit` can disagree with the gate. Release notes say so.
