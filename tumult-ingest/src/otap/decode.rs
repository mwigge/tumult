//! `BatchArrowRecords` → one record batch per OTAP table.
//!
//! The IPC bytes of a payload are a slice of a long-lived Arrow IPC stream:
//! the first payload carrying a given `schema_id` starts with the schema
//! message (and any dictionaries), later ones hold only one record batch and
//! dictionary batches. One [`StreamConsumer`] therefore lives as long as the
//! gRPC stream and keeps one IPC reader per payload type.
//!
//! Nothing a payload declares is trusted: [`scan`] walks the IPC messages of
//! every payload and checks body lengths, row counts and declared
//! decompressed sizes against the stream's [`OtapLimits`] before the Arrow
//! reader allocates anything for them.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::ipc::reader::StreamReader;
use arrow::ipc::{CompressionType, MessageHeader};

use super::limits::{Budget, OtapLimits};
use super::proto::{ArrowPayloadType, BatchArrowRecords};
use super::OtapError;

/// The decoded tables of one `BatchArrowRecords`.
#[derive(Debug)]
pub struct OtapBatch {
    tables: HashMap<i32, RecordBatch>,
    budget: Arc<Budget>,
    continued: usize,
}

impl Default for OtapBatch {
    fn default() -> Self {
        Self::new(OtapLimits::default())
    }
}

impl OtapBatch {
    /// An empty batch that will be decoded under `limits`.
    #[must_use]
    pub fn new(limits: OtapLimits) -> Self {
        Self {
            tables: HashMap::new(),
            budget: Arc::new(Budget::new(limits)),
            continued: 0,
        }
    }

    /// How many tables the batch carries.
    #[must_use]
    pub fn payloads(&self) -> usize {
        self.tables.len()
    }

    /// How many of them were decoded with a schema (and dictionaries) from
    /// an earlier batch of the stream rather than one sent with them.
    #[must_use]
    pub fn continued_payloads(&self) -> usize {
        self.continued
    }

    #[must_use]
    pub fn get(&self, payload: ArrowPayloadType) -> Option<&RecordBatch> {
        self.tables.get(&(payload as i32))
    }

    /// Add a table.
    ///
    /// # Errors
    /// A batch carries at most one table per payload type.
    pub fn insert(
        &mut self,
        payload: ArrowPayloadType,
        batch: RecordBatch,
    ) -> Result<(), OtapError> {
        if self.tables.insert(payload as i32, batch).is_some() {
            return Err(OtapError::DuplicatePayload(payload.as_str_name()));
        }
        Ok(())
    }

    /// Which tables the batch carries, for diagnostics.
    #[must_use]
    pub fn payload_types(&self) -> Vec<ArrowPayloadType> {
        let mut types: Vec<ArrowPayloadType> = self
            .tables
            .keys()
            .filter_map(|&k| ArrowPayloadType::try_from(k).ok())
            .collect();
        types.sort();
        types
    }

    /// What the batch has left of its per-batch limits.
    pub(super) fn budget(&self) -> &Arc<Budget> {
        &self.budget
    }
}

struct SchemaStream {
    schema_id: String,
    reader: StreamReader<Cursor<Vec<u8>>>,
    /// Rows added to this stream's dictionaries by delta batches.
    dictionary_rows: usize,
}

/// Per-gRPC-stream IPC state.
#[derive(Default)]
pub struct StreamConsumer {
    limits: OtapLimits,
    /// One reader per payload type: a schema id only means something within
    /// its own table.
    streams: HashMap<i32, SchemaStream>,
    /// Set by the first failed batch. Later payloads may depend on schemas
    /// and dictionaries the failed one carried, so nothing more is decoded.
    poisoned: bool,
}

impl StreamConsumer {
    #[must_use]
    pub fn new(limits: OtapLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Decode every payload of the batch. A new `schema_id` for a payload
    /// type replaces that type's previous reader.
    ///
    /// # Errors
    /// Any malformed payload or exceeded limit. The consumer refuses every
    /// later batch with [`OtapError::Poisoned`]: the caller must end the
    /// stream.
    pub fn consume(&mut self, records: BatchArrowRecords) -> Result<OtapBatch, OtapError> {
        if self.poisoned {
            return Err(OtapError::Poisoned);
        }
        let result = self.decode(records);
        self.poisoned = result.is_err();
        result
    }

    fn decode(&mut self, records: BatchArrowRecords) -> Result<OtapBatch, OtapError> {
        let mut out = OtapBatch::new(self.limits);
        for payload in records.arrow_payloads {
            let payload_type = ArrowPayloadType::try_from(payload.r#type)
                .map_err(|_| OtapError::UnknownPayloadType(payload.r#type))?;
            let name = payload_type.as_str_name();
            if out.get(payload_type).is_some() {
                return Err(OtapError::DuplicatePayload(name));
            }
            let known = self
                .streams
                .get(&payload.r#type)
                .is_some_and(|s| s.schema_id == payload.schema_id);
            let scanned = scan(name, &payload.record, !known, &out.budget)?;
            if scanned.record_batches == 0 {
                return Err(OtapError::EmptyPayload {
                    payload: name,
                    schema_id: payload.schema_id,
                });
            }
            let ipc = |source| OtapError::Ipc {
                payload: name,
                source,
            };
            if known {
                out.continued += 1;
                if let Some(stream) = self.streams.get_mut(&payload.r#type) {
                    *stream.reader.get_mut() = Cursor::new(payload.record);
                }
            } else {
                let reader =
                    StreamReader::try_new(Cursor::new(payload.record), None).map_err(ipc)?;
                self.streams.insert(
                    payload.r#type,
                    SchemaStream {
                        schema_id: payload.schema_id.clone(),
                        reader,
                        dictionary_rows: 0,
                    },
                );
            }
            let Some(stream) = self.streams.get_mut(&payload.r#type) else {
                continue;
            };
            stream.dictionary_rows = stream
                .dictionary_rows
                .saturating_add(scanned.delta_dictionary_rows);
            if stream.dictionary_rows > self.limits.max_rows_per_table {
                return Err(OtapError::Limit {
                    what: "dictionary rows per stream",
                    limit: self.limits.max_rows_per_table,
                });
            }
            match stream.reader.next() {
                Some(Ok(batch)) => out.insert(payload_type, batch)?,
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

/// What [`scan`] found in one payload.
struct Scanned {
    record_batches: usize,
    delta_dictionary_rows: usize,
}

/// Marks the start of every IPC message (the encapsulated message format).
const CONTINUATION: [u8; 4] = [0xff; 4];

/// Size of the uncompressed-length prefix of a compressed IPC buffer.
const LENGTH_PREFIX: usize = 8;

/// Prefix value of a buffer stored uncompressed in a compressed body.
const NOT_COMPRESSED: i64 = -1;

/// Walk the IPC messages of one payload without decoding them.
///
/// A payload is `[schema] dictionary-batch* record-batch`, the schema only
/// (and always) when the reader for it is new. Checked here, before the
/// Arrow reader sees the bytes:
///
/// * every message's metadata and body lie within the payload (the reader
///   would allocate a declared body length before reading it);
/// * row counts and array lengths are within the limits (a zero-column or
///   null-typed batch has no buffers to back its length);
/// * the bytes the body decodes to, read from each compressed buffer's
///   length prefix, fit the batch's budget (the reader allocates the
///   declared size before decompressing); only zstd is accepted, whose
///   decoder never writes past the declared size;
/// * nothing follows the record batch.
fn scan(
    payload: &'static str,
    bytes: &[u8],
    expect_schema: bool,
    budget: &Budget,
) -> Result<Scanned, OtapError> {
    let malformed = |reason: String| OtapError::Malformed { payload, reason };
    let mut found = Scanned {
        record_batches: 0,
        delta_dictionary_rows: 0,
    };
    let mut rest = bytes;
    let mut first = true;
    while !rest.is_empty() {
        if found.record_batches > 0 {
            return Err(malformed("data after the record batch".into()));
        }
        let Some((prefix, after_prefix)) = rest.split_first_chunk::<8>() else {
            return Err(malformed("truncated message prefix".into()));
        };
        if prefix[..4] != CONTINUATION {
            return Err(malformed("missing continuation marker".into()));
        }
        let declared = i32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
        let meta_len = usize::try_from(declared)
            .ok()
            .filter(|&n| n > 0 && n <= after_prefix.len())
            .ok_or_else(|| {
                malformed(format!("metadata length {declared} is not in the payload"))
            })?;
        let (meta, after_meta) = after_prefix.split_at(meta_len);
        let message = arrow::ipc::root_as_message(meta)
            .map_err(|_| malformed("invalid message metadata".into()))?;
        let body_len = usize::try_from(message.bodyLength())
            .ok()
            .filter(|&n| n <= after_meta.len())
            .ok_or_else(|| {
                malformed(format!(
                    "body length {} is not in the payload",
                    message.bodyLength()
                ))
            })?;
        let (body, after_body) = after_meta.split_at(body_len);
        let header = message.header_type();
        if (header == MessageHeader::Schema) != (first && expect_schema) {
            return Err(malformed(if first && expect_schema {
                "a new schema id must start with the schema message".into()
            } else {
                "unexpected schema message".into()
            }));
        }
        match header {
            MessageHeader::Schema => {}
            MessageHeader::DictionaryBatch => {
                let dictionary = message
                    .header_as_dictionary_batch()
                    .and_then(|d| d.data().map(|data| (d.isDelta(), data)));
                let Some((is_delta, data)) = dictionary else {
                    return Err(malformed("dictionary batch without data".into()));
                };
                let rows = check_batch(payload, &data, body, budget)?;
                if is_delta {
                    found.delta_dictionary_rows = found.delta_dictionary_rows.saturating_add(rows);
                }
            }
            MessageHeader::RecordBatch => {
                let Some(batch) = message.header_as_record_batch() else {
                    return Err(malformed("record batch without header".into()));
                };
                check_batch(payload, &batch, body, budget)?;
                found.record_batches += 1;
            }
            other => {
                return Err(malformed(format!(
                    "unsupported IPC message {}",
                    other.variant_name().unwrap_or("?")
                )))
            }
        }
        first = false;
        rest = after_body;
    }
    Ok(found)
}

/// Check one record (or dictionary) batch header against its body and the
/// limits; returns its row count.
fn check_batch(
    payload: &'static str,
    batch: &arrow::ipc::RecordBatch<'_>,
    body: &[u8],
    budget: &Budget,
) -> Result<usize, OtapError> {
    let malformed = |reason: String| OtapError::Malformed { payload, reason };
    let limits = budget.limits();
    let rows = usize::try_from(batch.length())
        .map_err(|_| malformed(format!("negative row count {}", batch.length())))?;
    if rows > limits.max_rows_per_table {
        return Err(OtapError::Limit {
            what: "rows per table",
            limit: limits.max_rows_per_table,
        });
    }
    // Nested arrays (list values) may be longer than the table, but every
    // element of an array with buffers costs at least a bit of body, so no
    // array may claim more elements than the batch may decode to in bytes.
    for node in batch.nodes().iter().flatten() {
        let len = usize::try_from(node.length())
            .map_err(|_| malformed(format!("negative array length {}", node.length())))?;
        if len > limits.max_decoded_bytes_per_batch {
            return Err(OtapError::Limit {
                what: "array length",
                limit: limits.max_decoded_bytes_per_batch,
            });
        }
    }
    let Some(compression) = batch.compression() else {
        return budget.charge_bytes(body.len()).map(|()| rows);
    };
    if compression.codec() != CompressionType::ZSTD {
        return Err(malformed("only zstd IPC compression is supported".into()));
    }
    for buffer in batch.buffers().iter().flatten() {
        let slice = usize::try_from(buffer.offset())
            .ok()
            .zip(usize::try_from(buffer.length()).ok())
            .and_then(|(offset, len)| body.get(offset..offset.checked_add(len)?))
            .ok_or_else(|| malformed("buffer outside the message body".into()))?;
        if slice.is_empty() {
            continue;
        }
        let Some((prefix, data)) = slice.split_first_chunk::<LENGTH_PREFIX>() else {
            return Err(malformed("compressed buffer without length prefix".into()));
        };
        let declared = i64::from_le_bytes(*prefix);
        let decoded = if declared == NOT_COMPRESSED {
            data.len()
        } else {
            usize::try_from(declared)
                .map_err(|_| malformed(format!("invalid uncompressed length {declared}")))?
        };
        budget.charge_bytes(decoded)?;
    }
    Ok(rows)
}
