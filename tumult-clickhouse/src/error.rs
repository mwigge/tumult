//! ClickHouse backend errors.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClickHouseError {
    #[error("clickhouse error: {0}")]
    Client(#[from] clickhouse_client::error::Error),

    #[error("analytics error: {0}")]
    Analytics(#[from] tumult_analytics::AnalyticsError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ClickHouseError> for tumult_analytics::AnalyticsError {
    fn from(e: ClickHouseError) -> Self {
        tumult_analytics::AnalyticsError::Io(std::io::Error::other(e.to_string()))
    }
}
