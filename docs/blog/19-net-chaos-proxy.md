---
title: "Chaos Without Root: The tumult-net Proxy"
parent: Blog
nav_order: 20
updated: 2026-07-21
---

# Chaos Without Root: The tumult-net Proxy

*2026-07-04*

Network chaos has always had a bouncer at the door. `tc netem` wants root. `iptables` wants `NET_ADMIN`. Container-scoped tooling wants a Docker socket. Great tools; but the moment you try to run a latency experiment inside a locked-down CI container, the door closes.

`tumult-net`; shipped in 1.5.0, and dispatched through the native plugin registry since 2.0.0; takes a different route entirely. It's a userspace TCP chaos proxy, built in Rust on [`tokio-netem`](https://docs.rs/tokio-netem). No kernel modules. No capabilities. No sidecars.

If your process can open a socket, it can inject faults.

## Where it sits

The trick is old and simple: don't touch the network; *be* the network. Point your client at the proxy instead of the real service, and every byte flows through a fault stack on its way to the upstream.

```
                        the only privilege required:
                          "may open a TCP socket"

┌──────────┐        ┌─────────────────────────┐        ┌──────────────┐
│  client   │        │   tumult-net proxy       │        │ real service  │
│  under    │──TCP──►│                          │──TCP──►│               │
│  test     │        │  delay · throttle ·      │        │ 127.0.0.1:8080│
│           │◄───────│  slice · flip · kill     │◄───────│               │
└──────────┘        │  (faults injected here)   │        └──────────────┘
 connects to        └─────────────────────────┘
 127.0.0.1:18080      plain userspace process
                      ┌─────────────────────┐
                      │  no root             │
                      │  no tc / iptables    │
                      │  no NET_ADMIN        │
                      │  no docker socket    │
                      └─────────────────────┘
```

Each accepted connection is split into its two halves, and the write side of each direction gets wrapped in `tokio-netem`'s I/O adapters. Faults are *directional*; latency is a one-way delay added to writes, which is how real asymmetric network pain actually behaves.

One scope note: this is a **TCP** proxy. That's the layer it lives at, and that's the layer it faults.

## The fault menu

Seven native functions. Five faults, one composite, one rollback.

```
 delay          ┌─ inject_latency ──── one-way delay + deterministic jitter
                │
 bandwidth      ├─ throttle_bandwidth ─ leaky-bucket egress limit (bytes/sec)
                │
 fragmentation  ├─ fragment_stream ─── fixed-size write slicing (MTU emulation)
                │
 corruption     ├─ corrupt_bytes ───── seeded per-byte bit-flips
                │
 connection-    ├─ terminate_connections ─ seeded mid-stream hard close
 kill           │
                │
 lifecycle      ├─ start_proxy ─────── all of the above, composed
                └─ stop_proxy ──────── the rollback for everything
```

`stop_proxy` is idempotent: call it when nothing is running and it shrugs and returns cleanly. That matters, because rollbacks in chaos experiments run in the worst possible moments, and a rollback that can fail on "already stopped" is a rollback you can't trust.

And about that `seed`; it's not decoration. The jitter schedule, the byte-corruption RNG, and the connection-termination RNG are all derived from it. Same seed, same traffic: the *same bytes* get flipped, the *same connections* get killed. Reproducible chaos is debuggable chaos.

## An experiment, end to end

Here's `examples/net-chaos.toon`, straight from the repo. Note the provider: `type: native`, `plugin: tumult-net`. In 2.0.0 this routes through the `NativeExecutorRegistry`; no script plugin, no subprocess shim, just a function call inside the Tumult binary.

```toon
title: TCP latency chaos via native tokio-netem proxy
description: Inject one-way latency through a tumult-net chaos proxy and roll it back

tags[2]: network, resilience

steady_state_hypothesis:
  title: System check
  probes[1]:
    - name: system-alive
      activity_type: probe
      provider:
        type: process
        path: echo
        arguments[1]: "ready"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "ready"

method[1]:
  - name: inject-latency
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: inject_latency
      arguments:
        listen: 127.0.0.1:18080
        upstream: 127.0.0.1:8080
        delay_ms: 200
        jitter_ms: 25
        seed: 42
    pause_after_s: 3.0

rollbacks[1]:
  - name: stop-proxy
    activity_type: action
    provider:
      type: native
      plugin: tumult-net
      function: stop_proxy
      arguments:
        listen: 127.0.0.1:18080
```

Typo the plugin or function name and 2.0.0's registry errors with the list of what actually exists; no silent no-ops.

## The lifecycle, with real recovery numbers

2.0.0 also changed *when* probes run. During-phase hypothesis probes now sample on a real interval (default 1s) concurrently with the fault, and post-phase sampling loops until the probes pass tolerance again; or a recovery timeout expires. So `recovery_time_s` and `mttr_s` in the journal are *observed*, not assumed.

Put those together and a tumult-net experiment looks like this:

```
 start_proxy            steady-state       inject_latency
 (or fault fn)          probes pass        200ms +25ms jitter
      │                     │                    │
      ▼                     ▼                    ▼
 ┌─────────┐         ┌───────────┐        ┌───────────┐
 │  proxy   │────────►│  baseline  │───────►│   FAULT    │
 │  daemon  │         │  ✓ ✓ ✓     │        │   WINDOW   │
 │  spawned │         └───────────┘        │            │
 └─────────┘                               │ probes     │
                                           │ sampled    │
                                           │ every 1s   │
                                           │ ✗ ✗ ✗ ...  │
                                           └─────┬─────┘
                                                 │ fault ends
                                                 ▼
                       ┌────────────────────────────────┐
                       │  recovery sampling              │
                       │  ✗ ✗ ✓ ← first pass = observed  │
                       │          recovery_time_s /      │
                       │          mttr_s in the journal  │
                       └───────────────┬────────────────┘
                                       ▼
                              ┌────────────────┐
                              │  stop_proxy     │  idempotent rollback:
                              │  (rollback)     │  kill daemon, remove
                              └────────────────┘  pidfile, done
```

The crate ships two read-only probes to close the loop from inside the same plugin: `reachable` (does `host:port` accept a connection; refusal is a clean `false`, not an error) and `measured_latency` (TCP handshake time in milliseconds). Point `measured_latency` at the proxy's listen port and your injected 200ms shows up as a number in the journal.

## Which layer do you want to break?

tumult-net doesn't replace the other network chaos plugins; it completes the set. Each one faults a different layer, and each layer has a real job:

```
 layer         tool               needs                 breaks
 ─────────    ────────────────   ───────────────────   ─────────────────────
 kernel        tumult-network     Linux, root/sudo,     everything on the
 (tc netem,    (script plugin)    tc + iptables         interface; every
  iptables)                                             process, every port

 container     tumult-pumba       Docker socket,        one container's
 (netem in     (script plugin)    a target container    egress traffic
  the netns)

 userspace     tumult-net         a TCP socket.         one proxied
 (in-process   (native plugin)    that's it.            connection path,
  proxy)                                                deterministically
```

Want to degrade *the whole host's* network, including DNS and every daemon on it? That's `tumult-network`; kernel-level fidelity is exactly what root buys you. Want container-scoped netem without touching the host? `tumult-pumba`. Want a reproducible, seeded fault on one service dependency, in a CI container, with zero privileges? That's tumult-net.

Right tool per layer. The `.toon` format is the same for all three, so switching layers is a provider swap, not a rewrite.

## Run it

```
$ tumult run examples/net-chaos.toon
Running experiment: TCP latency chaos via native tokio-netem proxy
Status: Completed
Duration: 3247ms
Method steps: 1 executed
Rollbacks: 1 executed
Journal written to: journal.toon
Ingested into persistent analytics store

$ tumult report --format junit --output report.xml
```

That second command is the 1.5.0 pairing that makes this CI-ready: `tumult report --format junit|json` emits one JUnit `<testcase>` per activity. A latency experiment that fails its hypothesis fails the pipeline; same as any other test. And since tumult-net needs no privileges, the whole loop runs inside a stock CI runner.

Under the hood, the fault outlives the CLI call: the action spawns a detached `tumult-net-proxyd` daemon and records its PID in a pidfile keyed by the listen address. `stop_proxy` finds it from a completely fresh invocation and tears it down. Stale pidfile? Cleaned up. No pidfile? "no chaos proxy running", exit zero.

## It shows up in your traces

Like every Tumult crate, tumult-net is instrumented with OpenTelemetry from day one. Every action and probe opens a client span in the `net.*` domain; `net.latency.inject`, `net.bandwidth.throttle`, `net.probe.latency`, and friends; carrying attributes like `net.listen.addr` and `net.latency.delay_ms`, with `net.proxy.started` / `net.proxy.stopped` events recording the daemon PID.

These spans attach under the resilience experiment spans emitted by `tumult-core`, so in your trace UI the injected latency sits *inside* the experiment waterfall, right next to the probe that measured it. The fault and its evidence, one screen.

---

*tumult-net shipped in 1.5.0 and moved to native-registry dispatch in 2.0.0. See the [plugin docs](../plugins/index.md) for the full catalog, and `examples/net-chaos.toon` to try it in the next five minutes.*
