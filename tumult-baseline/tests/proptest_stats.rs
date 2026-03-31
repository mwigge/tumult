//! Property-based tests for statistical functions in `tumult-baseline`.
//!
//! These tests use `proptest` to verify mathematical invariants that must hold
//! for any well-formed input, independent of specific values.

use proptest::prelude::*;
use tumult_baseline::stats::{derive_iqr_bounds, mean, percentile, stddev};

// ── Strategy helpers ─────────────────────────────────────────

/// A strategy for generating non-empty `Vec<f64>` with finite values.
fn finite_f64_vec() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(
        prop::num::f64::NORMAL | prop::num::f64::POSITIVE | prop::num::f64::NEGATIVE,
        1..=200,
    )
}

/// A strategy for a single finite (non-NaN, non-infinite) `f64`.
fn finite_f64() -> impl Strategy<Value = f64> {
    prop::num::f64::NORMAL
}

/// A strategy for a non-empty constant slice (all elements equal to `x`).
///
/// The value is clamped to `[-1e100, 1e100]` so that summing up to 50
/// identical copies cannot overflow to infinity inside `mean`.
fn constant_slice() -> impl Strategy<Value = (f64, Vec<f64>)> {
    (finite_f64(), 1usize..=50)
        .prop_map(|(x, n)| {
            // Clamp x so that x * n cannot overflow f64.
            let x = x.clamp(-1e100, 1e100);
            let vec = vec![x; n];
            (x, vec)
        })
        .prop_filter("x must be finite after clamp", |(x, _)| x.is_finite())
}

// ═══════════════════════════════════════════════════════════════
// prop_percentile_always_between_min_and_max
// ═══════════════════════════════════════════════════════════════

proptest! {
    /// For any non-empty vec of finite f64, `percentile(v, p)` is between the
    /// minimum and maximum element of `v`.
    #[test]
    fn prop_percentile_always_between_min_and_max(
        data in finite_f64_vec(),
        p in 0.0f64..=100.0f64,
    ) {
        // Filter out vectors with NaN/infinity (shouldn't happen with NORMAL
        // strategy, but be defensive).
        prop_assume!(data.iter().all(|x| x.is_finite()));

        let min = data.iter().copied().fold(f64::INFINITY, f64::min);
        let max = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);

        let result = percentile(&data, p);

        prop_assert!(
            result.is_finite(),
            "percentile must be finite; got {result} for p={p}"
        );
        prop_assert!(
            result >= min - f64::EPSILON * min.abs().max(1.0),
            "percentile {result} must be >= min {min}"
        );
        prop_assert!(
            result <= max + f64::EPSILON * max.abs().max(1.0),
            "percentile {result} must be <= max {max}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// prop_mean_of_constant_slice_equals_constant
// ═══════════════════════════════════════════════════════════════

proptest! {
    /// mean([x; n]) == x for any finite x and n >= 1.
    #[test]
    fn prop_mean_of_constant_slice_equals_constant(
        (x, slice) in constant_slice(),
    ) {
        prop_assume!(x.is_finite());

        let m = mean(&slice);

        prop_assert!(
            (m - x).abs() <= x.abs() * 1e-10 + 1e-10,
            "mean of constant slice must equal the constant; \
             expected ≈{x}, got {m}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// prop_stddev_non_negative
// ═══════════════════════════════════════════════════════════════

proptest! {
    /// Population standard deviation is always >= 0 for any non-empty finite vec.
    #[test]
    fn prop_stddev_non_negative(data in finite_f64_vec()) {
        prop_assume!(data.iter().all(|x| x.is_finite()));

        let sd = stddev(&data);

        prop_assert!(
            sd >= 0.0,
            "stddev must be non-negative; got {sd}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// prop_iqr_upper_ge_lower
// ═══════════════════════════════════════════════════════════════

proptest! {
    /// For any non-empty finite vec, the IQR upper bound >= lower bound.
    #[test]
    fn prop_iqr_upper_ge_lower(data in finite_f64_vec()) {
        prop_assume!(data.iter().all(|x| x.is_finite()));

        let bounds = derive_iqr_bounds(&data);

        prop_assert!(
            bounds.upper >= bounds.lower,
            "IQR upper ({}) must be >= lower ({})",
            bounds.upper, bounds.lower
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// prop_percentile_monotone
// ═══════════════════════════════════════════════════════════════

proptest! {
    /// Percentile is monotone: p1 <= p2 implies percentile(v, p1) <= percentile(v, p2).
    #[test]
    fn prop_percentile_monotone(
        data in finite_f64_vec(),
        p1 in 0.0f64..=100.0f64,
        p2 in 0.0f64..=100.0f64,
    ) {
        prop_assume!(data.iter().all(|x| x.is_finite()));

        let (lo, hi) = if p1 <= p2 { (p1, p2) } else { (p2, p1) };

        let r_lo = percentile(&data, lo);
        let r_hi = percentile(&data, hi);

        prop_assert!(
            r_lo <= r_hi + f64::EPSILON * r_hi.abs().max(1.0),
            "percentile({lo}) = {r_lo} must be <= percentile({hi}) = {r_hi}"
        );
    }
}
