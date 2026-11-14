//! `BatchArrowRecords` → one record batch per OTAP table.
//!
//! The IPC bytes of a payload are a slice of a long-lived Arrow IPC stream:
//! the first payload carrying a given `schema_id` starts with the schema
//! message (and any dictionaries), later ones hold only one record batch and
//! dictionary batches. One [`StreamConsumer`] therefore lives as long as the
//! gRPC stream and keeps, per payload type, the checked schema and the
//! dictionaries sent so far.
//!
//! Nothing a payload declares is trusted. The messages are decoded by the
//! strict decoder in [`super::ipc`], never by the general Arrow IPC reader,
//! and what the stream makes the receiver keep between batches is bounded:
//!
//! * bytes of schemas and dictionaries per stream and, through
//!   [`RetainedPool`] accounts, per connection, per peer and across all
//!   streams of the receiver;
//! * delta dictionary batches per schema (each one copies the dictionary it
//!   extends, and the copy is charged to the batch's byte budget);
//! * the length of a `schema_id`.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, RecordBatch};
use arrow::ipc::MessageHeader;

use super::ipc::{self, Plan};
use super::limits::{Budget, OtapLimits, Retained, RetainedPool, MAX_SCHEMA_ID_BYTES};
use super::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords};
use super::OtapError;
use crate::log_guard;

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

    /// Fail when the batch carries a table that is none of `allowed`: a
    /// table no decoder of the stream's signal would look at.
    pub(super) fn only(
        &self,
        signal: &'static str,
        allowed: &[ArrowPayloadType],
    ) -> Result<(), OtapError> {
        match self
            .payload_types()
            .into_iter()
            .find(|payload| !allowed.contains(payload))
        {
            Some(payload) => Err(OtapError::Misplaced {
                payload: payload.as_str_name(),
                signal,
            }),
            None => Ok(()),
        }
    }

    /// What the batch has left of its per-batch limits.
    pub(super) fn budget(&self) -> &Arc<Budget> {
        &self.budget
    }
}

/// What one payload type's IPC stream has established so far.
struct SchemaStream {
    schema_id: String,
    plan: Plan,
    dictionaries: HashMap<i64, ArrayRef>,
    /// Delta dictionary batches applied under this schema.
    deltas: usize,
}

impl SchemaStream {
    /// Bytes this stream keeps the receiver holding between batches.
    fn retained(&self) -> usize {
        self.dictionaries
            .values()
            .map(|values| values.get_array_memory_size())
            .fold(self.schema_id.len() + self.plan.retained(), |a, b| {
                a.saturating_add(b)
            })
    }

    /// Replace or extend one dictionary.
    fn apply(
        &mut self,
        payload: &'static str,
        header: arrow::ipc::DictionaryBatch<'_>,
        body: &[u8],
        budget: &Budget,
    ) -> Result<(), OtapError> {
        let update = ipc::dictionary(payload, &self.plan, header, body, budget)?;
        if !update.is_delta {
            self.dictionaries.insert(update.id, update.values);
            return Ok(());
        }
        let limits = budget.limits();
        let Some(current) = self.dictionaries.get(&update.id) else {
            return Err(OtapError::Malformed {
                payload,
                reason: "delta for a dictionary that was never sent".into(),
            });
        };
        self.deltas += 1;
        if self.deltas > limits.max_dictionary_deltas {
            return Err(OtapError::StreamState {
                what: "dictionary deltas per stream",
                limit: limits.max_dictionary_deltas,
                shared: false,
            });
        }
        if current.len().saturating_add(update.values.len()) > limits.max_rows_per_table {
            return Err(OtapError::Limit {
                what: "dictionary rows per stream",
                limit: limits.max_rows_per_table,
            });
        }
        // Extending copies the whole dictionary: the copy counts as bytes
        // this batch decodes, which bounds the work a run of deltas causes.
        budget.charge_bytes(
            current
                .get_array_memory_size()
                .saturating_add(update.values.get_array_memory_size()),
        )?;
        let joined = arrow::compute::concat(&[current.as_ref(), update.values.as_ref()])
            .map_err(|source| OtapError::Ipc { payload, source })?;
        self.dictionaries.insert(update.id, joined);
        Ok(())
    }
}

/// Per-gRPC-stream IPC state.
#[derive(Default)]
pub struct StreamConsumer {
    limits: OtapLimits,
    /// One schema per payload type: a schema id only means something within
    /// its own table.
    streams: HashMap<i32, SchemaStream>,
    /// Set by the first failed batch. Later payloads may depend on schemas
    /// and dictionaries the failed one carried, so nothing more is decoded.
    poisoned: bool,
    /// What the stream holds in the accounts of retained bytes it draws
    /// on.
    retained: Retained,
}

impl StreamConsumer {
    #[must_use]
    pub fn new(limits: OtapLimits) -> Self {
        Self::with_accounts(limits, Vec::new())
    }

    /// A consumer whose retained bytes also count against `pool`.
    #[must_use]
    pub fn with_pool(limits: OtapLimits, pool: Arc<RetainedPool>) -> Self {
        Self::with_accounts(limits, vec![pool])
    }

    /// A consumer whose retained bytes count against every one of
    /// `accounts`, given from the narrowest (the connection's share) to
    /// the widest (the receiver's pool). The first account without room
    /// is the one a refusal names.
    #[must_use]
    pub fn with_accounts(limits: OtapLimits, accounts: Vec<Arc<RetainedPool>>) -> Self {
        Self {
            limits,
            streams: HashMap::new(),
            poisoned: false,
            retained: Retained::new(accounts),
        }
    }

    /// Decode every payload of the batch. A new `schema_id` for a payload
    /// type replaces that type's previous schema and dictionaries.
    ///
    /// # Errors
    /// Any malformed payload or exceeded limit. The consumer drops what it
    /// kept and refuses every later batch with [`OtapError::Poisoned`]: the
    /// caller must end the stream.
    pub fn consume(&mut self, records: BatchArrowRecords) -> Result<OtapBatch, OtapError> {
        if self.poisoned {
            return Err(OtapError::Poisoned);
        }
        let result = self.decode(records);
        if result.is_err() {
            self.poisoned = true;
            self.release();
        }
        result
    }

    /// Drop everything kept between batches and give its bytes back.
    fn release(&mut self) {
        self.streams.clear();
        // Shrinking cannot fail.
        let _ = self.retained.resize(0);
    }

    fn decode(&mut self, records: BatchArrowRecords) -> Result<OtapBatch, OtapError> {
        let mut out = OtapBatch::new(self.limits);
        for payload in records.arrow_payloads {
            let payload_type = ArrowPayloadType::try_from(payload.r#type)
                .ok()
                .filter(|known| *known != ArrowPayloadType::Unknown)
                .ok_or(OtapError::UnknownPayloadType(payload.r#type))?;
            let name = payload_type.as_str_name();
            if payload.schema_id.len() > MAX_SCHEMA_ID_BYTES {
                return Err(OtapError::Malformed {
                    payload: name,
                    reason: format!("schema id longer than {MAX_SCHEMA_ID_BYTES} bytes"),
                });
            }
            if out.get(payload_type).is_some() {
                return Err(OtapError::DuplicatePayload(name));
            }
            let table = self.table(name, &payload, &mut out)?;
            out.insert(payload_type, table)?;
            self.account()?;
        }
        Ok(out)
    }

    /// Decode the messages of one payload: `[schema] dictionary-batch*
    /// record-batch`, the schema only (and always) when the schema id is
    /// new for the payload type.
    fn table(
        &mut self,
        name: &'static str,
        payload: &ArrowPayload,
        out: &mut OtapBatch,
    ) -> Result<RecordBatch, OtapError> {
        let malformed = |reason: &str| OtapError::Malformed {
            payload: name,
            reason: reason.into(),
        };
        let empty = || OtapError::EmptyPayload {
            payload: name,
            schema_id: log_guard::clean(&payload.schema_id),
        };
        let mut rest = payload.record.as_slice();
        let known = self
            .streams
            .get(&payload.r#type)
            .is_some_and(|s| s.schema_id == payload.schema_id);
        if known {
            out.continued += 1;
        } else {
            if rest.is_empty() {
                return Err(empty());
            }
            let framed = ipc::frame(name, rest)?;
            let Some(schema) = framed.message.header_as_schema() else {
                return Err(malformed(
                    "a new schema id must start with the schema message",
                ));
            };
            let stream = SchemaStream {
                schema_id: payload.schema_id.clone(),
                plan: ipc::plan(name, schema)?,
                dictionaries: HashMap::new(),
                deltas: 0,
            };
            self.streams.insert(payload.r#type, stream);
            rest = framed.rest;
        }
        let Some(stream) = self.streams.get_mut(&payload.r#type) else {
            return Err(empty());
        };
        let budget = out.budget.as_ref();
        let mut table = None;
        while !rest.is_empty() {
            if table.is_some() {
                return Err(malformed("data after the record batch"));
            }
            let framed = ipc::frame(name, rest)?;
            match framed.message.header_type() {
                MessageHeader::Schema => return Err(malformed("unexpected schema message")),
                MessageHeader::DictionaryBatch => {
                    let Some(header) = framed.message.header_as_dictionary_batch() else {
                        return Err(malformed("dictionary batch without data"));
                    };
                    stream.apply(name, header, framed.body, budget)?;
                }
                MessageHeader::RecordBatch => {
                    let Some(header) = framed.message.header_as_record_batch() else {
                        return Err(malformed("record batch without header"));
                    };
                    table = Some(ipc::record_batch(
                        name,
                        &stream.plan,
                        header,
                        framed.body,
                        budget,
                        &stream.dictionaries,
                    )?);
                }
                other => {
                    return Err(OtapError::Malformed {
                        payload: name,
                        reason: format!(
                            "unsupported IPC message {}",
                            other.variant_name().unwrap_or("?")
                        ),
                    })
                }
            }
            rest = framed.rest;
        }
        table.ok_or_else(empty)
    }

    /// Check what the stream now keeps against the per-stream limit and
    /// the accounts it draws on.
    fn account(&mut self) -> Result<(), OtapError> {
        let retained = self
            .streams
            .values()
            .map(SchemaStream::retained)
            .fold(0usize, usize::saturating_add);
        if retained > self.limits.max_stream_retained_bytes {
            return Err(OtapError::StreamState {
                what: "retained bytes per stream",
                limit: self.limits.max_stream_retained_bytes,
                shared: false,
            });
        }
        self.retained.resize(retained)
    }
}
