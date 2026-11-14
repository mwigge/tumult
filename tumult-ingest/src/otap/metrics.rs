//! `UNIVARIATE_METRICS` (+ data point and attribute tables) →
//! `ExportMetricsServiceRequest`.
//!
//! Number (gauge/sum) and histogram data points are decoded in full.
//! Summary and exponential-histogram points are counted but left empty: the
//! OTLP conversion rejects those kinds anyway, and this keeps the rejected
//! count identical across carriers. Exemplars are dropped (no lake column).
//! `MULTIVARIATE_METRICS` batches are refused, and so are data points whose
//! metric row is not in the batch or is of another kind.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point, ExponentialHistogram, ExponentialHistogramDataPoint, Gauge,
    Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
    Summary, SummaryDataPoint,
};

use super::arrays::{orphans, Table};
use super::attrs::Attrs;
use super::common::{Groups, Root};
use super::decode::OtapBatch;
use super::proto::ArrowPayloadType;
use super::OtapError;

const PARENT_ID: &str = "parent_id";

/// Columns the exporter always writes; a table without them is not metrics.
const REQUIRED: [&str; 3] = ["id", "metric_type", "name"];

/// The tables of a metrics batch. Exemplar tables are accepted and not
/// stored.
const TABLES: [ArrowPayloadType; 18] = [
    ArrowPayloadType::ResourceAttrs,
    ArrowPayloadType::ScopeAttrs,
    ArrowPayloadType::UnivariateMetrics,
    ArrowPayloadType::NumberDataPoints,
    ArrowPayloadType::SummaryDataPoints,
    ArrowPayloadType::HistogramDataPoints,
    ArrowPayloadType::ExpHistogramDataPoints,
    ArrowPayloadType::NumberDpAttrs,
    ArrowPayloadType::SummaryDpAttrs,
    ArrowPayloadType::HistogramDpAttrs,
    ArrowPayloadType::ExpHistogramDpAttrs,
    ArrowPayloadType::NumberDpExemplars,
    ArrowPayloadType::HistogramDpExemplars,
    ArrowPayloadType::ExpHistogramDpExemplars,
    ArrowPayloadType::NumberDpExemplarAttrs,
    ArrowPayloadType::HistogramDpExemplarAttrs,
    ArrowPayloadType::ExpHistogramDpExemplarAttrs,
    ArrowPayloadType::MetricAttrs,
];
const NUMBER_DATA_POINTS: &str = "NUMBER_DATA_POINTS";
const HISTOGRAM_DATA_POINTS: &str = "HISTOGRAM_DATA_POINTS";
const SUMMARY_DATA_POINTS: &str = "SUMMARY_DATA_POINTS";
const EXP_HISTOGRAM_DATA_POINTS: &str = "EXP_HISTOGRAM_DATA_POINTS";

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
    batch.only("metrics", &TABLES)?;
    let metrics_rb = batch
        .get(ArrowPayloadType::UnivariateMetrics)
        .ok_or(OtapError::MissingTable("UNIVARIATE_METRICS"))?;
    let mut root = Root::new("UNIVARIATE_METRICS", metrics_rb, batch, &REQUIRED)?;
    let mut points = DataPoints::decode(batch)?;

    let mut out = ExportMetricsServiceRequest::default();
    let mut groups = Groups::default();
    for i in 0..root.len() {
        root.table.tick()?;
        let (r, new_resource) = groups.resource(root.resource_id(i), out.resource_metrics.len());
        if new_resource {
            out.resource_metrics.push(ResourceMetrics {
                resource: root.resource(i)?,
                scope_metrics: Vec::new(),
                schema_url: root.resource_schema_url(i)?,
            });
        }
        let rm = &mut out.resource_metrics[r];
        let (s, new_scope) = groups.scope(r, root.scope_id(i), rm.scope_metrics.len());
        if new_scope {
            rm.scope_metrics.push(ScopeMetrics {
                scope: root.scope(i)?,
                metrics: Vec::new(),
                schema_url: root.scope_schema_url(i)?,
            });
        }
        let metric = metric_at(&root, i, &mut points)?;
        rm.scope_metrics[s].metrics.push(metric);
    }
    points.finish()?;
    root.finish()?;
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
            data_points: vec![ExponentialHistogramDataPoint::default(); points.exp_histograms(id)],
            aggregation_temporality: t.i32_at("aggregation_temporality", i)?.unwrap_or(0),
        })),
        metric_type::SUMMARY => Some(Data::Summary(Summary {
            data_points: vec![SummaryDataPoint::default(); points.summaries(id)],
        })),
        0 => None,
        other => {
            return Err(t.row_error("metric_type", i, &format!("unknown metric type {other}")))
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

/// Data point tables keyed by metric id. Every entry is removed by the
/// metric that owns it; what is left at the end has no parent.
#[derive(Default)]
struct DataPoints {
    numbers: HashMap<u32, Vec<NumberDataPoint>>,
    histograms: HashMap<u32, Vec<HistogramDataPoint>>,
    summaries: HashMap<u32, usize>,
    exp_histograms: HashMap<u32, usize>,
}

/// A data point table with its metric ids and row ids.
struct Points {
    table: Table,
    parents: Vec<u32>,
    ids: Vec<Option<u32>>,
}

impl Points {
    fn decode(
        batch: &OtapBatch,
        payload: ArrowPayloadType,
        name: &'static str,
    ) -> Result<Option<Self>, OtapError> {
        let Some(rb) = batch.get(payload) else {
            return Ok(None);
        };
        let table = Table::new(name, rb, batch.budget())?;
        let parents: Vec<u32> = table.ids(PARENT_ID)?.into_iter().flatten().collect();
        if parents.len() < table.len() {
            return Err(table.orphans(table.len() - parents.len()));
        }
        let ids = table.unique_ids("id")?;
        Ok(Some(Self {
            table,
            parents,
            ids,
        }))
    }
}

impl DataPoints {
    fn decode(batch: &OtapBatch) -> Result<Self, OtapError> {
        let mut out = Self::default();
        let mut attrs = Attrs::decode(batch, ArrowPayloadType::NumberDpAttrs, "NUMBER_DP_ATTRS")?;
        if let Some(points) = Points::decode(
            batch,
            ArrowPayloadType::NumberDataPoints,
            NUMBER_DATA_POINTS,
        )? {
            let t = &points.table;
            for (i, parent) in points.parents.iter().enumerate() {
                t.tick()?;
                let value = match (t.i64_at("int_value", i)?, t.f64_at("double_value", i)?) {
                    (Some(_), Some(_)) => {
                        return Err(t.row_error(
                            "int_value",
                            i,
                            "both int_value and double_value are set",
                        ))
                    }
                    (Some(v), None) => Some(number_data_point::Value::AsInt(v)),
                    (None, Some(v)) => Some(number_data_point::Value::AsDouble(v)),
                    (None, None) => None,
                };
                out.numbers
                    .entry(*parent)
                    .or_default()
                    .push(NumberDataPoint {
                        attributes: attrs.take(points.ids[i]),
                        start_time_unix_nano: t.timestamp_at("start_time_unix_nano", i)?,
                        time_unix_nano: t.timestamp_at("time_unix_nano", i)?,
                        exemplars: Vec::new(),
                        flags: t.u32_at("flags", i)?.unwrap_or(0),
                        value,
                    });
            }
        }
        attrs.finish()?;
        let mut attrs = Attrs::decode(
            batch,
            ArrowPayloadType::HistogramDpAttrs,
            "HISTOGRAM_DP_ATTRS",
        )?;
        if let Some(points) = Points::decode(
            batch,
            ArrowPayloadType::HistogramDataPoints,
            HISTOGRAM_DATA_POINTS,
        )? {
            let t = &points.table;
            for (i, parent) in points.parents.iter().enumerate() {
                t.tick()?;
                out.histograms
                    .entry(*parent)
                    .or_default()
                    .push(HistogramDataPoint {
                        attributes: attrs.take(points.ids[i]),
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
        attrs.finish()?;
        out.summaries = count_by_parent(
            batch,
            ArrowPayloadType::SummaryDataPoints,
            SUMMARY_DATA_POINTS,
        )?;
        out.exp_histograms = count_by_parent(
            batch,
            ArrowPayloadType::ExpHistogramDataPoints,
            EXP_HISTOGRAM_DATA_POINTS,
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

    fn summaries(&mut self, id: Option<u32>) -> usize {
        id.and_then(|id| self.summaries.remove(&id)).unwrap_or(0)
    }

    fn exp_histograms(&mut self, id: Option<u32>) -> usize {
        id.and_then(|id| self.exp_histograms.remove(&id))
            .unwrap_or(0)
    }

    /// Fail when data points are left that no metric of their kind owns.
    fn finish(self) -> Result<(), OtapError> {
        let left = [
            (
                NUMBER_DATA_POINTS,
                self.numbers.values().map(Vec::len).sum::<usize>(),
            ),
            (
                HISTOGRAM_DATA_POINTS,
                self.histograms.values().map(Vec::len).sum(),
            ),
            (SUMMARY_DATA_POINTS, self.summaries.values().sum()),
            (
                EXP_HISTOGRAM_DATA_POINTS,
                self.exp_histograms.values().sum(),
            ),
        ];
        match left.into_iter().find(|(_, count)| *count > 0) {
            Some((table, count)) => Err(orphans(table, count)),
            None => Ok(()),
        }
    }
}

fn count_by_parent(
    batch: &OtapBatch,
    payload: ArrowPayloadType,
    name: &'static str,
) -> Result<HashMap<u32, usize>, OtapError> {
    let mut counts = HashMap::new();
    if let Some(points) = Points::decode(batch, payload, name)? {
        for parent in points.parents {
            *counts.entry(parent).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
