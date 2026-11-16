---
title: Logging
parent: Guides
nav_order: 29
---

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
| Export | the same events are sent as OTLP log records when an OTLP endpoint is configured. In `tumultd` export begins once its own OTLP address is bound; earlier startup events are on stderr only. Export ends when telemetry is flushed at shutdown (in `tumultd`: on the shutdown signal, while its own OTLP listener still accepts); later events are on stderr only |

A JSON line:

```json
{"timestamp":"2026-11-13T09:14:02.118334Z","level":"WARN","target":"tumult_clickhouse::store","event.name":"clickhouse.connect.retrying","message":"ClickHouse connection failed, retrying","service.name":"tumultd","service.version":"3.0.1","trace_id":"4bf92f3577b34da6a3ce929d0e0e4736","span_id":"00f067aa0ba902b7","url.full":"http://clickhouse.internal:8123","attempt":2,"max_attempts":3,"backoff_s":4,"error.message":"connection refused"}
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
  with `...[truncated N bytes]`. The cut is made once, before the event
  reaches a sink, so the exported OTLP record carries the same value as the
  line on stderr.
- A closed or broken stderr never ends the process: a line that cannot be
  written is dropped.

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

`tumultd` receives its own export. For the same reason the events of its
ingest path (`tumult_ingest::writer`, `tumult_ingest::grpc`,
`tumult_ingest::http`, `tumult_ingest::otap`, `tumult_ingest::listener`,
`tumult_ingest::log_guard`, `tumult_lake::writer`, `tumult_otlp::common`) are on
stderr only: a record that cannot be persisted would otherwise be logged,
exported to the same ingest, fail again and be logged again. A binary names
such targets with `LogOptions::with_local_only_targets`.

### Process start and fatal errors

`init` is the first thing `main` does. If it fails, the binary writes one
JSON line by hand (`logging.init.failed`) and continues without logs rather
than refusing to start.

An error that ends the process is the last event on stderr:

```json
{"timestamp":"...","level":"ERROR","target":"tumult_otel::logging","event.name":"process.fatal","message":"exiting because of an unrecoverable error","service.name":"tumult","service.version":"3.0.1","error.message":"reading experiment missing.toon: No such file or directory (os error 2)"}
```

The exit code is non-zero. A panic is reported the same way, as
`process.panic` with `exception.message`.

These two events do not depend on the filter. With `RUST_LOG=off`, or a
filter that names other targets only, the line is written to stderr
directly, as JSON, exactly once. `tumultd` logs `process.fatal` before it
flushes telemetry, so the event is also exported while the OTLP listener is
still up.

The standard panic message and backtrace are printed after the event only
when `RUST_BACKTRACE` is set to something other than `0`.

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

The checker reads spans as well as events: a span macro (`info_span!`,
`span!`, ...) or an `#[instrument(name = "..")]` with a name that is not
lowercase dotted is a `bad-span-name`, and span fields follow the same
vocabulary. An `#[instrument]` without a name is named after its function
and is not reported. A macro imported under another name
(`use tracing::info as i;`) is checked under that name. An `error.message`
put together with `format!`, `format_args!` or `concat!` is a
`computed-error-message`: pass the error itself (`%e`, or `format!("{e:#}")`
for another rendering) and put what varies in fields of its own.

`tracing` needs field names to be literals in the macro call, so the names
are not Rust constants; the checker and this table are the source of truth.

## What must never be logged

- Passwords, tokens, API keys, session ids, cookies, `Authorization`
  headers, private keys. Log the name of the setting (`config.key`), never
  its value.
- A URL that was not passed through `logging::sanitize_url` (removes
  userinfo, query, fragment and `;` path parameters). A URL it cannot read
  for certain is logged as `[redacted-url]` and no part of it is shown;
  that includes any URL with an `@` after the first `/`, `?` or `#`, which
  is either a path with an `@` in it or a password with an unescaped
  delimiter, and any URL with a scheme the sanitiser does not know
  (`alice://password@host`: what stands before `://` may be a user name).
  The schemes it knows are `amqp`, `amqps`, `clickhouse`, `ftp`, `ftps`,
  `grpc`, `grpcs`, `h2c`, `http`, `https`, `kafka`, `mongodb`, `mqtt`,
  `mysql`, `nats`, `otlp`, `postgres`, `postgresql`, `redis`, `rediss`,
  `s3`, `sftp`, `ssh`, `tcp`, `tls`, `tumult`, `udp`, `ws` and `wss`.
  `file` is not one of them: a `file://` URL is logged as `[redacted-url]`;
  log a path as `file.path`.
  `logging::server_address` answers the same way for `server.address`. An
  error text that may quote a URL goes through `logging::redact_url`, which
  replaces the user name, the password and the query, fragment and path
  parameter values of the URL (three bytes or longer) with `[redacted]`
  wherever they stand, raw or percent-encoded, and as a lossy conversion
  spells them when they are not UTF-8. Every `@` of the URL is taken as a
  possible end of the userinfo. A user name of one or two bytes is replaced
  only where it is a word of its own, that is where no ASCII letter or
  digit stands next to it. The text is searched and kept up to 8 KiB, the
  most a field holds; the rest is replaced by `...[truncated N bytes]`. A
  credential of more than 4096 bytes, or credentials of more than 16 KiB
  together, are not searched for: the whole text becomes `[redacted]`.
  `logging::redact_credentials` does the same with a user name and a
  password that are configured beside the URL. The ClickHouse store uses
  it for the error text of a connection retry, so the configured user
  name is `[redacted]` there, and so is the built-in `default` when no
  other is configured: a retry warning that says `[redacted]` where a
  word such as "default" is expected has lost nothing else.
- Query strings. Log `url.path`.
- Request and response bodies, prompts, model output.
- stdout and stderr of child processes and plugin scripts. Log their sizes
  (`stdout_bytes`, `stderr_bytes`: what the process wrote, also when less
  was kept) and the exit code.
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
or module carries an `#[allow]` with a comment saying so: the admin and
report subcommands of `tumultd`, and `winfault`. Tests may print and use
`dbg!` (`allow-print-in-tests` and `allow-dbg-in-tests` in `clippy.toml`).

`tumult` prints its results with `outln!`, `out!` and `errln!`
(`tumult_cli::output`), not with the `std` macros. Those panic when the
reader has gone away (`tumult run x.toon | head -1`), and a panic is an
abort in the release profile: the run would end before its journal is
ingested. The `output` macros give up on a stream that fails (a
`cli.output.closed` debug event for a closed pipe, a
`cli.output.write_failed` warning for anything else) and the command
finishes its work. Four lines of `tumult store` output still use `println!`
and are to be converted.

### Human diagnostics that stay text

Three commands write to stderr for the person who ran them, as plain text
and not as events, whatever `TUMULT_LOG_FORMAT` says:

| Command | What it writes to stderr |
|---|---|
| `tumult validate` | findings about the experiment: an unknown plugin or function, a plugin search path that was skipped |
| `tumult discover` | plugin discovery warnings that annotate the listing |
| `tumult analyze` | `warning: skipping <file>: <cause>` for a journal it could not load |

They are part of what the command reports, read next to its stdout, and no
machine is expected to parse them. Anything else these commands have to say
(a fatal error, a failure in a library they call) is an event like
everywhere else. Do not add to this list: a new diagnostic is an event.

## Child processes

A daemon's stderr is its log stream. `tumult-net-proxyd` logs JSON to stderr;
the parent keeps that stream in `tumult-net-<key>.log.d/stderr.log` while
the proxy runs, under `$XDG_RUNTIME_DIR` when that is set and usable and
under the temp directory otherwise. The directory is created for the proxy
with mode `0700` and the file with `0600`, never through an existing file or
link. The directory is opened once without following a link and checked
through that handle: one that belongs to another user, or that others can
enter, is refused. The file is created, read and removed relative to the
handle, so swapping the directory's name for a link afterwards redirects
nothing; stopping the proxy checks the directory the same way before it
removes anything from it. Nothing rotates the
file, so the daemon stops writing to it after 1 MiB
(`LogOptions::with_output_limit`). The event that does not fit and every
later one are dropped whole, and one `logging.output.limit_reached` warning
is written in their place, so the file says where it stops. The last 32 KiB
of the limit are kept back for that line and for the daemon's last words:
its `process.fatal` and `process.panic` events are still written after it.
The file and its directory are removed when
the proxy is stopped and when it fails to start. In that case the parent
reads at most the first 8 KiB and reports the `error.message` of the last
log line in it.

Output of plugin scripts and agent CLIs is data, not log: it is captured,
bounded, and returned to the caller. A script plugin's stream is cut at
8 MiB on a character boundary and nothing is written into it; the result
says so (`stdout_truncated`, `stderr_truncated`) and
`plugin.script.output.truncated` carries the size the script wrote
(`bytes_total`). A script that exits non-zero is an observation about the
system under test, logged at `debug` as `plugin.script.exited`; a timeout
and a stream that could not be read are warnings.

## Shutdown flush

A stopping `tumultd` exports its pending spans, metrics and log events
before the listener that receives them goes away, and waits at most 15
seconds for that. An export that hangs longer is given up on with a
`daemon.telemetry.flush.timed_out` warning (`timeout_ms`), and the shutdown
goes on. The process then exits at most 5 seconds after its work is done,
whether or not that export has returned.

## Crates not converted yet

`tumult-api`, `tumult-compliance`, `tumult-core`, `tumult-graph`,
`tumult-ingest` and `tumult-lake` still log through the same subscriber, so
their output is JSON with typed fields, but their events have no `name:` yet
and some still use the replaced field names. They are listed in `DEFERRED`
in `scripts/check-log-messages.py`, which prints that list with its length
on every run; converting a crate means removing its entry and fixing what
the checker then reports.
