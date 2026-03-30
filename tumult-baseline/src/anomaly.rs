//! Baseline anomaly detection.
//!
//! Detects if a baseline measurement itself is anomalous (degraded before
//! the experiment even starts). Uses coefficient of variation and outlier detection.

use crate::stats::{mean, percentile, stddev};

/// Result of an anomaly check on baseline data.
#[derive(Debug, Clone)]
pub struct AnomalyCheck {
    pub anomaly_detected: bool,
    pub reason: Option<String>,
    pub coefficient_of_variation: f64,
}

/// Check if a baseline dataset shows anomalous behavior.
///
/// An anomaly is detected when:
/// 1. Coefficient of variation exceeds the threshold (default 0.5 = 50%)
/// 2. The range (max-min) exceeds 10x the median
/// 3. Too few samples were collected
pub fn check_baseline_anomaly(data: &[f64], min_samples: usize) -> AnomalyCheck {
    if data.len() < min_samples {
        return AnomalyCheck {
            anomaly_detected: true,
            reason: Some(format!(
                "insufficient samples: {} < {}",
                data.len(),
                min_samples
            )),
            coefficient_of_variation: 0.0,
        };
    }

    let m = mean(data);
    let sd = stddev(data);
    let cv = if m.abs() > f64::EPSILON { sd / m } else { 0.0 };

    if cv > 0.5 {
        return AnomalyCheck {
            anomaly_detected: true,
            reason: Some(format!(
                "high variance: coefficient of variation {cv:.2} exceeds 0.50"
            )),
            coefficient_of_variation: cv,
        };
    }

    let min_val = percentile(data, 0.0);
    let max_val = percentile(data, 100.0);
    let median = percentile(data, 50.0);
    let range = max_val - min_val;

    if median > f64::EPSILON && range > 10.0 * median {
        return AnomalyCheck {
            anomaly_detected: true,
            reason: Some(format!(
                "extreme range: {range:.2} exceeds 10x median {median:.2}"
            )),
            coefficient_of_variation: cv,
        };
    }

    AnomalyCheck {
        anomaly_detected: false,
        reason: None,
        coefficient_of_variation: cv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_data_is_not_anomalous() {
        let data = vec![100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0, 97.0];
        let result = check_baseline_anomaly(&data, 5);
        assert!(!result.anomaly_detected);
        assert!(result.reason.is_none());
    }

    #[test]
    fn high_variance_is_anomalous() {
        let data = vec![1.0, 100.0, 2.0, 99.0, 3.0, 98.0, 1.0, 200.0];
        let result = check_baseline_anomaly(&data, 5);
        assert!(result.anomaly_detected);
        assert!(result.reason.unwrap().contains("coefficient of variation"));
    }

    #[test]
    fn too_few_samples_is_anomalous() {
        let data = vec![100.0, 101.0];
        let result = check_baseline_anomaly(&data, 5);
        assert!(result.anomaly_detected);
        assert!(result.reason.unwrap().contains("insufficient samples"));
    }

    #[test]
    fn constant_values_are_not_anomalous() {
        let data = vec![50.0; 10];
        let result = check_baseline_anomaly(&data, 5);
        assert!(!result.anomaly_detected);
        assert_eq!(result.coefficient_of_variation, 0.0);
    }

    #[test]
    fn extreme_range_is_anomalous() {
        // Median ~10, range 10000 → 10000 > 10 * 10
        let mut data = vec![10.0; 20];
        data.push(10000.0);
        let result = check_baseline_anomaly(&data, 5);
        assert!(result.anomaly_detected);
    }

    #[test]
    fn empty_data_with_min_samples_is_anomalous() {
        let result = check_baseline_anomaly(&[], 1);
        assert!(result.anomaly_detected);
    }

    #[test]
    fn exact_min_samples_is_not_anomalous() {
        let data = vec![100.0, 101.0, 99.0, 100.0, 102.0];
        let result = check_baseline_anomaly(&data, 5);
        assert!(!result.anomaly_detected);
    }
}
