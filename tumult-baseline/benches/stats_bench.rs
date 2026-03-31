//! Benchmarks for tumult-baseline statistical functions.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use tumult_baseline::tolerance::Method;
use tumult_baseline::{derive_baseline, mean, percentile, stddev, AcquisitionConfig, ProbeSamples};

fn make_samples(n: usize) -> Vec<f64> {
    // Cast from usize to f64: bench data only; precision loss on large n is acceptable.
    #[allow(clippy::cast_precision_loss)]
    (0..n).map(|i| 50.0 + (i % 100) as f64 * 0.5).collect()
}

fn make_probe_samples(name: &str, n: usize) -> ProbeSamples {
    ProbeSamples {
        name: name.into(),
        values: make_samples(n),
        errors: 0,
        total_attempts: u32::try_from(n).unwrap_or(u32::MAX),
        sampled_at: vec![],
    }
}

fn bench_percentile_small(c: &mut Criterion) {
    let data = make_samples(100);
    c.bench_function("percentile_p95_100_elements", |b| {
        b.iter(|| percentile(black_box(&data), black_box(95.0)));
    });
}

fn bench_percentile_large(c: &mut Criterion) {
    let data = make_samples(10_000);
    c.bench_function("percentile_p95_10k_elements", |b| {
        b.iter(|| percentile(black_box(&data), black_box(95.0)));
    });
}

fn bench_mean_stddev_large(c: &mut Criterion) {
    let data = make_samples(10_000);
    c.bench_function("mean_10k_elements", |b| {
        b.iter(|| mean(black_box(&data)));
    });
    c.bench_function("stddev_10k_elements", |b| {
        b.iter(|| stddev(black_box(&data)));
    });
}

fn bench_derive_baseline_full(c: &mut Criterion) {
    let probe_names = ["probe-a", "probe-b", "probe-c", "probe-d", "probe-e"];
    let samples: Vec<ProbeSamples> = probe_names
        .iter()
        .map(|name| make_probe_samples(name, 100))
        .collect();
    let config = AcquisitionConfig {
        method: Method::MeanStddev { sigma: 2.0 },
        min_samples: 5,
    };

    c.bench_function("derive_baseline_5probes_100samples", |b| {
        b.iter(|| derive_baseline(black_box(&samples), black_box(&config)));
    });
}

criterion_group!(
    benches,
    bench_percentile_small,
    bench_percentile_large,
    bench_mean_stddev_large,
    bench_derive_baseline_full
);
criterion_main!(benches);
