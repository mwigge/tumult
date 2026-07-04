# tumult-timewarp

Clock-skew and entropy/crypto-skew fault injection for Tumult.

A script plugin (no Rust changes) discovered from `plugins/`. It injects the
kinds of faults that time drift and RNG starvation cause in real systems:
expired certs/tokens, failed TLS handshakes, and degraded crypto throughput.

Runs from any Tumult runner (in the demo, the `tumult-mcp` container). Targets
can be the runner itself (helper processes) or sibling containers via
`docker exec` (`TUMULT_TARGET`).

## Why this is honest about mechanism

Containers in docker-compose **share the host kernel clock**. Two things follow
that shape every action here:

1. Linux **time namespaces virtualize only `CLOCK_MONOTONIC` / `CLOCK_BOOTTIME`,
   not `CLOCK_REALTIME`** (wall clock). You cannot give a container its own wall
   clock with namespaces.
2. `date -s` needs `CAP_SYS_TIME` and moves the **host** clock — unsafe, usually
   denied, and it would skew every container at once.

So the realistic per-target wall-clock lever is **libfaketime** (`LD_PRELOAD`),
which shifts the perceived time of the single process it wraps. Everything else
in this plugin proves clock-driven *consequences* (cert/token expiry, crypto
slowdown) without touching any system clock — which is exactly what you want in
a shared-kernel environment.

## Actions

| Action | Mechanism | Honest limit |
|---|---|---|
| `skew-clock` | `faketime "+Ns" <cmd>` — per-process wall-clock offset via libfaketime | Affects only the wrapped process; requires `libfaketime` present (on the runner, or in `TUMULT_TARGET`). Not installed in the stock demo image — the action detects this and exits 1 with guidance. |
| `advance-clock-past-cert-expiry` | Mints a short-lived self-signed cert, then `openssl verify -attime <now>` (valid) vs `-attime <now+skew>` (expired) | Models skew by feeding the verifier a future time — the exact input a drifted clock would give. Requires `openssl`. Writes `cert-result.txt` (`EXPIRED_UNDER_SKEW`). |
| `token-ttl` | Mints an HMAC (`sha256sum`) bearer token with a short TTL, validates at real now (accept) vs skewed now (reject) | Pure coreutils — most portable. Writes `token-result.txt` (`REJECTED_UNDER_SKEW`). |
| `entropy-drain` | N background `dd if=/dev/random` reader workers (setsid process groups, `timeout`-bounded) | On kernels ≥ 5.6 the CRNG never blocks, so `entropy_avail` stays ~constant (~256): this is RNG **read/CPU pressure**, not true pool depletion. Genuinely drains only on older kernels. |
| `rng-pressure` | N background workers generating random bytes in a loop (`openssl rand`, else `/dev/urandom`) | Contention-driven crypto slowdown, not entropy depletion. Measure the effect with `crypto-throughput`. |
| `stop-entropy-drain` | Rollback: `kill -TERM -<pgid>` on each recorded worker group | Kills the whole worker tree (timeout + reader) as a unit; no orphans. Idempotent. |
| `restore-clock` | Rollback: removes temp cert/token state | Nothing else to undo — faketime is per-process and no system clock was changed. Idempotent. |

## Probes

| Probe | Output | Notes |
|---|---|---|
| `entropy-available` | integer, `/proc/sys/kernel/random/entropy_avail` | ~256 constant on modern kernels; the correct signal on old ones. |
| `crypto-throughput` | integer ms to generate `TUMULT_CRYPTO_BYTES` of randomness | Rises under `rng-pressure` / CPU contention. |
| `clock-offset` | integer seconds `target_epoch − runner_epoch` | 0 when clocks agree (shared kernel); non-zero only when a target process is faketime-skewed. |

## Deliberately skipped

- **`docker exec <target> date -s <time>` (host-clock skew).** Rejected: it
  needs `CAP_SYS_TIME`, is denied for unprivileged demo containers, and when it
  *does* work it moves the shared host clock — skewing SigNoz, the collector,
  the runner and every sibling at once. That is not a targeted fault; it is a
  self-inflicted outage of the whole stack. `skew-clock` (faketime) is the
  honest, contained substitute.
- **Time-namespace wall-clock skew (`unshare --time`).** Rejected: time
  namespaces do not virtualize `CLOCK_REALTIME`, so they cannot skew wall clock
  at all — only monotonic/boottime, which cert/token/TLS logic does not use.

## Environment variables

Shared: `TUMULT_TW_STATE_DIR` (default `/tmp/tumult-timewarp`), `TUMULT_TARGET`
(optional container for `docker exec`).

- `skew-clock`: `TUMULT_SKEW_SECONDS` (default 3600, may be negative),
  `TUMULT_FAKETIME_CMD` (default `date -u +%s`).
- `advance-clock-past-cert-expiry`: `TUMULT_CERT_TTL_SECONDS` (default 5),
  `TUMULT_SKEW_SECONDS` (default 8640000 = 100d).
- `token-ttl`: `TUMULT_TOKEN_TTL_SECONDS` (default 2), `TUMULT_SKEW_SECONDS`
  (default 3600), `TUMULT_TOKEN_SECRET`.
- `entropy-drain`: `TUMULT_DRAIN_WORKERS` (default 4), `TUMULT_DRAIN_DURATION`
  (default 60).
- `rng-pressure`: `TUMULT_RNG_WORKERS` (default 4), `TUMULT_RNG_DURATION`
  (default 60).
- `crypto-throughput`: `TUMULT_CRYPTO_BYTES` (default 33554432 = 32 MiB).

## Demo experiments

- `demo/experiments/demo-timewarp-clock.toon` — clock skew rejects a
  once-valid token (`token-ttl`), demo-app stays healthy, rollback cleans state.
- `demo/experiments/demo-timewarp-entropy.toon` — sustained `rng-pressure`;
  crypto still completes and entropy stays readable (resilience), rollback stops
  the load.

Both pass `tumult validate` and are designed to run to a clean `Completed`.
