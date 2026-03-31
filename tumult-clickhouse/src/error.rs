//! ClickHouse backend errors.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClickHouseError {
    #[error("clickhouse error: {0}")]
    Client(#[from] clickhouse::error::Error),

    #[error("analytics error: {0}")]
    Analytics(#[from] tumult_analytics::AnalyticsError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ClickHouseError> for tumult_analytics::AnalyticsError {
    fn from(e: ClickHouseError) -> Self {
        tumult_analytics::AnalyticsError::ClickHouse(e.to_string())
    }
}
