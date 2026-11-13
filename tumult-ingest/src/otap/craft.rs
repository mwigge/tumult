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

    fn overwrite(&mut self, bytes: &mut [u8], at: usize, width: usize) {
        let value = INTERESTING[self.below(INTERESTING.len())].to_le_bytes();
        let Some(target) = bytes.get_mut(at..at + width) else {
            return;
        };
        target.copy_from_slice(&value[..width]);
    }
}
