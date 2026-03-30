//! Export journal data to Parquet and CSV formats.

use std::fs::File;
use std::path::Path;

use arrow::csv::WriterBuilder as CsvWriterBuilder;
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::error::AnalyticsError;

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

/// Export a RecordBatch to Arrow IPC (Feather) format.
pub fn export_arrow_ipc(batch: &RecordBatch, path: &Path) -> Result<(), AnalyticsError> {
    let file = File::create(path)?;
    let mut writer = arrow::ipc::writer::FileWriter::try_new(file, &batch.schema())?;
    writer.write(batch)?;
    writer.finish()?;
    Ok(())
}

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
    fn parquet_creates_file() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("t.parquet");
        export_parquet(&sample_batch(), &p).unwrap();
        assert!(p.exists());
    }
    #[test]
    fn arrow_ipc_creates_file() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("t.arrow");
        export_arrow_ipc(&sample_batch(), &p).unwrap();
        assert!(p.exists());
        assert!(std::fs::metadata(&p).unwrap().len() > 0);
    }
    #[test]
    fn csv_creates_file() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("t.csv");
        export_csv(&sample_batch(), &p).unwrap();
        let c = std::fs::read_to_string(&p).unwrap();
        assert!(c.contains("id,value,duration"));
    }
}
