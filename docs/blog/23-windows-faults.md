---
title: "Windows Chaos, For Real: Native Faults Nobody Else Ships"
parent: Blog
nav_order: 24
---

# Windows Chaos, For Real: Native Faults Nobody Else Ships

*2026-07-05*

Here's a gap in open-source chaos engineering that everyone quietly steps around: Windows. Chaos Mesh is Kubernetes. LitmusChaos is Kubernetes. The whole OSS ecosystem grew up on Linux containers, so if your resilience story includes a Windows service — a legacy .NET app, a Windows-only dependency, a fleet of Windows Server boxes — the open-source toolbox mostly shrugs. The commercial tools (Gremlin, Harness) do Windows; the free ones send you a link to their Kubernetes docs.

2.12.0 ships `tumult-windows`: native fault injection on Windows, from the same single binary, the same experiment format, the same journals. Three faults to start — not parity with a decade of Gremlin, but the ones that matter first.

## Three faults

```
   process_kill ─────► taskkill /F /IM <image>   (or /PID <pid>)
   cpu_stress ───────► N worker threads, busy-spin for <duration>
   network_blackhole ► netsh advfirewall … action=block   (+ rollback)
```

- **`process_kill`** — terminate a process by image name or PID. The bluntest fault there is: does your service come back when Windows kills it?
- **`cpu_stress`** — spin up N worker threads and saturate them for a duration. No external stress tool to install; the fault *is* the load.
- **`network_blackhole`** — drop traffic to a port or host by adding a Windows Firewall block rule, and — because chaos you can't undo isn't chaos, it's an outage — a rollback that deletes the rule when the experiment ends.

It's the fifth native plugin (`tumult-ssh`, `tumult-net`, `tumult-kubernetes`, `tumult-cloud`, and now `tumult-windows`), registered the same way, discovered the same way:

```
$ tumult discover
  ...
  tumult-windows (native)
    tumult-windows::process_kill
    tumult-windows::cpu_stress
    tumult-windows::network_blackhole
```

## The part we care about: it's not shipped blind

It's easy to write Windows code on Linux, compile it, and *hope*. We don't ship "hope." Every other fault domain in Tumult is validated live on the demo before release — and Windows should be no different just because the demo runs on Linux.

So it runs against a **real Windows 11 guest**. `winfault` — a tiny standalone binary that runs one fault and prints JSON — cross-compiles cleanly to `x86_64-pc-windows-gnu` (it carries no DuckDB, so there's no C++ toolchain fight), gets dropped into the guest, and each fault is exercised for real:

```
process_kill      start notepad ─► confirm running ─► winfault process-kill ─► confirm GONE
                  {"success":true,"stdout":"SUCCESS: The process \"Notepad.exe\" ... terminated"}

cpu_stress        guest CPU baseline ~3%  ─►  during the run: sustained ~50%  ─►  recovers
                  (1 worker on a 2-core guest = one core pinned; the guest's own
                   metrics endpoint measures the spike, then relaxes)

network_blackhole add rule ─► netsh confirms it's present ─► rollback ─► "Deleted 1 rule(s)."
```

That CPU line is the fun one. Drive the guest hard enough and its *own* telemetry endpoint stops answering — the machine is too busy being stressed to report how stressed it is. Back off to a single worker and you get a clean, measured number: ~3% at rest, a steady ~50% while one core is pinned, back to baseline when the fault lifts. That's a fault doing exactly what it says on a machine that actually exists.

## What it is and isn't

It's three faults, built on Windows' own tools (`taskkill`, `netsh`, and plain Rust threads), so the command construction is unit-tested on Linux (24 tests) and the execution is proven on Windows. It needs a Windows host to *run* — the plugin is discoverable everywhere, but the faults do their work where Windows lives.

It isn't a decade of parity. There's no WFP-level packet surgery, no ETW tricks, no service-control-manager gymnastics yet. Those can come. What's here is the first slice that earns its keep: the faults an SRE reaches for first, on the platform the OSS world keeps pretending isn't there — and proven against a running copy of it.

Windows was the one box on the roadmap nobody in open source ticks. It's ticked.
