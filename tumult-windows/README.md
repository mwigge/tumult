# tumult-windows

Windows-native fault injection for the Tumult chaos-engineering platform.

`tumult-windows` implements the `NativeExecutor` trait (like `tumult-ssh`,
`tumult-net`, `tumult-kubernetes`, and `tumult-cloud`) and is registered in the
CLI composition root, so `tumult discover` lists it and its functions
automatically.

Faults are injected by driving **built-in Windows tools** (`taskkill`, `netsh`)
through `std::process::Command`, plus a self-contained CPU busy-spin. There is
no raw Win32/WFP code, so the crate cross-compiles cleanly to
`x86_64-pc-windows-gnu` and its command construction is unit-testable on Linux.

## Faults

| Function (`tumult-windows::…`) | Effect | Arguments |
|--------------------------------|--------|-----------|
| `process_kill` | Terminate a process: `taskkill /F /IM <image>` or `taskkill /F /PID <pid>` | `image` **or** `pid` (exactly one) |
| `cpu_stress` | Spin N CPU-bound threads for a duration (no external tool; observable via CPU metrics) | `workers` (default: host parallelism, else 2), `duration_secs` (default: 10) |
| `network_blackhole` | Block outbound TCP via the Windows firewall: `netsh advfirewall firewall add rule name=<n> dir=out action=block remoteport=<port> protocol=TCP`. Returns the exact rollback command. | `port` **or** `remote_host` (exactly one) |

The blackhole rollback deletes the rule it created:
`netsh advfirewall firewall delete rule name=<n>`. The rule name is
deterministic (`tumult-blackhole-port-<port>` / `tumult-blackhole-host-<host>`),
and `network_blackhole`'s result includes the ready-to-run rollback command.

## Execution requires a Windows host

`process_kill` and `network_blackhole` only take effect where `taskkill` and
`netsh` exist. On Linux they return a typed `WindowsError::Spawn` (used by the
unit tests to prove the execution path without a Windows box). `cpu_stress` is
pure Rust and runs on any platform. `netsh` rule changes require Administrator
privileges on the guest.

The plugin is validated live against a real **Windows 11 guest**: the
orchestrator cross-compiles the `winfault` binary to `x86_64-pc-windows-gnu`,
copies it into the guest, and runs each fault.

## `winfault` standalone binary

A tiny, dependency-light binary (`src/bin/winfault.rs`) runs a single fault and
prints a JSON result to stdout (exit non-zero on failure). This is what the
orchestrator cross-compiles and runs inside the guest.

```text
winfault process-kill --image notepad.exe
winfault process-kill --pid 4321
winfault cpu-stress --workers 4 --duration-secs 30
winfault network-blackhole --port 443
winfault network-blackhole --remote-host 10.0.0.5
winfault network-blackhole-rollback --rule-name tumult-blackhole-port-443
```

Example output:

```json
{"success":true,"fault":"cpu-stress","result":{"workers":4,"requested_secs":30.0,"elapsed_secs":30.01}}
```

## Design: construction vs. execution

- `commands.rs` is **pure** — it turns validated arguments into the exact
  program + argument vector, with no side effects. It is exhaustively unit-
  tested on Linux (`build_taskkill_args`, blackhole add/delete rule strings).
- `faults.rs` **executes** those vectors and owns the cross-platform CPU spin.

## Cross-compilation

```sh
rustup target add x86_64-pc-windows-gnu
cargo build -p tumult-windows --bin winfault --target x86_64-pc-windows-gnu
```

Dependencies are kept minimal (`tumult-plugin`, `async-trait`, `thiserror`,
`serde_json`) — no `tumult-analytics`/DuckDB or other heavy C dependencies — so
the windows-gnu cross-build stays clean.
