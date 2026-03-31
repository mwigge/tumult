# tumult-baseline

Baseline acquisition and statistical methods for Tumult experiments -- capture steady-state metrics and derive tolerance bounds.

## Key Types

- `BaselineBounds` -- upper and lower tolerance bounds
- `mean`, `stddev`, `percentile` -- core statistical functions
- `derive_mean_stddev_bounds`, `derive_iqr_bounds` -- tolerance derivation

## Usage

```rust
use tumult_baseline::stats::{mean, derive_mean_stddev_bounds};

let data = vec![100.0, 102.0, 98.0, 101.0, 99.0];
let bounds = derive_mean_stddev_bounds(&data, 2.0);
assert!(bounds.contains(mean(&data)));
```

## More Information

See the [main README](../README.md) for project overview and setup.
