//! Opens OTel-Arrow traces streams against a running daemon, plays a hostile
//! client, and prints exactly what the server answered. Used by
//! `scripts/proof/otap/proof.sh` for the cases a well-behaved Collector
//! never produces.
//!
//! ```text
//! otap_probe <http://host:port> <case> [bearer-token]
//! otap_probe <http://host:port> fuzz:<cases> [bearer-token]
//! ```
//!
//! Cases: `no-batch`, `malformed`, `oversized`, `buffer-outside`,
//! `buffer-negative`, `variadic`, `union`, `null-rows`, `no-ids`,
//! `zero-ids`, `forged-schema-id`, `rejected-flood`.
//!
//! Output of a case, one line each: `open: <code>: <message>` when the
//! stream is refused, `ack: <batch id> <status> <message>` per batch status,
//! then `closed: <code>: <message>` (or `closed: clean`).
//!
//! `fuzz:<cases>` sends that many damaged copies of a valid batch, each on
//! its own stream, and prints one summary line. It exits non-zero when a
//! batch goes unanswered or a stream ends in a way a refusal does not
//! explain (the daemon is gone, say).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, DurationNanosecondArray, FixedSizeBinaryArray, NullArray, RecordBatch, StringArray,
    TimestampNanosecondArray, UInt16Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit, UnionFields, UnionMode};
use arrow::ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow::ipc::CompressionType;
use tonic::transport::Channel;
use tumult_ingest::otap::craft::{
    batch_message, metadata_ranges, schema_message, BatchHeader, Mutator,
};
use tumult_ingest::otap::proto::arrow_traces_service_client::ArrowTracesServiceClient;
use tumult_ingest::otap::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords, StatusCode};

type Failure = Box<dyn std::error::Error>;

/// One byte more than the daemon's default `KRONIKA_OTAP_MAX_MESSAGE_BYTES`.
const OVERSIZED: usize = tumult_ingest::otap::limits::DEFAULT_MAX_MESSAGE_BYTES + 1;

/// The daemon's default `KRONIKA_OTAP_MAX_ROWS`.
const MAX_ROWS: usize = tumult_ingest::otap::limits::DEFAULT_MAX_ROWS_PER_TABLE;

const FUZZ_SEED: u64 = 0x6f74_6170_7072_6f62;

fn plain_id() -> Field {
    Field::new("id", DataType::UInt16, true).with_metadata(HashMap::from([(
        "encoding".to_string(),
        "plain".to_string(),
    )]))
}

/// The bytes of a fresh, zstd-compressed IPC stream holding `batch`.
fn ipc(batch: &RecordBatch) -> Result<Vec<u8>, Failure> {
    let options = IpcWriteOptions::default().try_with_compression(Some(CompressionType::ZSTD))?;
    let mut writer = StreamWriter::try_new_with_options(Vec::new(), batch.schema_ref(), options)?;
    writer.write(batch)?;
    Ok(std::mem::take(writer.get_mut()))
}

fn table(columns: Vec<(Field, ArrayRef)>) -> Result<RecordBatch, Failure> {
    let (fields, arrays): (Vec<Field>, Vec<ArrayRef>) = columns.into_iter().unzip();
    Ok(RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)?)
}

/// `rows` of the SPANS `id` column and nothing else.
fn only_ids(rows: usize) -> Result<RecordBatch, Failure> {
    table(vec![(
        plain_id(),
        Arc::new(UInt16Array::from(vec![0u16; rows])),
    )])
}

/// A SPANS table with every column a span needs. `zero_ids` leaves the
/// trace and span ids all zero.
fn spans(rows: usize, zero_ids: bool) -> Result<RecordBatch, Failure> {
    let id = |i: usize, width: usize| {
        let mut bytes = vec![0u8; width];
        if !zero_ids {
            bytes[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
        }
        bytes
    };
    let trace_ids: Vec<Vec<u8>> = (0..rows).map(|i| id(i, 16)).collect();
    let span_ids: Vec<Vec<u8>> = (0..rows).map(|i| id(i, 8)).collect();
    let time = |unit| Field::new("start_time_unix_nano", unit, false);
    table(vec![
        (
            plain_id(),
            Arc::new(UInt16Array::from_iter_values(
                (0..rows).map(|i| (i % 65_536) as u16),
            )),
        ),
        (
            time(DataType::Timestamp(TimeUnit::Nanosecond, None)),
            Arc::new(TimestampNanosecondArray::from(vec![1_000; rows])),
        ),
        (
            Field::new(
                "duration_time_unix_nano",
                DataType::Duration(TimeUnit::Nanosecond),
                false,
            ),
            Arc::new(DurationNanosecondArray::from(vec![5; rows])),
        ),
        (
            Field::new("trace_id", DataType::FixedSizeBinary(16), true),
            Arc::new(FixedSizeBinaryArray::try_from_iter(trace_ids.iter())?),
        ),
        (
            Field::new("span_id", DataType::FixedSizeBinary(8), true),
            Arc::new(FixedSizeBinaryArray::try_from_iter(span_ids.iter())?),
        ),
        (
            Field::new("name", DataType::Utf8, false),
            Arc::new(StringArray::from(vec!["probe"; rows])),
        ),
    ])
}

/// A schema message and one hand-built record batch.
fn crafted(schema: &Schema, header: &BatchHeader, body: &[u8]) -> Vec<u8> {
    let mut out = schema_message(schema);
    out.extend(batch_message(header, body));
    out
}

/// One `UInt16` column of one row whose values buffer is `buffer`.
fn one_id(buffer: (i64, i64), variadic: Option<Vec<i64>>) -> Vec<u8> {
    let header = BatchHeader {
        rows: 1,
        nodes: vec![(1, 0)],
        buffers: vec![(0, 0), buffer],
        variadic,
        ..BatchHeader::default()
    };
    crafted(
        &Schema::new(vec![plain_id()]),
        &header,
        &[7, 0, 0, 0, 0, 0, 0, 0],
    )
}

/// 100 rows of a union column whose type ids buffer is empty.
fn union() -> Result<Vec<u8>, Failure> {
    let fields = UnionFields::try_new(vec![0], vec![Field::new("a", DataType::Int32, true)])?;
    let schema = Schema::new(vec![Field::new(
        "u",
        DataType::Union(fields, UnionMode::Sparse),
        true,
    )]);
    let header = BatchHeader {
        rows: 100,
        nodes: vec![(100, 0), (100, 0)],
        buffers: vec![(0, 0), (0, 0), (0, 0)],
        ..BatchHeader::default()
    };
    Ok(crafted(&schema, &header, &[]))
}

fn records(batch_id: i64, schema_id: &str, record: Vec<u8>) -> BatchArrowRecords {
    BatchArrowRecords {
        batch_id,
        arrow_payloads: vec![ArrowPayload {
            schema_id: schema_id.into(),
            r#type: ArrowPayloadType::Spans as i32,
            record,
        }],
        headers: vec![],
    }
}

/// The batches of one case, in sending order.
fn batches(case: &str) -> Result<Vec<BatchArrowRecords>, Failure> {
    let one = |record| Ok(vec![records(77, "probe", record)]);
    match case {
        "no-batch" => Ok(Vec::new()),
        "malformed" => one(b"definitely not arrow".to_vec()),
        "oversized" => one(vec![0; OVERSIZED]),
        // The three inputs that used to reach an assertion in arrow's reader.
        "buffer-outside" => one(one_id((1 << 40, 8), None)),
        "buffer-negative" => one(one_id((0, -1), None)),
        "variadic" => one(one_id((0, 2), Some(vec![1]))),
        "union" => one(union()?),
        // A few hundred bytes that declare the most rows a table may have.
        "null-rows" => {
            let nulls = table(vec![(
                Field::new("id", DataType::Null, true),
                Arc::new(NullArray::new(MAX_ROWS)),
            )])?;
            one(ipc(&nulls)?)
        }
        "no-ids" => one(ipc(&only_ids(MAX_ROWS)?)?),
        "zero-ids" => one(ipc(&spans(4, true)?)?),
        "forged-schema-id" => Ok(vec![records(
            77,
            "probe\n2026-01-01T00:00:00Z ERROR forged line\r\u{1b}[31m",
            schema_message(&Schema::new(vec![plain_id()])),
        )]),
        // More refused batches than a stream may send.
        "rejected-flood" => (0..12)
            .map(|i| Ok(records(100 + i, &format!("flood-{i}"), ipc(&only_ids(1)?)?)))
            .collect(),
        other => Err(format!("unknown case {other:?}").into()),
    }
}

/// How one stream went.
#[derive(Default)]
struct Exchange {
    /// `(batch id, status name, message)` per status.
    acks: Vec<(i64, &'static str, String)>,
    /// `clean`, or the gRPC code the stream closed with, and its message.
    closed: (String, String),
    /// Set instead of everything else when the stream was refused.
    refused: Option<(String, String)>,
}

async fn exchange(
    client: &mut ArrowTracesServiceClient<Channel>,
    token: Option<&str>,
    batches: Vec<BatchArrowRecords>,
) -> Result<Exchange, Failure> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut request = tonic::Request::new(
        tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx),
    );
    if let Some(token) = token {
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
    }
    let mut out = Exchange::default();
    let mut acks = match client.arrow_traces(request).await {
        Ok(response) => response.into_inner(),
        Err(status) => {
            out.refused = Some((format!("{:?}", status.code()), status.message().into()));
            return Ok(out);
        }
    };
    tokio::spawn(async move {
        for batch in batches {
            // The server may already have closed the stream; its answer is
            // what gets reported.
            if tx.send(batch).await.is_err() {
                return;
            }
        }
    });
    loop {
        match acks.message().await {
            Ok(Some(ack)) => {
                let code =
                    StatusCode::try_from(ack.status_code).map_or("UNKNOWN", |c| c.as_str_name());
                out.acks.push((ack.batch_id, code, ack.status_message));
            }
            Ok(None) => {
                out.closed = ("clean".into(), String::new());
                return Ok(out);
            }
            Err(status) => {
                out.closed = (format!("{:?}", status.code()), status.message().into());
                return Ok(out);
            }
        }
    }
}

fn print(exchange: &Exchange) {
    if let Some((code, message)) = &exchange.refused {
        println!("open: {code}: {message}");
        return;
    }
    for (batch_id, code, message) in &exchange.acks {
        println!("ack: {batch_id} {code} {message}");
    }
    match &exchange.closed {
        (code, _) if code == "clean" => println!("closed: clean"),
        (code, message) => println!("closed: {code}: {message}"),
    }
}

/// Damage `record` one to three times, half of them inside the flatbuffer
/// metadata of one of its messages.
fn damage(mutator: &mut Mutator, record: &mut Vec<u8>) {
    for _ in 0..=mutator.below(3) {
        let ranges = metadata_ranges(record);
        if ranges.is_empty() || mutator.below(2) == 0 {
            mutator.mutate(record);
            continue;
        }
        let (start, end) = ranges[mutator.below(ranges.len())];
        mutator.mutate_within(record, start, end);
    }
}

/// Send `cases` damaged batches, one per stream; every one must be answered
/// and every stream must end cleanly or with a refusal.
async fn fuzz(
    client: &mut ArrowTracesServiceClient<Channel>,
    token: Option<&str>,
    cases: u64,
) -> Result<(), Failure> {
    let pristine = ipc(&spans(16, false)?)?;
    let mut tally: BTreeMap<String, u64> = BTreeMap::new();
    for case in 0..cases {
        let mut mutator = Mutator::new(FUZZ_SEED.wrapping_add(case));
        let mut record = pristine.clone();
        damage(&mut mutator, &mut record);
        let batch_id = i64::try_from(case)?;
        let got = exchange(client, token, vec![records(batch_id, "fuzz", record)]).await?;
        if let Some((code, message)) = &got.refused {
            return Err(format!("case {case}: stream refused: {code}: {message}").into());
        }
        let [(acked, status, _)] = got.acks.as_slice() else {
            return Err(format!(
                "case {case}: {} statuses for one batch, closed {:?}",
                got.acks.len(),
                got.closed
            )
            .into());
        };
        if *acked != batch_id {
            return Err(format!("case {case}: status for batch {acked}").into());
        }
        let closed = got.closed.0.as_str();
        if !matches!(closed, "clean" | "InvalidArgument" | "ResourceExhausted") {
            return Err(format!("case {case}: stream closed {:?}", got.closed).into());
        }
        *tally.entry(format!("{status}/{closed}")).or_default() += 1;
    }
    let parts: Vec<String> = tally.iter().map(|(k, n)| format!("{k}={n}")).collect();
    println!("fuzz: {cases} cases answered: {}", parts.join(" "));
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Failure> {
    let mut args = std::env::args().skip(1);
    let (Some(endpoint), Some(case)) = (args.next(), args.next()) else {
        return Err("usage: otap_probe <endpoint> <case> [bearer-token]".into());
    };
    let token = args.next();
    let mut client = ArrowTracesServiceClient::connect(endpoint)
        .await?
        // Let the request through so the server is the one to refuse it.
        .max_encoding_message_size(2 * OVERSIZED);
    if let Some(cases) = case.strip_prefix("fuzz:") {
        return fuzz(&mut client, token.as_deref(), cases.parse()?).await;
    }
    let got = exchange(&mut client, token.as_deref(), batches(&case)?).await?;
    print(&got);
    Ok(())
}
