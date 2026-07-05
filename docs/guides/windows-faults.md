---
title: Windows Faults
parent: Guides
nav_order: 21
---

# Windows Faults

`tumult-windows` is a native plugin that injects faults on Windows hosts — the
fault domain the open-source chaos ecosystem (Kubernetes-centric Chaos Mesh,
LitmusChaos) does not cover. It shipped in **2.12.0** and is validated live
against a real Windows 11 guest.

It is the fifth native executor (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`,
`tumult-cloud`, `tumult-windows`), discovered and invoked exactly like the others.
The plugin is listed by `tumult discover` on any platform; the faults **execute on
a Windows host** (they shell out to `taskkill` / `netsh` or spin CPU threads).

## The three faults

| Function | What it does | Arguments |
|----------|--------------|-----------|
| `process_kill` | Terminate a process (`taskkill /F`) | `image` **or** `pid` (exactly one) |
| `cpu_stress` | Saturate CPU with worker threads | `workers` (default: host parallelism), `duration_secs` (default: 10) |
| `network_blackhole` | Block a port/host via Windows Firewall | `port` **or** `remote_host` (exactly one) |

`network_blackhole` returns the exact rollback command in its result and the
plugin removes the firewall rule when the experiment's rollback runs
(`netsh advfirewall firewall delete rule name=<rule>`).

## Example experiment

```toon
version: v1
title: Windows service survives a process kill
steady-state:
  probes[1]:
    - name: service-healthy
      type: probe
      provider:
        type: process
        arguments[3]: ["cmd", "/c", "sc query MyService | findstr RUNNING"]
method[1]:
  - name: kill-the-service
    type: action
    provider:
      type: native
      plugin: tumult-windows
      function: process_kill
      arguments:
        image: MyService.exe
```

`tumult validate` and `tumult run` work as they do for any experiment; the run
produces a normal journal that flows into analytics, compliance, and ChaosGraph.

## The `winfault` binary

The crate also builds a small standalone binary, `winfault`, for direct
in-guest use and validation. It runs one fault and prints a JSON result:

```
winfault process-kill --image notepad.exe
winfault cpu-stress --workers 4 --duration-secs 30
winfault network-blackhole --port 443
winfault network-blackhole-rollback --rule-name tumult-blackhole-port-443
```

It carries no DuckDB dependency, so it cross-compiles cleanly to
`x86_64-pc-windows-gnu` (from Linux with `gcc-mingw-w64` and
`rustup target add x86_64-pc-windows-gnu`, or natively on Windows).

## How it's validated

Command construction (the `taskkill` / `netsh` argument vectors, rule-name
derivation, arg parsing) is unit-tested on Linux — 24 tests — so correctness is
covered in CI without a Windows box. Execution is proven against a running
Windows 11 guest: `process_kill` terminates a live process and confirms it is
gone; `cpu_stress` drives measured guest CPU from baseline to a sustained load
and recovers; `network_blackhole` adds a firewall rule, confirms it via `netsh`,
and rolls it back.

## Scope

Three faults, built on Windows' own tools — not parity with a mature commercial
Windows chaos suite. There is no WFP packet manipulation, ETW, or
service-control-manager fault yet. What ships is the first, most-reached-for
slice, proven on real Windows.
