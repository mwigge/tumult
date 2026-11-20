---
title: Testing Policy
parent: Guides
nav_order: 32
---

# Testing policy

A test exists to fail when behaviour we own changes. This page says what that
means in this repository and what the CI guard enforces.

## What a test must assert

Each test asserts at least one observable outcome of our code: a returned
value, a persisted row, a request sent, a span or metric emitted, a rendered
string, an error variant.

"It did not panic" is acceptable only when not panicking is the contract. Then
the name ends in `_does_not_panic` and a comment above the test says which
panic it guards. The comment has to contain the word "panic".

A test that returns `Result` and only uses `?` asserts nothing about the
success path. Add an assertion on the value.

A new or rewritten test is checked once by breaking the code it covers and
seeing the test fail. A test that cannot be made to fail has no value.

## What we do not test

- Derived `Debug`, `Clone`, `Default`.
- `thiserror` message text. Match on the variant and its fields.
- Serde round-trips of plain derives, getters, constants restated as literals.

Exception: a format that leaves the process (journal and experiment TOON, API
and MCP payloads, stored enum labels, CLI flags) is pinned with a literal of
the encoded form, not with encode-then-decode.

## Mocks and fakes

Substitute only at I/O boundaries: network, clock, filesystem roots,
environment, external binaries. Prefer an in-memory fake or a local fake server
that records what we sent, and assert on what was sent.

A test whose assertions are satisfied by the stub's canned return value alone
is not a test.

Shared test helpers live in `tumult-test-utils`, one definition each. A helper
is tested only when it contains logic of its own.

## Real paths

A behaviour a user can see has at least one test that goes the whole way: the
real thing runs, and the test reads what a user would read.

| Behaviour of | The test goes through |
|---|---|
| A run (status, steps, rollbacks, recovery) | The `tumult` binary, or the runner with `tumult_exec::ProviderExecutor`, with steps that are real `sh` commands |
| A command | The `tumult` binary: its exit code, its output, the files it writes |
| The lake | A store in a temporary directory, written by the code that writes it in production |
| The API | The real router over a real store |
| The HTML report | A browser without a network opens the report the `tumult` binary writes: `npm run test:report` in `web/` |
| A report (per run, executive digest, evidence pack, run report, metric digest) | The builder and the renderer over a store filled with the recorded runs in `tumult-test-utils/recorded`; the text is compared with the file kept in `tests/golden` |
| The page of a run in Krönika | A browser opens the page on a real `tumultd` with a lake of its own, filled by a real run and an imported journal: `npm run test:live` in `web/` |

The specs in `web/tests` answer `/api` with fixtures. They prove how a page
draws what it is given, not that what it is given is true: a page that shows
evidence also has a spec in `web/live-tests`.

A scripted executor (a type that implements `ActivityExecutor` and returns
what the test tells it to) is a double at an I/O boundary. It is allowed for
cases a real process cannot give on demand, such as a cancel at an exact
step. It is never the only proof of a behaviour: the same behaviour has a
test from the table above.

Rows written by hand into a store prove the reader, not the writer. Use them
for data the current code no longer writes, such as old spellings. For
everything else the rows come from a real run.

## Kept files

A report test compares the whole text with a file kept next to the test, so
every word and number of the report is under test.

- `tumult_test_utils::assert_kept(path, text)` does the comparison and names
  the first line that differs.
- `TUMULT_BLESS=1 cargo test -p <crate>` writes the files again. Read the
  difference before you commit it: the difference is the review.
- The recorded runs are real runs of the `tumult` binary. The README in
  `tumult-test-utils/recorded` says how to record them again.
- What changes from run to run (the version, a time of the clock) is
  asserted first and then replaced by a name such as `{version}`.

## Mutation tests

`cargo-mutants` changes the code one small step at a time: a `==` becomes
`!=`, a function returns a fixed value, a `!` is removed. After each step it
runs the tests of the crate. A change that no test notices is a line the
tests do not prove.

```sh
cargo install cargo-mutants --locked
make mutants                          # the lines changed since origin/main
make mutants ARGS="-p tumult-core"    # one crate
```

- Every changed line is mutated before the commit. The `Mutation tests`
  workflow does the same on each pull request, in eight parts.
- `MISSED` means no test failed. Add a test that fails for that change, or
  remove the code if nothing needs it.
- `TIMEOUT` means the change made a test hang. It counts as caught only when
  a test would also fail without the timeout; otherwise bound the loop.
- A change that cannot be told apart from the original (an equivalent
  mutant) is listed under `exclude_re` in `.cargo/mutants.toml` with a
  comment that says why. This is rare. Do not use it to silence a missed
  mutant.
- Test code and `tumult-test-utils` are not mutated (`.cargo/mutants.toml`).

## Names

`<subject>_<condition>_<outcome>` or a plain sentence. The name states the
behaviour and the body asserts exactly that. If the name says "truncates",
there is an assertion on length.

## Time

No `sleep` to wait for something. Wait on the event (channel, `Notify`, join
handle) or poll with a deadline. For timers use an injected clock or
`tokio::time::pause`. No upper bound on elapsed wall-clock time. A lower bound
is allowed only against a delay the test itself asked for.

A sleep that is not a wait for an event (the interval of a bounded poll, a
delay that is the thing under test, waiting for something that must not
happen) carries a comment on the same line or directly above it that starts
with `// sleep:` and gives the reason in at least three words.

## Isolation

- No fixed ports. Bind to port 0 and read the address back.
- No real `$HOME`, no network beyond loopback.
- Environment variables are changed only through
  `tumult_test_utils::EnvGuard`. It holds a process-wide lock and restores the
  previous values when dropped. `EnvGuard::lock()` is not re-entrant: a second
  call on the same thread while the first guard is alive deadlocks. Take one
  guard per test and pass it to helpers.
- A stand-in program that a test spawns is written with
  `tumult_test_utils::fake_bin` or `write_executable`, not with
  `std::fs::write`. A file written by the test process can be held open by
  a child that another test thread is starting, and spawning it then fails
  with "Text file busy". The helpers write the file from a child process,
  so the test process never holds it open for writing.
- Spans are asserted with `tumult_test_utils::capture_spans`, which returns
  only the spans of the calling test. Metrics are asserted with
  `tumult_test_utils::MetricCapture`, which owns a private meter provider. Do
  not install a global tracer or meter provider in a test. Code that calls
  `set_tracer_provider` itself is tested in a test binary of its own, because
  the global provider is shared by every test in a binary.
- What a `tumult` command prints is asserted, not only that it returned
  `Ok`. In the library tests of `tumult-cli`,
  `crate::output::testing::capture` runs a closure and returns what the
  calling thread printed through `outln!`, `out!` and `errln!`, stdout and
  stderr apart. It sees neither the output of other threads nor a bare
  `println!`. Tests of the binary (`tumult-cli/tests/` and
  `src/dispatch_tests.rs`) start `tumult` and read its streams instead.
- The suite passes under both `cargo nextest run` and `cargo test`.

## Ignored and skipped tests

`#[ignore = "reason"]` always carries a reason, and the reason names the CI
job that runs the test. The same holds for `#[cfg_attr(.., ignore = "reason")]`
and for Python `skip`, `skipif` and `xfail`. A test that cannot run in an
environment fails or is cfg-gated. It does not return early. `test.only`,
`it.only`, `describe.only`, `fdescribe` and `fit` are never committed.

## Duplicates

One behaviour, one test, at the lowest level that can observe it. A function
is tested in the crate that defines it.

## Deleting a test

The commit names the test that still covers the behaviour, or says why the
behaviour is not ours to cover.

## The guard

`scripts/check-test-hygiene.py` runs in CI and fails on:

| Rule | Meaning |
|---|---|
| `no-assert` | A Rust test (`#[test]`, `#[tokio::test]`, `#[rstest]`, `#[test_case]` and the like) that asserts nothing. See below for what counts. |
| `bare-ignore` | `#[ignore]` or `#[cfg_attr(.., ignore)]` without a reason string. |
| `bare-sleep` | `sleep(..)`, `sleep_until(..)` or `park_timeout(..)` in Rust test code, also under a renamed import, without a `// sleep:` comment of at least three words. A tokio sleep in a test on a paused clock (`start_paused = true` or `tokio::time::pause()`) is exempt. A `std::thread` sleep is not: the paused clock does not skip it. |
| `raw-env` | `env::set_var` or `env::remove_var` in Rust test code outside `tumult-test-utils`, also when `env` or the function is imported under another name. |
| `focused-test` | `test.only`, `it.only`, `describe.only` (also `.only.each` and `describe["only"]`), `fdescribe`, `fit` in JavaScript or TypeScript, outside comments and strings. |
| `bare-skip` | A Python `skip`, `skipif`, `skipIf`, `skipUnless` or `xfail` with no reason or an empty one: as a decorator, in `pytestmark`, in the `marks` of `pytest.param`, or as a `pytest.skip()` call. |
| `stale-allow` | An allowlist entry that matches nothing. |
| `bad-allow` | An allowlist entry that is not `path::name::rule`. |

What counts as an assertion:

- an `assert*!` macro that looks at a value. One that compares literals
  alone (`assert!(true)`, `assert!(1 == 1)`, `assert_eq!("a", "a")`) does not
  count, and neither does `debug_assert!`;
- `unwrap_err`, `expect_err`, `panic!` and `unreachable!`, so a `match` or
  `let .. else` that panics on the wrong variant is an assertion;
- `#[should_panic]`, and `#[test_case(input => expected)]`;
- `ensure!`, `bail!` or `return Err(..)` in a test that returns `Result`;
- clap's `debug_assert()` and the `assert()` and `assert_async()` of mockito
  and assert_cmd;
- a call to an `assert_*` function that is defined in the same crate, or in
  `tumult-test-utils`, and itself asserts.

A `_does_not_panic` test with a comment above it that names the panic is
exempt.

Rust test code, for the sleep and environment rules, is a test function, a
`#[cfg(test)] mod` block, a file under a `tests` directory or a file named
`tests.rs`.

Run it locally:

```sh
python3 scripts/check-test-hygiene.py
python3 scripts/test-check-test-hygiene.py   # the guard's own tests
```

The guard is a text scan, so it has limits:

- It cannot tell whether an assertion is meaningful. Review against this page
  does that.
- Helpers are matched by name within a top-level directory and in
  `tumult-test-utils`. An assertion
  inside a helper that is not named `assert_*` is not seen, and such a test
  is reported.
- A test file pulled in with `#[cfg(test)] mod x;` under another name than
  `tests.rs` is not checked for sleeps or environment changes.
- Renamed imports are followed for `use` items only. A function taken as a
  value (`let nap = sleep;`, `map(env::remove_var)`) is not seen.
- A closure that sleeps or changes the environment is reported whether or
  not it is called.
- Names are resolved per file, not per module: a file that imports
  `std::thread::sleep` has every bare `sleep(..)` treated as a thread sleep.

### Allowlist

`scripts/test-hygiene-allow.txt` lists known offenders, one
`path::test_name::rule  # reason` per line. An entry covers one rule for one
function. It exists so the guard could be switched on before every crate was
cleaned. Do not add to it. When a listed test is fixed or removed, the guard
fails with `stale-allow` until the line is deleted.
