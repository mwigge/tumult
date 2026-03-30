//! Export journal data to Parquet and CSV formats.

use std::fs::File;
use std::path::Path;

use arrow::csv::WriterBuilder as CsvWriterBuilder;
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::error::AnalyticsError;

/// Export a RecordBatch to Parquet format.
pub fn export_parquet(batch: &RecordBatch, path: &Path) -> Result<(), AnalyticsError> {
    let file = File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(parquet::basic::Compression::ZSTD(Default::default()))
        .build();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
    writer.write(batch)?;
    writer.close()?;
    Ok(())
}

/// Export a RecordBatch to CSV format.
pub fn export_csv(batch: &RecordBatch, path: &Path) -> Result<(), AnalyticsError> {
    let file = File::create(path)?;
    let mut writer = CsvWriterBuilder::new().with_header(true).build(file);
    writer.write(batch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray, UInt64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn sample_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("value", DataType::Int64, false),
            Field::new("duration", DataType::UInt64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(UInt64Array::from(vec![100, 200, 300])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn export_parquet_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.parquet");
        export_parquet(&sample_batch(), &path).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn export_csv_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.csv");
        export_csv(&sample_batch(), &path).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("id,value,duration"));
        assert!(content.contains("a,1,100"));
    }
}
