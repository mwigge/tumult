//! The statements that write the telemetry: spans, logs, metric points and
//! import batches (`writer.rs`).

pub(crate) const INSERT_SPAN: &str = "INSERT INTO spans VALUES (
    ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(? AS JSON))";

pub(crate) const INSERT_LOG: &str =
    "INSERT INTO logs (ts_ns, severity_text, body, trace_id, span_id,
       service_name, log_attrs, resource_attrs, event_name, scope_attrs)
    VALUES (?,?,?,?,?,?,
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
       ?,
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)))";

pub(crate) const INSERT_METRIC_SUM: &str = "INSERT INTO metric_sums VALUES (?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))";

pub(crate) const INSERT_METRIC_GAUGE: &str = "INSERT INTO metric_gauges VALUES (?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))";

pub(crate) const INSERT_METRIC_HISTOGRAM: &str =
    "INSERT INTO metric_histograms VALUES (?,?,?,?,?,?,
    CAST(? AS BIGINT[]), CAST(? AS DOUBLE[]),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    ?,?,?)";

pub(crate) const INSERT_IMPORT_BATCH: &str = "INSERT INTO import_batches VALUES (?,?,?,?,?)";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT_SPAN,
        INSERT_LOG,
        INSERT_METRIC_SUM,
        INSERT_METRIC_GAUGE,
        INSERT_METRIC_HISTOGRAM,
        INSERT_IMPORT_BATCH
    ]
}
