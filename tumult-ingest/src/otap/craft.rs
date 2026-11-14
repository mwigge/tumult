//! Hand-built and mutated Arrow IPC messages, for the hostile tests and the
//! proof probe. A well-behaved Arrow writer cannot produce most of these:
//! buffers that point outside the body, node counts that disagree with the
//! schema, fields the schema never declared.

use arrow::datatypes::Schema;
use arrow::ipc::writer::StreamWriter;
use arrow::ipc::{
    BodyCompression, BodyCompressionArgs, BodyCompressionMethod, Buffer, CompressionType,
    DictionaryBatch, DictionaryBatchArgs, FieldNode, Message, MessageArgs, MessageHeader,
    MetadataVersion, RecordBatch, RecordBatchArgs,
};
use flatbuffers::{FlatBufferBuilder, WIPOffset};

/// Marks the start of every encapsulated IPC message.
const CONTINUATION: [u8; 4] = [0xff; 4];

/// One encapsulated message: continuation marker, metadata length, the
/// metadata padded to eight bytes, then the body.
#[must_use]
pub fn frame(metadata: &[u8], body: &[u8]) -> Vec<u8> {
    let padded = metadata.len().div_ceil(8) * 8;
    let mut out = Vec::with_capacity(8 + padded + body.len());
    out.extend_from_slice(&CONTINUATION);
    out.extend_from_slice(&i32::try_from(padded).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(metadata);
    out.resize(8 + padded, 0);
    out.extend_from_slice(body);
    out
}

/// The schema message an Arrow writer opens a stream with.
///
/// # Panics
/// When the Arrow writer refuses the schema.
#[must_use]
pub fn schema_message(schema: &Schema) -> Vec<u8> {
    StreamWriter::try_new(Vec::new(), schema)
        .expect("schema is writable")
        .get_ref()
        .clone()
}

/// What a record batch header declares; none of it is checked against the
/// body.
#[derive(Clone, Debug, Default)]
pub struct BatchHeader {
    pub rows: i64,
    /// `(length, null_count)` per field node.
    pub nodes: Vec<(i64, i64)>,
    /// `(offset, length)` per buffer.
    pub buffers: Vec<(i64, i64)>,
    /// `Some` writes a `variadicBufferCounts` vector.
    pub variadic: Option<Vec<i64>>,
    /// Declare the buffers zstd-compressed.
    pub zstd: bool,
    /// The `bodyLength` to declare instead of the real one.
    pub body_length: Option<i64>,
}

fn batch_table<'a>(
    fbb: &mut FlatBufferBuilder<'a>,
    header: &BatchHeader,
) -> WIPOffset<RecordBatch<'a>> {
    let nodes: Vec<FieldNode> = header
        .nodes
        .iter()
        .map(|&(length, nulls)| FieldNode::new(length, nulls))
        .collect();
    let buffers: Vec<Buffer> = header
        .buffers
        .iter()
        .map(|&(offset, length)| Buffer::new(offset, length))
        .collect();
    let nodes = fbb.create_vector(&nodes);
    let buffers = fbb.create_vector(&buffers);
    let variadic = header.variadic.as_ref().map(|v| fbb.create_vector(v));
    let compression = header.zstd.then(|| {
        BodyCompression::create(
            fbb,
            &BodyCompressionArgs {
                codec: CompressionType::ZSTD,
                method: BodyCompressionMethod::BUFFER,
            },
        )
    });
    RecordBatch::create(
        fbb,
        &RecordBatchArgs {
            length: header.rows,
            nodes: Some(nodes),
            buffers: Some(buffers),
            compression,
            variadicBufferCounts: variadic,
        },
    )
}

fn message(
    mut fbb: FlatBufferBuilder<'_>,
    header_type: MessageHeader,
    header: WIPOffset<flatbuffers::UnionWIPOffset>,
    body_length: i64,
    body: &[u8],
) -> Vec<u8> {
    let root = Message::create(
        &mut fbb,
        &MessageArgs {
            version: MetadataVersion::V5,
            header_type,
            header: Some(header),
            bodyLength: body_length,
            custom_metadata: None,
        },
    );
    fbb.finish(root, None);
    frame(fbb.finished_data(), body)
}

fn declared_length(header: &BatchHeader, body: &[u8]) -> i64 {
    header
        .body_length
        .unwrap_or_else(|| i64::try_from(body.len()).unwrap_or(i64::MAX))
}

/// A record batch message with exactly the header given.
#[must_use]
pub fn batch_message(header: &BatchHeader, body: &[u8]) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let batch = batch_table(&mut fbb, header).as_union_value();
    let length = declared_length(header, body);
    message(fbb, MessageHeader::RecordBatch, batch, length, body)
}

/// A dictionary batch message for dictionary `id`.
#[must_use]
pub fn dictionary_message(id: i64, is_delta: bool, header: &BatchHeader, body: &[u8]) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let data = batch_table(&mut fbb, header);
    let dictionary = DictionaryBatch::create(
        &mut fbb,
        &DictionaryBatchArgs {
            id,
            data: Some(data),
            isDelta: is_delta,
        },
    )
    .as_union_value();
    let length = declared_length(header, body);
    message(
        fbb,
        MessageHeader::DictionaryBatch,
        dictionary,
        length,
        body,
    )
}

/// Where the flatbuffer metadata of each encapsulated message of `bytes`
/// lies, as `(start, end)`; stops at the first message it cannot follow.
#[must_use]
pub fn metadata_ranges(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(prefix) = bytes.get(at..at + 8) {
        let declared = i32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
        let Ok(len) = usize::try_from(declared) else {
            break;
        };
        let (start, end) = (at + 8, at + 8 + len);
        let Some(Ok(message)) = bytes.get(start..end).map(arrow::ipc::root_as_message) else {
            break;
        };
        out.push((start, end));
        let Ok(body) = usize::try_from(message.bodyLength()) else {
            break;
        };
        at = end.saturating_add(body);
    }
    out
}

/// One record or dictionary batch message of a payload, taken apart.
struct Parsed {
    /// Where the message lies in the payload, body included.
    start: usize,
    end: usize,
    /// `Some((id, is_delta))` for a dictionary batch.
    dictionary: Option<(i64, bool)>,
    header: BatchHeader,
    body: Vec<u8>,
}

fn parsed_header(batch: RecordBatch<'_>) -> BatchHeader {
    BatchHeader {
        rows: batch.length(),
        nodes: batch
            .nodes()
            .into_iter()
            .flatten()
            .map(|n| (n.length(), n.null_count()))
            .collect(),
        buffers: batch
            .buffers()
            .into_iter()
            .flatten()
            .map(|b| (b.offset(), b.length()))
            .collect(),
        variadic: None,
        zstd: batch.compression().is_some(),
        body_length: None,
    }
}

/// The record and dictionary batch messages of `bytes` that parse.
fn batches(bytes: &[u8]) -> Vec<Parsed> {
    let mut out = Vec::new();
    for (meta_start, meta_end) in metadata_ranges(bytes) {
        let Ok(message) = arrow::ipc::root_as_message(&bytes[meta_start..meta_end]) else {
            break;
        };
        let body_end = usize::try_from(message.bodyLength())
            .ok()
            .and_then(|len| meta_end.checked_add(len));
        let Some(body) = body_end.and_then(|end| bytes.get(meta_end..end)) else {
            break;
        };
        let dictionary = message.header_as_dictionary_batch();
        let batch = match dictionary {
            Some(dictionary) => dictionary.data(),
            None => message.header_as_record_batch(),
        };
        let Some(batch) = batch else {
            continue;
        };
        out.push(Parsed {
            start: meta_start - 8,
            end: meta_end + body.len(),
            dictionary: dictionary.map(|d| (d.id(), d.isDelta())),
            header: parsed_header(batch),
            body: body.to_vec(),
        });
    }
    out
}

/// A small deterministic generator (xorshift64*): the same seed always
/// yields the same mutations, so a failing case can be replayed by number.
#[derive(Clone, Debug)]
pub struct Mutator(u64);

/// Integers that sit on the edges of the checks a decoder makes.
const INTERESTING: [i64; 14] = [
    0,
    1,
    -1,
    -2,
    7,
    8,
    255,
    256,
    65_536,
    2_147_483_647,
    1 << 40,
    i64::MAX,
    i64::MIN,
    0x0101_0101_0101_0101,
];

impl Mutator {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn step(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `bound` (`0` when `bound` is `0`).
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.step() % u64::try_from(bound).unwrap_or(u64::MAX)).unwrap_or(0)
    }

    /// Damage `bytes` in one of several ways: flip a bit, overwrite an
    /// aligned integer with an edge value, truncate, cut a range out, or
    /// copy one range over another.
    pub fn mutate(&mut self, bytes: &mut Vec<u8>) {
        let end = bytes.len();
        self.mutate_within(bytes, 0, end);
    }

    /// [`Self::mutate`] with the damage starting inside `start..end` (the
    /// flatbuffer metadata of one message, say).
    pub fn mutate_within(&mut self, bytes: &mut Vec<u8>, start: usize, end: usize) {
        let end = end.min(bytes.len());
        if start >= end {
            return;
        }
        let at = start + self.below(end - start);
        match self.below(8) {
            0 => bytes[at] ^= 1 << self.below(8),
            1 => bytes[at] = u8::try_from(self.below(256)).unwrap_or(0),
            2 => self.overwrite(bytes, at / 8 * 8, 8),
            3 => self.overwrite(bytes, at / 4 * 4, 4),
            4 => self.overwrite(bytes, at / 2 * 2, 2),
            5 => bytes.truncate(at),
            6 => {
                let len = self.below(64).min(bytes.len() - at);
                bytes.drain(at..at + len);
            }
            _ => {
                let len = self.below(64).min(bytes.len() - at);
                let from = self.below(bytes.len() - len + 1);
                bytes.copy_within(from..from + len, at);
            }
        }
    }

    /// A number near `value` or on the edge of a check.
    fn nearby(&mut self, value: i64) -> i64 {
        match self.below(6) {
            0 => value.wrapping_add(1),
            1 => value.wrapping_sub(1),
            2 => value.wrapping_add(8),
            3 => value.wrapping_mul(2),
            _ => INTERESTING[self.below(INTERESTING.len())],
        }
    }

    /// Change what one batch header declares.
    fn restructure(&mut self, header: &mut BatchHeader) {
        let (node, buffer) = (
            self.below(header.nodes.len()),
            self.below(header.buffers.len()),
        );
        match self.below(12) {
            0 => header.rows = self.nearby(header.rows),
            1 | 2 if !header.nodes.is_empty() => {
                header.nodes[node].0 = self.nearby(header.nodes[node].0);
            }
            3 | 4 if !header.nodes.is_empty() => {
                header.nodes[node].1 = self.nearby(header.nodes[node].1);
            }
            5 | 6 if !header.buffers.is_empty() => {
                header.buffers[buffer].0 = self.nearby(header.buffers[buffer].0);
            }
            7 | 8 if !header.buffers.is_empty() => {
                header.buffers[buffer].1 = self.nearby(header.buffers[buffer].1);
            }
            9 if !header.buffers.is_empty() => {
                let other = self.below(header.buffers.len());
                header.buffers.swap(buffer, other);
            }
            10 if !header.nodes.is_empty() => {
                let other = self.below(header.nodes.len());
                header.nodes.swap(node, other);
            }
            _ => header.zstd = !header.zstd,
        }
    }

    /// Damage that the flatbuffer verifier cannot see: take one record or
    /// dictionary batch message of `bytes` apart, change what its header
    /// declares (row count, a node, a buffer, the compression, the
    /// dictionary id) or a byte of its body, and write it back as a
    /// well-formed message. Returns `false` when `bytes` holds no such
    /// message.
    pub fn mutate_structure(&mut self, bytes: &mut Vec<u8>) -> bool {
        let mut found = batches(bytes);
        if found.is_empty() {
            return false;
        }
        let mut chosen = found.swap_remove(self.below(found.len()));
        for _ in 0..=self.below(2) {
            match self.below(8) {
                0 => {
                    if let Some((id, is_delta)) = &mut chosen.dictionary {
                        *id = self.nearby(*id);
                        *is_delta = self.below(2) == 0;
                    }
                }
                1 if !chosen.body.is_empty() => {
                    let end = chosen.body.len();
                    self.mutate_within(&mut chosen.body, 0, end);
                }
                _ => self.restructure(&mut chosen.header),
            }
        }
        let message = match chosen.dictionary {
            Some((id, is_delta)) => dictionary_message(id, is_delta, &chosen.header, &chosen.body),
            None => batch_message(&chosen.header, &chosen.body),
        };
        bytes.splice(chosen.start..chosen.end, message);
        true
    }

    fn overwrite(&mut self, bytes: &mut [u8], at: usize, width: usize) {
        let value = INTERESTING[self.below(INTERESTING.len())].to_le_bytes();
        let Some(target) = bytes.get_mut(at..at + width) else {
            return;
        };
        target.copy_from_slice(&value[..width]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{ArrayRef, DictionaryArray, RecordBatch as Table, StringArray, UInt8Array};
    use arrow::datatypes::Field;

    use super::*;

    /// A stream with a schema, a dictionary batch and a record batch.
    fn stream() -> Vec<u8> {
        let keys = UInt8Array::from_iter_values([0, 1, 1]);
        let values = Arc::new(StringArray::from_iter_values(["a", "b"]));
        let column: ArrayRef = Arc::new(DictionaryArray::new(keys, values));
        let field = Field::new("name", column.data_type().clone(), true);
        let table = Table::try_new(Arc::new(Schema::new(vec![field])), vec![column]).unwrap();
        let mut writer = StreamWriter::try_new(Vec::new(), table.schema_ref()).unwrap();
        writer.write(&table).unwrap();
        std::mem::take(writer.get_mut())
    }

    #[test]
    fn a_batch_taken_apart_and_put_back_declares_the_same() {
        let bytes = stream();
        let found = batches(&bytes);
        let kinds: Vec<bool> = found.iter().map(|p| p.dictionary.is_some()).collect();
        assert_eq!(kinds, [true, false], "the schema message is left alone");
        assert_eq!(found[0].dictionary, Some((0, false)));
        assert_eq!((found[0].header.rows, found[1].header.rows), (2, 3));
        assert_eq!(found[1].header.nodes, [(3, 0)]);
        assert_eq!(found[1].end, bytes.len());
        for parsed in &found {
            let again = match parsed.dictionary {
                Some((id, delta)) => dictionary_message(id, delta, &parsed.header, &parsed.body),
                None => batch_message(&parsed.header, &parsed.body),
            };
            let reparsed = batches(&again);
            assert_eq!(reparsed.len(), 1);
            assert_eq!(reparsed[0].dictionary, parsed.dictionary);
            assert_eq!(reparsed[0].header.rows, parsed.header.rows);
            assert_eq!(reparsed[0].header.nodes, parsed.header.nodes);
            assert_eq!(reparsed[0].header.buffers, parsed.header.buffers);
            assert_eq!(reparsed[0].body, parsed.body);
        }
    }

    #[test]
    fn structural_damage_always_leaves_messages_that_verify() {
        let pristine = stream();
        let mut changed = 0;
        for seed in 1..=500 {
            let mut bytes = pristine.clone();
            assert!(Mutator::new(seed).mutate_structure(&mut bytes));
            changed += usize::from(bytes != pristine);
            let ranges = metadata_ranges(&bytes);
            assert_eq!(ranges.len(), 3, "seed {seed}: every message still verifies");
        }
        assert!(changed > 400, "{changed} of 500 seeds changed the stream");
        assert!(!Mutator::new(1).mutate_structure(&mut b"not ipc".to_vec()));
    }
}
