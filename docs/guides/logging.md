# Logging

Every Tumult binary writes its diagnostics as structured events through one
subscriber, installed by `tumult_otel::logging::init`. This guide describes
the output, how to configure it, the field vocabulary, and how to add an
event.

Logs are diagnostics. Command results (`tumult run` output, reports, the MCP
JSON-RPC stream) are the product of a command and go to stdout; logs never
do.

## Output

| | |
|---|---|
| Stream | stderr, always |
| Format | one JSON object per line; one text line per event when stderr is a terminal |
| Export | the same events are sent as OTLP log records when an OTLP endpoint is configured. Export ends when telemetry is flushed at shutdown (in `tumultd`: on the shutdown signal, while its own OTLP listener still accepts); later events are on stderr only |

A JSON line:

```json
{"timestamp":"2026-11-13T09:14:02.118334Z","level":"WARN","target":"tumult_clickhouse::store","event.name":"clickhouse.connect.retrying","message":"ClickHouse connection failed, retrying","service.name":"tumultd","service.version":"2.22.0","trace_id":"4bf92f3577b34da6a3ce929d0e0e4736","span_id":"00f067aa0ba902b7","url.full":"http://clickhouse.internal:8123","attempt":2,"max_attempts":3,"backoff_s":4,"error.message":"connection refused"}
```

Keys the formatter owns:

| Key | Present | Meaning |
|---|---|---|
| `timestamp` | always | RFC 3339, UTC, microseconds |
| `level` | always | `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR` |
| `target` | always | Rust module path of the event |
| `event.name` | when the event has a `name:` | stable event name, see below |
| `message` | always | constant, human-readable sentence |
| `service.name` | always | `tumultd`, `tumult`, `tumult-net-proxyd`, ... |
| `service.version` | always | version of the binary |
| `deployment.environment.name` | when configured | from `OTEL_RESOURCE_ATTRIBUTES` or `TUMULT_ENVIRONMENT` |
| `trace_id`, `span_id` | inside an OpenTelemetry span | W3C ids of the active span |
| `spans` | inside `tracing` spans | enclosing spans with their fields, outermost first |

Everything else on the line is an event field. An event field that uses one
of the names above is emitted as `field.<name>` so it cannot overwrite the
formatter's value.

Guarantees of the JSON format:

- One event is one line. Newlines and every other control character inside a
  value are escaped.
- Field values keep their type: numbers are JSON numbers, booleans are JSON
  booleans.
- Any single string value is cut at 8 KiB (`MAX_FIELD_BYTES`) and suffixed
  with `...[truncated N bytes]`.

## Configuration

| Variable | Effect |
|---|---|
| `RUST_LOG` | Level filter, in `tracing` directive syntax (`debug`, `info,tumult_ingest=debug`). Replaces the default filter entirely. An unparseable value is reported as `logging.filter.invalid` and the default is used. |
| `TUMULT_LOG_FORMAT` | `json` or `text`. Unset: text when stderr is a terminal, JSON otherwise. An unknown value falls back to JSON and is reported as `logging.format.invalid`. |
| `NO_COLOR` | Disables colour in the text format. |
| `TUMULT_ENVIRONMENT` | Value of `deployment.environment.name` when `OTEL_RESOURCE_ATTRIBUTES` does not set it. |
| `OTEL_RESOURCE_ATTRIBUTES` | `deployment.environment.name=...` is stamped on every line. |
| `OTEL_SERVICE_NAME` | Overrides `service.name` (default `tumultd` for the daemon, `tumult` for `tumult` and `tumult-mcp`). |

Default levels when `RUST_LOG` is unset:

| Binary | Default |
|---|---|
| `tumultd` | `info` |
| `tumult-net-proxyd` | `info` |
| `tumult`, `tumult-mcp` | `warn`; `info` when an OTLP endpoint is configured |

With the default filter, `opentelemetry*`, `tonic`, `hyper*`, `h2`, `tower*`,
`reqwest` and `rustls` are capped at `warn`. Events from the exporter's own
stack are never exported over OTLP, whatever `RUST_LOG` says, so the exporter
cannot feed itself.

### Process start and fatal errors

`init` is the first thing `main` does. If it fails, the binary writes one
JSON line by hand (`logging.init.failed`) and continues without logs rather
than refusing to start.

An error that ends the process is the last event on stderr:

```json
{"timestamp":"...","level":"ERROR","target":"tumult_otel::logging","event.name":"process.fatal","message":"fatal error","service.name":"tumult","service.version":"2.22.0","error.message":"reading experiment missing.toon: No such file or directory (os error 2)"}
```

The exit code is non-zero.

## Event names

Every event has a `name:`. It is the identity of the event: dashboards,
alerts and tests match on it, so it does not change when the message is
reworded.

- lowercase, dot-separated, at least two segments: `component.thing.happened`
- first segment is the component: `daemon`, `auth`, `lake`, `otel`, `cli`, `mcp`, `net`, `ssh`,
  `report`, `agent_cli`, `agentic`, `clickhouse`, `cypher`, `plugin`,
  `logging`, `process`
- last segment says what happened, in the past tense or as a state:
  `failed`, `skipped`, `rejected`, `retrying`, `timed_out`

The message is a constant. Anything that varies is a field.

```rust
// Wrong: the value is in the message, the event has no name.
tracing::warn!("journal {} skipped: {e}", path.display());

// Right.
tracing::warn!(
    name: "cli.journal.skipped",
    {
        file.path = %path.display(),
        error.message = %tumult_otel::logging::err(&error),
    },
    "journal skipped, it could not be read"
);
```

## Field vocabulary

Use an OpenTelemetry semantic-convention name when one exists. Control-plane
identifiers use the `resilience.` prefix, the same names the spans carry.

| Field | Type | Use |
|---|---|---|
| `error.message` | string | the error, rendered with `logging::err` (full `anyhow` chain) or `logging::err_sources` (walks `source()`) |
| `error.type` | string | short, bounded class: `io`, `timeout`, `join`, `llm`, or `logging::error_type(&e)` |
| `exception.message` | string | payload of a caught panic |
| `resilience.experiment.id` | string | experiment id |
| `resilience.experiment.name` | string | experiment title |
| `resilience.run.id` | string | run id |
| `resilience.outcome.status` | string | run outcome |
| `user.name` | string | authenticated or attempted user name |
| `client.address` | string | peer address of an inbound request |
| `server.address`, `server.port` | string, int | an address this process listens on or connects to |
| `url.full` | string | a URL, always through `logging::sanitize_url` |
| `url.path` | string | path only, never the query string |
| `http.request.method` | string | |
| `http.response.status_code` | int | |
| `http.response.body.size` | int | bytes |
| `file.path` | string | a file or directory |
| `config.key` | string | name of a setting or environment variable |
| `config.value` | string | its value, never for a secret |
| `process.pid` | int | |
| `process.exit.code` | int | |
| `process.executable.name` | string | |
| `process.stream` | string | `stdout` or `stderr` of a child process |
| `gen_ai.tool.name` | string | MCP or agent tool name |
| `db.namespace` | string | database name |
| `signal` | string | `SIGINT`, `SIGTERM` |
| `attempt`, `max_attempts` | int | retry counters |
| `*_ms`, `*_s`, `*_bytes` | number | durations and sizes carry their unit as a suffix |

Names the vocabulary replaced (`error`, `e`, `err`, `run_id`,
`experiment_id`, `path`, `url`, `endpoint`, `addr`, `key`, `env_key`,
`status`, `method`, `tool`, `user`, ...) are rejected by
`scripts/check-log-messages.py`, which names the replacement.

`tracing` needs field names to be literals in the macro call, so the names
are not Rust constants; the checker and this table are the source of truth.

## What must never be logged

- Passwords, tokens, API keys, session ids, cookies, `Authorization`
  headers, private keys. Log the name of the setting (`config.key`), never
  its value.
- A URL that was not passed through `logging::sanitize_url` (removes
  userinfo, query and fragment). An error text that may quote a URL goes
  through `logging::redact_url`.
- Query strings. Log `url.path`.
- Request and response bodies, prompts, model output.
- stdout and stderr of child processes and plugin scripts. Log their sizes
  (`stdout_bytes`, `stderr_bytes`) and the exit code.
- Query text (SQL, Cypher). Log a bounded classification such as
  `cypher.clause`.
- Unbounded values taken from a request. The formatter caps every string at
  8 KiB; cut request-controlled values shorter at the call site when a
  smaller bound is known.

## Adding an event

1. Write the test first. `tumult_otel::logging::testing::capture_json` runs
   a closure and returns the events it logged, parsed:

   ```rust
   let logs = capture_json(tracing::Level::WARN, || {
       journal_skipped(Path::new("a.toon"), &error);
   });
   let event = &logs.named("cli.journal.skipped")[0];
   assert_eq!(event["level"], "WARN");
   assert_eq!(event["file.path"], "a.toon");
   ```

   In an async test use `LogCapture::start(level)` and `.snapshot()`.
2. Give the event a name, a constant message and vocabulary fields.
3. Put the event in a small function next to the code that raises it when it
   has more than a couple of fields, so the call site stays readable and the
   test can call it directly.
4. Run `python3 scripts/check-log-messages.py`.

Pick the level by who has to act:

| Level | Use |
|---|---|
| `ERROR` | work was lost or the process cannot continue |
| `WARN` | something failed and was handled, or a configuration is unsafe |
| `INFO` | lifecycle: started, listening, stopped; one line per unit of work at most |
| `DEBUG` | detail for a developer; expected failures such as a peer that hung up |

An error is either returned or logged, never dropped. `let _ = fallible();`
needs a comment that says why the failure does not matter.

## Printing

`println!`, `eprintln!`, `print!`, `eprint!` and `dbg!` are denied by clippy
in every crate (`clippy::print_stdout`, `clippy::print_stderr`,
`clippy::dbg_macro`). Where output is the product of a command, the function
or module carries an `#[allow]` with a comment saying so: the result
printing of `tumult`, the admin and report subcommands of `tumultd`, and
`winfault`. Tests may print (`allow-print-in-tests` in `clippy.toml`).

## Child processes

A daemon's stderr is its log stream. `tumult-net-proxyd` logs JSON to stderr;
the parent keeps that stream in `<tmp>/tumult-net-<key>.stderr.log` while the
proxy runs and removes it when the proxy is stopped. When the proxy fails to
start, the parent reports the `error.message` of its last log line.

Output of plugin scripts and agent CLIs is data, not log: it is captured,
bounded, and returned to the caller.

## Crates not converted yet

`tumult-api`, `tumult-compliance`, `tumult-core`, `tumult-graph`,
`tumult-ingest` and `tumult-lake` still log through the same subscriber, so
their output is JSON with typed fields, but their events have no `name:` yet
and some still use the replaced field names. They are listed in `DEFERRED`
in `scripts/check-log-messages.py`; converting a crate means removing its
entry and fixing what the checker then reports.
