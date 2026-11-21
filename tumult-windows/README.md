# tumult-windows

Windows faults for Tumult: `process_kill`, `cpu_stress`,
`network_blackhole` and its rollback. `WindowsExecutor` runs them as the
`tumult-windows` native plugin, registered in `tumult-exec` like the other
native plugins. What each function does is on the
[plugin page](../docs/plugins/tumult-windows.md).

## How it is built

The crate calls the tools Windows ships with (`taskkill`, `netsh`) through
`std::process::Command`, and spins the CPU in Rust. It has no Win32 or WFP
code, so it cross-compiles to `x86_64-pc-windows-gnu`.

- `commands.rs` turns checked arguments into the exact program and
  argument list. It has no side effects, so tests on Linux check every
  `taskkill` and `netsh` command and every rule name.
- `faults.rs` runs those commands and holds the CPU spin.
- On Linux, `process_kill` and `network_blackhole` fail with
  `WindowsError::Spawn`, which the tests use to prove the execution path.
  `cpu_stress` runs anywhere.

The crate depends only on `tumult-plugin`, `async-trait`, `thiserror` and
`serde_json`. No DuckDB, so the cross build stays clean.

## winfault

`src/bin/winfault.rs` runs one fault and prints a JSON result. It exits
with a code other than 0 when the fault fails.

```text
winfault process-kill --image notepad.exe
winfault process-kill --pid 4321
winfault cpu-stress --workers 4 --duration-secs 30
winfault network-blackhole --port 443
winfault network-blackhole --remote-host 10.0.0.5
winfault network-blackhole-rollback --rule-name tumult-blackhole-port-443
```

```json
{"success":true,"fault":"cpu-stress","result":{"workers":4,"requested_secs":30.0,"elapsed_secs":30.01}}
```

## Cross-compile

From Linux, install `gcc-mingw-w64` for the linker, then:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build -p tumult-windows --bin winfault --target x86_64-pc-windows-gnu
```

The live check copies this `winfault` into a Windows 11 guest and runs
each fault there.
