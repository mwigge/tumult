# tumult-core

Core engine for the Tumult chaos engineering platform -- experiment parsing, execution, journaling, and hypothesis evaluation.

## Key Types

- `Experiment` -- parsed experiment definition
- `Runner` -- orchestrates experiment execution
- `Journal` -- records experiment results in TOON format
- `SteadyStateHypothesis` -- tolerance evaluation

## Usage

```rust
use tumult_core::{Experiment, Runner};

let experiment = Experiment::from_file("experiment.toon")?;
let journal = Runner::new().run(experiment).await?;
```

## More Information

See the [main README](../README.md) for project overview and setup.
