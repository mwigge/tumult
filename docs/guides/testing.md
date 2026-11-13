# Testing policy

A test exists to fail when behaviour we own changes. This page says what that
means in this repository and what the CI guard enforces.

## What a test must assert

Each test asserts at least one observable outcome of our code: a returned
value, a persisted row, a request sent, a span or metric emitted, a rendered
string, an error variant.

"It did not panic" is acceptable only when not panicking is the contract. Then
the name ends in `_does_not_panic` and a comment above the test says which
panic it guards.

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

## Isolation

- No fixed ports. Bind to port 0 and read the address back.
- No real `$HOME`, no network beyond loopback.
- Environment variables are changed only through
  `tumult_test_utils::EnvGuard`. It holds a process-wide lock and restores the
  previous values when dropped.
- Spans are asserted with `tumult_test_utils::capture_spans`, which returns
  only the spans of the calling test. Metrics are asserted with
  `tumult_test_utils::MetricCapture`, which owns a private meter provider. Do
  not install a global tracer or meter provider in a test.
- The suite passes under both `cargo nextest run` and `cargo test`.

## Ignored and skipped tests

`#[ignore = "reason"]` always carries a reason, and the reason names the CI
job that runs the test. A test that cannot run in an environment fails or is
cfg-gated. It does not return early. `test.only`, `it.only`, `describe.only`,
`fdescribe` and `fit` are never committed.

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
| `no-assert` | A Rust test body with no `assert*!` macro, `assert_*` helper call, `unwrap_err`, `expect_err`, `panic!` or `#[should_panic]`. A `_does_not_panic` test with a comment above it is exempt. |
| `bare-ignore` | `#[ignore]` without a reason string. |
| `focused-test` | `test.only`, `it.only`, `describe.only`, `fdescribe`, `fit` in JavaScript or TypeScript. |
| `bare-skip` | `@pytest.mark.skip` or `@unittest.skip` without a reason. |

Run it locally:

```sh
python3 scripts/check-test-hygiene.py
python3 scripts/test-check-test-hygiene.py   # the guard's own tests
```

The guard is a text scan. It cannot tell whether an assertion is meaningful;
review against this page does that.

### Allowlist

`scripts/test-hygiene-allow.txt` lists known offenders, one
`path::test_name  # reason` per line. It exists so the guard could be switched
on before every crate was cleaned. Do not add to it. When a listed test is
fixed or removed, the guard prints a note; delete the line in the same change.
