//! `UNIVARIATE_METRICS` (+ data point and attribute tables) →
//! `ExportMetricsServiceRequest`.
//!
//! Number (gauge/sum) and histogram data points are decoded in full.
//! Summary and exponential-histogram points are counted but left empty: the
//! OTLP conversion rejects those kinds anyway, and this keeps the rejected
//! count identical across carriers. Exemplars are dropped (no lake column).
//! `MULTIVARIATE_METRICS` batches are refused.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point, ExponentialHistogram, ExponentialHistogramDataPoint, Gauge,
    Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
    Summary, SummaryDataPoint,
};

use super::arrays::Table;
use super::attrs::{attributes, AttrMap};
use super::common::{groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

/// Codes of the `metric_type` column.
mod metric_type {
    pub const GAUGE: u8 = 1;
    pub const SUM: u8 = 2;
    pub const HISTOGRAM: u8 = 3;
    pub const EXP_HISTOGRAM: u8 = 4;
    pub const SUMMARY: u8 = 5;
}

/// Decode one OTAP metrics batch into the OTLP request the gRPC/HTTP paths
/// receive.
///
/// # Errors
/// Any missing root table, malformed column, unknown metric type, or a
/// multivariate batch.
pub fn to_metrics_request(batch: &OtapBatch) -> Result<ExportMetricsServiceRequest, OtapError> {
    if batch.get(ArrowPayloadType::MultivariateMetrics).is_some() {
        return Err(OtapError::Unsupported("MULTIVARIATE_METRICS"));
    }
    let metrics_rb = batch
        .get(ArrowPayloadType::UnivariateMetrics)
        .ok_or(OtapError::MissingTable("UNIVARIATE_METRICS"))?;
    let root = Root::new("UNIVARIATE_METRICS", metrics_rb, batch)?;
    let mut points = DataPoints::decode(batch)?;

    let mut out = ExportMetricsServiceRequest::default();
    for (i, new_resource, new_scope) in groups(&root) {
        if new_resource {
            out.resource_metrics.push(ResourceMetrics {
                resource: root.resource(i)?,
                scope_metrics: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let Some(rm) = out.resource_metrics.last_mut() else {
            continue;
        };
        if new_scope {
            rm.scope_metrics.push(ScopeMetrics {
                scope: root.scope(i)?,
                metrics: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let metric = metric_at(&root, i, &mut points)?;
        if let Some(sm) = rm.scope_metrics.last_mut() {
            sm.metrics.push(metric);
        }
    }
    Ok(out)
}

fn metric_at(root: &Root, i: usize, points: &mut DataPoints) -> Result<Metric, OtapError> {
    let t = &root.table;
    let id = root.ids[i];
    let kind = t.u8_at("metric_type", i)?.unwrap_or(0);
    let data = match kind {
        metric_type::GAUGE => Some(Data::Gauge(Gauge {
            data_points: points.numbers(id),
        })),
        metric_type::SUM => Some(Data::Sum(Sum {
            data_points: points.numbers(id),
            aggregation_temporality: t.i32_at("aggregation_temporality", i)?.unwrap_or(0),
            is_monotonic: t.bool_at("is_monotonic", i)?.unwrap_or(false),
        })),
        metric_type::HISTOGRAM => Some(Data::Histogram(Histogram {
            data_points: points.histograms(id),
            aggregation_temporality: t.i32_at("aggregation_temporality", i)?.unwrap_or(0),
        })),
        metric_type::EXP_HISTOGRAM => Some(Data::ExponentialHistogram(ExponentialHistogram {
            data_points: vec![
                ExponentialHistogramDataPoint::default();
                points.count(&points.exp_histograms, id)
            ],
            aggregation_temporality: t.i32_at("aggregation_temporality", i)?.unwrap_or(0),
        })),
        metric_type::SUMMARY => Some(Data::Summary(Summary {
            data_points: vec![SummaryDataPoint::default(); points.count(&points.summaries, id)],
        })),
        0 => None,
        other => {
            return Err(OtapError::Column {
                table: "UNIVARIATE_METRICS",
                column: "metric_type".into(),
                reason: format!("row {i}: unknown metric type {other}"),
            })
        }
    };
    Ok(Metric {
        name: t.string_at("name", i)?,
        description: t.string_at("description", i)?,
        unit: t.string_at("unit", i)?,
        data,
        ..Metric::default()
    })
}

/// Data point tables keyed by metric id.
#[derive(Default)]
struct DataPoints {
    numbers: HashMap<u32, Vec<NumberDataPoint>>,
    histograms: HashMap<u32, Vec<HistogramDataPoint>>,
    summaries: HashMap<u32, usize>,
    exp_histograms: HashMap<u32, usize>,
}

impl DataPoints {
    fn decode(batch: &OtapBatch) -> Result<Self, OtapError> {
        let mut out = Self::default();
        if let Some(rb) = batch.get(ArrowPayloadType::NumberDataPoints) {
            let t = Table::new("NUMBER_DATA_POINTS", rb)?;
            let attrs = attributes(
                batch.get(ArrowPayloadType::NumberDpAttrs),
                "NUMBER_DP_ATTRS",
            )?;
            let ids = t.ids("id")?;
            for (i, parent) in t.ids("parent_id")?.into_iter().enumerate() {
                let Some(parent) = parent else { continue };
                let value = match (t.i64_at("int_value", i)?, t.f64_at("double_value", i)?) {
                    (Some(v), _) => Some(number_data_point::Value::AsInt(v)),
                    (None, Some(v)) => Some(number_data_point::Value::AsDouble(v)),
                    (None, None) => None,
                };
                out.numbers
                    .entry(parent)
                    .or_default()
                    .push(NumberDataPoint {
                        attributes: attrs_of(&attrs, ids[i]),
                        start_time_unix_nano: t.timestamp_at("start_time_unix_nano", i)?,
                        time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
                        exemplars: Vec::new(),
                        flags: t.u32_at("flags", i)?.unwrap_or(0),
                        value,
                    });
            }
        }
        if let Some(rb) = batch.get(ArrowPayloadType::HistogramDataPoints) {
            let t = Table::new("HISTOGRAM_DATA_POINTS", rb)?;
            let attrs = attributes(
                batch.get(ArrowPayloadType::HistogramDpAttrs),
                "HISTOGRAM_DP_ATTRS",
            )?;
            let ids = t.ids("id")?;
            for (i, parent) in t.ids("parent_id")?.into_iter().enumerate() {
                let Some(parent) = parent else { continue };
                out.histograms
                    .entry(parent)
                    .or_default()
                    .push(HistogramDataPoint {
                        attributes: attrs_of(&attrs, ids[i]),
                        start_time_unix_nano: t.timestamp_at("start_time_unix_nano", i)?,
                        time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
                        count: t.u64_at("count", i)?.unwrap_or(0),
                        sum: t.f64_at("sum", i)?,
                        bucket_counts: t.u64_list_at("bucket_counts", i)?,
                        explicit_bounds: t.f64_list_at("explicit_bounds", i)?,
                        exemplars: Vec::new(),
                        flags: t.u32_at("flags", i)?.unwrap_or(0),
                        min: t.f64_at("min", i)?,
                        max: t.f64_at("max", i)?,
                    });
            }
        }
        out.summaries = count_by_parent(
            batch,
            ArrowPayloadType::SummaryDataPoints,
            "SUMMARY_DATA_POINTS",
        )?;
        out.exp_histograms = count_by_parent(
            batch,
            ArrowPayloadType::ExpHistogramDataPoints,
            "EXP_HISTOGRAM_DATA_POINTS",
        )?;
        Ok(out)
    }

    fn numbers(&mut self, id: Option<u32>) -> Vec<NumberDataPoint> {
        id.and_then(|id| self.numbers.remove(&id))
            .unwrap_or_default()
    }

    fn histograms(&mut self, id: Option<u32>) -> Vec<HistogramDataPoint> {
        id.and_then(|id| self.histograms.remove(&id))
            .unwrap_or_default()
    }

    fn count(&self, counts: &HashMap<u32, usize>, id: Option<u32>) -> usize {
        id.and_then(|id| counts.get(&id).copied()).unwrap_or(0)
    }
}

fn attrs_of(
    attrs: &AttrMap,
    id: Option<u32>,
) -> Vec<opentelemetry_proto::tonic::common::v1::KeyValue> {
    id.and_then(|id| attrs.get(&id).cloned())
        .unwrap_or_default()
}

fn count_by_parent(
    batch: &OtapBatch,
    payload: ArrowPayloadType,
    name: &'static str,
) -> Result<HashMap<u32, usize>, OtapError> {
    let mut counts = HashMap::new();
    if let Some(rb) = batch.get(payload) {
        let t = Table::new(name, rb)?;
        for parent in t.ids("parent_id")?.into_iter().flatten() {
            *counts.entry(parent).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
