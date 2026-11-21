# tumult-core

The engine of Tumult: it parses an experiment, runs it, judges the steady
state and writes the journal.

## Key types

| Item | What it does |
|---|---|
| `Experiment` | A parsed experiment definition |
| `Hypothesis` | The steady-state probes and their tolerances |
| `run_experiment` | Runs an experiment and returns its journal |
| `Journal` | The record of a run, written as TOON |

## Use it from Rust

```rust
use std::sync::Arc;
use tumult_core::controls::ControlRegistry;
use tumult_core::engine::parse_experiment;
use tumult_core::runner::{run_experiment, ActivityExecutor, RunConfig};
use tumult_exec::ProviderExecutor;

let experiment = parse_experiment(&std::fs::read_to_string("experiment.toon")?)?;
let executor: Arc<dyn ActivityExecutor> = Arc::new(ProviderExecutor::new());
let controls = Arc::new(ControlRegistry::new());
let journal = run_experiment(&experiment, &executor, &controls, &RunConfig::default())?;
```

The order of a run is in [Execution flow](../docs/guides/execution-flow.md).
