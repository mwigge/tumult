# tumult-core

Core engine for the Tumult resilience intelligence platform -- experiment parsing, execution, journaling, and hypothesis evaluation.

## Key Types

- `Experiment` -- parsed experiment definition
- `run_experiment` -- orchestrates experiment execution
- `Journal` -- records experiment results in TOON format
- `Hypothesis` -- the steady-state probes and their tolerances

## Usage

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

## More Information

See the [main README](../README.md) for project overview and setup.
