//! `BatchArrowRecords` → one record batch per OTAP table.
//!
//! The IPC bytes of a payload are a slice of a long-lived Arrow IPC stream:
//! the first payload carrying a given `schema_id` starts with the schema
//! message (and any dictionaries), later ones hold only record batches and
//! dictionary deltas. One [`StreamConsumer`] therefore lives as long as the
//! gRPC stream and keeps one IPC reader per schema id.

use std::collections::HashMap;
use std::io::Cursor;

use arrow::array::RecordBatch;
use arrow::ipc::reader::StreamReader;

use super::proto::{ArrowPayloadType, BatchArrowRecords};
use super::OtapError;

/// The decoded tables of one `BatchArrowRecords`.
#[derive(Default, Debug)]
pub struct OtapBatch {
    tables: HashMap<i32, RecordBatch>,
}

impl OtapBatch {
    pub fn get(&self, payload: ArrowPayloadType) -> Option<&RecordBatch> {
        self.tables.get(&(payload as i32))
    }

    pub fn insert(&mut self, payload: ArrowPayloadType, batch: RecordBatch) {
        self.tables.insert(payload as i32, batch);
    }

    /// Which tables the batch carries, for diagnostics.
    pub fn payload_types(&self) -> Vec<ArrowPayloadType> {
        let mut types: Vec<ArrowPayloadType> = self
            .tables
            .keys()
            .filter_map(|&k| ArrowPayloadType::try_from(k).ok())
            .collect();
        types.sort();
        types
    }
}

struct SchemaStream {
    payload_type: ArrowPayloadType,
    reader: StreamReader<Cursor<Vec<u8>>>,
}

/// Per-gRPC-stream IPC state.
#[derive(Default)]
pub struct StreamConsumer {
    streams: HashMap<String, SchemaStream>,
}

impl StreamConsumer {
    /// Decode every payload of the batch. A new `schema_id` for a payload
    /// type replaces that type's previous reader (the producer never
    /// interleaves schemas of one type).
    pub fn consume(&mut self, records: BatchArrowRecords) -> Result<OtapBatch, OtapError> {
        let mut out = OtapBatch::default();
        for payload in records.arrow_payloads {
            let payload_type = ArrowPayloadType::try_from(payload.r#type)
                .map_err(|_| OtapError::UnknownPayloadType(payload.r#type))?;
            let name = payload_type.as_str_name();
            let ipc = |source| OtapError::Ipc {
                payload: name,
                source,
            };
            let stream = if let Some(stream) = self.streams.get_mut(&payload.schema_id) {
                *stream.reader.get_mut() = Cursor::new(payload.record);
                stream
            } else {
                self.streams.retain(|_, s| s.payload_type != payload_type);
                let reader =
                    StreamReader::try_new(Cursor::new(payload.record), None).map_err(ipc)?;
                self.streams
                    .entry(payload.schema_id.clone())
                    .or_insert(SchemaStream {
                        payload_type,
                        reader,
                    })
            };
            match stream.reader.next() {
                Some(Ok(batch)) => out.insert(payload_type, batch),
                Some(Err(source)) => return Err(ipc(source)),
                None => {
                    return Err(OtapError::EmptyPayload {
                        payload: name,
                        schema_id: payload.schema_id,
                    })
                }
            }
        }
        Ok(out)
    }
}
