//! A real store that refuses a write, for the tests of what a sender is
//! told when its batch was not stored.
//!
//! The writer's own connection renames `metric_gauges` away (`DuckDB`
//! renames a table only without its index, so the index goes first). A
//! metrics batch with a gauge then fails inside its transaction, after its
//! sums went in. Putting the table back removes the cause.

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
    Sum,
};

use crate::writer::{Batch, IngestWriter};

const HIDE: [&str; 2] = [
    "DROP INDEX idx_metric_gauges_name_ts",
    "ALTER TABLE metric_gauges RENAME TO metric_gauges_hidden",
];

const RESTORE: [&str; 2] = [
    "ALTER TABLE metric_gauges_hidden RENAME TO metric_gauges",
    "CREATE INDEX idx_metric_gauges_name_ts ON metric_gauges (metric_name, ts_ns)",
];

/// Rename `metric_gauges` away: every metrics batch with a gauge fails.
pub(crate) async fn hide_gauges(ingest: &IngestWriter) {
    run(ingest, HIDE).await;
}

/// Put `metric_gauges` and its index back.
pub(crate) async fn restore_gauges(ingest: &IngestWriter) {
    run(ingest, RESTORE).await;
}

async fn run(ingest: &IngestWriter, statements: [&'static str; 2]) {
    ingest
        .write(Batch::Exec(Box::new(move |writer| {
            for sql in statements {
                writer.execute(sql, []).map_err(|e| e.to_string())?;
            }
            Ok(())
        })))
        .await
        .unwrap();
}

/// An export of one sum point and one gauge point.
pub(crate) fn sum_and_gauge() -> ExportMetricsServiceRequest {
    let point = |value: f64| NumberDataPoint {
        time_unix_nano: 1_785_000_001_000_000_000,
        value: Some(number_data_point::Value::AsDouble(value)),
        ..NumberDataPoint::default()
    };
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![
                    Metric {
                        name: "demo.requests".into(),
                        data: Some(Data::Sum(Sum {
                            data_points: vec![point(3.0)],
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                        ..Metric::default()
                    },
                    Metric {
                        name: "demo.cpu".into(),
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![point(0.5)],
                        })),
                        ..Metric::default()
                    },
                ],
                ..ScopeMetrics::default()
            }],
            ..ResourceMetrics::default()
        }],
    }
}

/// How many sum rows and gauge rows the store holds.
pub(crate) fn stored(store: &tumult_lake::Store) -> (u64, u64) {
    let rows = store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT (SELECT count(*) FROM metric_sums) AS sums, \
             (SELECT count(*) FROM metric_gauges) AS gauges",
        )
        .unwrap();
    let count = |column: &str| rows[0][column].as_u64().unwrap();
    (count("sums"), count("gauges"))
}
