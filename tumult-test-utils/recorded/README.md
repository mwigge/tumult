# Recorded runs

What the `tumult` binary sent and wrote on five runs of a small lab.
Tests load it into a store and build the reports from it.

| File | Holds |
|---|---|
| `experiments/*.toon` | The four experiments that ran |
| `journals/NN-<experiment>.journal.toon` | The journal of each run, in run order |
| `traces.jsonl`, `metrics.jsonl`, `logs.jsonl` | Each OTLP request the binary sent, one a line |

## The runs

| Run | Experiment | Ends | Telemetry |
|---|---|---|---|
| 01 | checkout-cache-restart | completed | yes |
| 02 | payments-slow-ledger | deviated | yes |
| 03 | search-node-loss | deviated | yes |
| 04 | search-node-loss, after the fix | completed | yes |
| 05 | billing-export-restart | completed | no |

## Use it

```rust
let journals = tumult_test_utils::recorded::load(&writer);
```

The loader takes the same path as the daemon: the OTLP requests go
through `tumult-otlp` into the store, the journals through the ingest.

## Record it again

```text
cargo build -p tumult-cli
cargo run -p tumult-test-utils --features recorded --example record_telemetry -- target/debug/tumult
```

A new recording has new ids and new times. Run the report tests with
`TUMULT_BLESS=1`, read the difference, then commit both.
