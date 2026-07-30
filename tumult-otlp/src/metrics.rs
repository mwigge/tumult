//! `ExportMetricsServiceRequest` → metric row batches.

use tumult_lake::{MetricGaugeRow, MetricHistogramRow, MetricSumRow};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};

use crate::common::{self, keys, ResourceCtx};

/// Promoted low-cardinality metric dimensions (tumult metadata standard §6:
/// only bounded `resilience.*` keys are metric-safe). Covers both the
/// standard's names and the ones tumult's instruments actually emit.
const PROMOTED_METRIC_KEYS: &[&str] = &[
    keys::EXPERIMENT_NAME,
    keys::OUTCOME_STATUS,
    keys::FAULT_PLUGIN,
    keys::PLUGIN_NAME,
];

/// The three metric row batches produced from one export request.
#[derive(Debug, Default)]
pub struct MetricRows {
    pub sums: Vec<MetricSumRow>,
    pub gauges: Vec<MetricGaugeRow>,
    pub histograms: Vec<MetricHistogramRow>,
}

struct PromotedDims {
    experiment_name: Option<String>,
    outcome_status: Option<String>,
    plugin_name: Option<String>,
    attrs: Vec<(String, String)>,
}

fn promoted_dims(attrs: &[KeyValue]) -> PromotedDims {
    PromotedDims {
        experiment_name: common::attr_string(attrs, keys::EXPERIMENT_NAME),
        outcome_status: common::attr_string(attrs, keys::OUTCOME_STATUS),
        plugin_name: common::attr_string(attrs, keys::FAULT_PLUGIN)
            .or_else(|| common::attr_string(attrs, keys::PLUGIN_NAME)),
        attrs: common::unpromoted_attrs(attrs, PROMOTED_METRIC_KEYS),
    }
}

fn number_value(value: Option<&number_data_point::Value>) -> f64 {
    match value {
        Some(number_data_point::Value::AsDouble(d)) => *d,
        Some(number_data_point::Value::AsInt(i)) => *i as f64,
        None => 0.0,
    }
}

fn ts(ts_unix_nano: u64) -> i64 {
    i64::try_from(ts_unix_nano).unwrap_or(i64::MAX)
}

/// Convert an OTLP metrics export request into store rows (pure).
///
/// Sums and gauges go to their own tables; histograms keep their buckets.
/// Exponential histograms and summaries are not yet stored
/// // TODO(otel): map exponential-histogram + summary data points.
#[must_use]
pub fn metrics_request_to_rows(request: &ExportMetricsServiceRequest) -> MetricRows {
    let mut out = MetricRows::default();
    for resource_metrics in &request.resource_metrics {
        let resource = ResourceCtx::from_resource(resource_metrics.resource.as_ref());
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                match metric.data.as_ref() {
                    Some(metric::Data::Sum(sum)) => {
                        for dp in &sum.data_points {
                            let dims = promoted_dims(&dp.attributes);
                            out.sums.push(MetricSumRow {
                                ts_ns: ts(dp.time_unix_nano),
                                metric_name: metric.name.clone(),
                                value: number_value(dp.value.as_ref()),
                                experiment_name: dims.experiment_name,
                                outcome_status: dims.outcome_status,
                                plugin_name: dims.plugin_name,
                                attrs: dims.attrs,
                                resource_attrs: resource.resource_attrs.clone(),
                            });
                        }
                    }
                    Some(metric::Data::Gauge(gauge)) => {
                        for dp in &gauge.data_points {
                            let dims = promoted_dims(&dp.attributes);
                            out.gauges.push(MetricGaugeRow {
                                ts_ns: ts(dp.time_unix_nano),
                                metric_name: metric.name.clone(),
                                value: number_value(dp.value.as_ref()),
                                experiment_name: dims.experiment_name,
                                outcome_status: dims.outcome_status,
                                plugin_name: dims.plugin_name,
                                attrs: dims.attrs,
                                resource_attrs: resource.resource_attrs.clone(),
                            });
                        }
                    }
                    Some(metric::Data::Histogram(histogram)) => {
                        for dp in &histogram.data_points {
                            let dims = promoted_dims(&dp.attributes);
                            out.histograms.push(MetricHistogramRow {
                                ts_ns: ts(dp.time_unix_nano),
                                metric_name: metric.name.clone(),
                                count: dp.count,
                                sum: dp.sum.unwrap_or(0.0),
                                min: dp.min,
                                max: dp.max,
                                bucket_counts: dp
                                    .bucket_counts
                                    .iter()
                                    .map(|c| i64::try_from(*c).unwrap_or(i64::MAX))
                                    .collect(),
                                explicit_bounds: dp.explicit_bounds.clone(),
                                experiment_name: dims.experiment_name,
                                outcome_status: dims.outcome_status,
                                plugin_name: dims.plugin_name,
                                attrs: dims.attrs,
                                resource_attrs: resource.resource_attrs.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::AnyValue;
    use opentelemetry_proto::tonic::metrics::v1::{
        Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics,
        ScopeMetrics, Sum,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;

    fn kv(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            key_strindex: 0,
        }
    }

    fn request_with(metrics: Vec<Metric>) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![kv("service.name", Value::StringValue("tumult".into()))],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    #[test]
    fn sum_data_points_promote_dims() {
        let request = request_with(vec![Metric {
            name: "tumult.experiments.total".into(),
            description: String::new(),
            unit: "{experiment}".into(),
            data: Some(metric::Data::Sum(Sum {
                data_points: vec![NumberDataPoint {
                    attributes: vec![
                        kv(keys::OUTCOME_STATUS, Value::StringValue("completed".into())),
                        kv(
                            "resilience.target.system",
                            Value::StringValue("database".into()),
                        ),
                    ],
                    time_unix_nano: 1_774_980_000_000_000_000,
                    value: Some(number_data_point::Value::AsInt(3)),
                    ..NumberDataPoint::default()
                }],
                aggregation_temporality: 2,
                is_monotonic: true,
            })),
            metadata: vec![],
        }]);

        let rows = metrics_request_to_rows(&request);
        assert_eq!(rows.sums.len(), 1);
        let row = &rows.sums[0];
        assert_eq!(row.metric_name, "tumult.experiments.total");
        assert!((row.value - 3.0).abs() < f64::EPSILON);
        assert_eq!(row.outcome_status.as_deref(), Some("completed"));
        assert_eq!(
            row.attrs,
            vec![(
                "resilience.target.system".to_string(),
                "database".to_string()
            )]
        );
    }

    #[test]
    fn histogram_data_points_keep_buckets() {
        let request = request_with(vec![Metric {
            name: "tumult.experiment.duration".into(),
            description: String::new(),
            unit: "s".into(),
            data: Some(metric::Data::Histogram(Histogram {
                data_points: vec![HistogramDataPoint {
                    attributes: vec![],
                    start_time_unix_nano: 0,
                    time_unix_nano: 5,
                    count: 7,
                    sum: Some(42.5),
                    bucket_counts: vec![1, 2, 4],
                    explicit_bounds: vec![5.0, 10.0],
                    min: Some(1.0),
                    max: Some(9.0),
                    ..HistogramDataPoint::default()
                }],
                aggregation_temporality: 2,
            })),
            metadata: vec![],
        }]);

        let rows = metrics_request_to_rows(&request);
        assert_eq!(rows.histograms.len(), 1);
        let row = &rows.histograms[0];
        assert_eq!(row.count, 7);
        assert!((row.sum - 42.5).abs() < f64::EPSILON);
        assert_eq!(row.bucket_counts, vec![1, 2, 4]);
        assert_eq!(row.explicit_bounds, vec![5.0, 10.0]);
        assert_eq!(row.min, Some(1.0));
    }

    #[test]
    fn gauge_data_points() {
        let request = request_with(vec![Metric {
            name: "tumult.active_experiments".into(),
            description: String::new(),
            unit: String::new(),
            data: Some(metric::Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![],
                    time_unix_nano: 9,
                    value: Some(number_data_point::Value::AsDouble(1.0)),
                    ..NumberDataPoint::default()
                }],
            })),
            metadata: vec![],
        }]);
        let rows = metrics_request_to_rows(&request);
        assert_eq!(rows.gauges.len(), 1);
        assert_eq!(rows.gauges[0].ts_ns, 9);
    }
}
