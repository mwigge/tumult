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
with `// sleep:` and gives the reason.

## Isolation

- No fixed ports. Bind to port 0 and read the address back.
- No real `$HOME`, no network beyond loopback.
- Environment variables are changed only through
  `tumult_test_utils::EnvGuard`. It holds a process-wide lock and restores the
  previous values when dropped. `EnvGuard::lock()` is not re-entrant: a second
  call on the same thread while the first guard is alive deadlocks. Take one
  guard per test and pass it to helpers.
- Spans are asserted with `tumult_test_utils::capture_spans`, which returns
  only the spans of the calling test. Metrics are asserted with
  `tumult_test_utils::MetricCapture`, which owns a private meter provider. Do
  not install a global tracer or meter provider in a test. Code that calls
  `set_tracer_provider` itself is tested in a test binary of its own, because
  the global provider is shared by every test in a binary.
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
| `bare-sleep` | `sleep(..)` in Rust test code without a `// sleep:` comment. A test on a paused tokio clock (`start_paused = true` or `tokio::time::pause()`) is exempt. |
| `raw-env` | `env::set_var` or `env::remove_var` in Rust test code outside `tumult-test-utils`. |
| `focused-test` | `test.only`, `it.only`, `describe.only`, `fdescribe`, `fit` in JavaScript or TypeScript, outside comments and strings. |
| `bare-skip` | A Python `skip`, `skipif`, `skipIf`, `skipUnless` or `xfail` decorator, or a `pytest.skip()` call, with no reason or an empty one. |
| `stale-allow` | An allowlist entry that matches nothing. |
| `bad-allow` | An allowlist entry that is not `path::name::rule`. |

What counts as an assertion:

- an `assert*!` macro, except on the literal `true`;
- `unwrap_err`, `expect_err`, `panic!` and `unreachable!`, so a `match` or
  `let .. else` that panics on the wrong variant is an assertion;
- `#[should_panic]`, and `#[test_case(input => expected)]`;
- `ensure!`, `bail!` or `return Err(..)` in a test that returns `Result`;
- clap's `debug_assert()` and the `assert()` and `assert_async()` of mockito
  and assert_cmd;
- a call to an `assert_*` function that is defined in the same crate and
  itself asserts.

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
- Helpers are matched by name within a top-level directory. An assertion
  inside a helper that is not named `assert_*` is not seen, and such a test
  is reported.
- A test file pulled in with `#[cfg(test)] mod x;` under another name than
  `tests.rs` is not checked for sleeps or environment changes.
- `set_var` imported and called without the `env::` prefix is not seen.

### Allowlist

`scripts/test-hygiene-allow.txt` lists known offenders, one
`path::test_name::rule  # reason` per line. An entry covers one rule for one
function. It exists so the guard could be switched on before every crate was
cleaned. Do not add to it. When a listed test is fixed or removed, the guard
fails with `stale-allow` until the line is deleted.
