//! One client against the others: what a single connection or peer can
//! take of the limits all clients share.
//!
//! The streams here are served by the receiver's own stream task
//! (`service::respond`), each with the place a real stream of that socket
//! address would claim, and a writer on a private lake.

use std::sync::Arc;
use std::time::Duration;

use arrow::array::{ArrayRef, DictionaryArray, RecordBatch, StringArray, UInt16Array};
use arrow::datatypes::{Field, Schema};
use arrow::ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow::ipc::CompressionType;
use tokio::sync::mpsc;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::tokio_stream::StreamExt;

use super::admission::Shares;
use super::hostile_ipc::dictionary_table;
use super::limits::OtapLimits;
use super::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords, BatchStatus, StatusCode};
use super::service::{respond, Guards, StatusStream, StreamContext};
use super::{OtapBatch, OtapError};
use crate::writer::{Batch, IngestWriter};

const MIB: usize = 1024 * 1024;

fn no_rows(_: &OtapBatch) -> Result<Batch, OtapError> {
    Ok(Batch::Spans(Vec::new()))
}

/// A receiver's shared state, without the gRPC server around it.
struct Receiver {
    limits: OtapLimits,
    shares: Arc<Shares>,
    guards: Arc<Guards>,
    decodes: Arc<tokio::sync::Semaphore>,
    ingest: IngestWriter,
    _dir: tempfile::TempDir,
    _store: tumult_lake::Store,
}

/// What a stream answered to its one batch. The stream stays open for as
/// long as this is held; dropping it is the client closing the stream.
struct Answer {
    ack: BatchStatus,
    /// The status the stream closed with, when the batch was refused.
    closed: Option<tonic::Status>,
    _inbound: mpsc::Sender<Result<BatchArrowRecords, tonic::Status>>,
    _stream: StatusStream,
}

impl Answer {
    fn code(&self) -> StatusCode {
        StatusCode::try_from(self.ack.status_code).unwrap()
    }
}

impl Receiver {
    fn new(limits: OtapLimits) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        Self {
            limits,
            shares: Shares::new(&limits),
            guards: Guards::new(&limits),
            decodes: Arc::new(tokio::sync::Semaphore::new(limits.max_concurrent_decodes)),
            ingest,
            _dir: dir,
            _store: store,
        }
    }

    /// Open a stream on the connection from `addr`, send `batch` on it and
    /// keep it open.
    async fn send(&self, addr: &str, batch: BatchArrowRecords) -> Answer {
        let slot = self.shares.claim(addr.parse().unwrap()).ok().unwrap();
        let context = StreamContext {
            ingest: self.ingest.clone(),
            limits: self.limits,
            decodes: self.decodes.clone(),
            signal: "traces",
            convert: no_rows,
            guards: self.guards.clone(),
            slot: Some(slot),
        };
        let (inbound, batches) = mpsc::channel(1);
        inbound.send(Ok(batch)).await.unwrap();
        let mut stream = respond(ReceiverStream::new(batches), context, None);
        // A guard, not a wait: an answer that never comes fails the test.
        let answered = tokio::time::timeout(Duration::from_secs(120), async {
            let ack = stream.next().await.unwrap().expect("a batch status");
            let closed = if ack.status_code == StatusCode::Ok as i32 {
                None
            } else {
                Some(stream.next().await.unwrap().unwrap_err())
            };
            (ack, closed)
        });
        let (ack, closed) = answered.await.expect("the batch was never answered");
        Answer {
            ack,
            closed,
            _inbound: inbound,
            _stream: stream,
        }
    }
}

fn spans(schema_id: &str, record: Vec<u8>) -> BatchArrowRecords {
    BatchArrowRecords {
        batch_id: 7,
        arrow_payloads: vec![ArrowPayload {
            schema_id: schema_id.into(),
            r#type: ArrowPayloadType::Spans as i32,
            record,
        }],
        headers: Vec::new(),
    }
}

/// A batch whose one dictionary makes the stream keep about `bytes`, sent
/// zstd-compressed as the exporter does: a few kilobytes on the wire.
fn pinning(bytes: usize) -> BatchArrowRecords {
    let values = StringArray::from_iter_values(["a".to_owned(), "b".repeat(bytes)]);
    let keys = UInt16Array::from_iter_values([0]);
    let column: ArrayRef = Arc::new(DictionaryArray::new(keys, Arc::new(values)));
    let field = Field::new("name", column.data_type().clone(), true);
    let table = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![column]).unwrap();
    let options = IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::ZSTD))
        .unwrap();
    let mut writer =
        StreamWriter::try_new_with_options(Vec::new(), table.schema_ref(), options).unwrap();
    writer.write(&table).unwrap();
    let record = std::mem::take(writer.get_mut());
    assert!(
        record.len() < 64 * 1024,
        "{} bytes on the wire",
        record.len()
    );
    spans("pin", record)
}

/// About 70 KiB kept per stream.
fn small() -> BatchArrowRecords {
    let table = dictionary_table(1, 1000, "a");
    let mut writer = StreamWriter::try_new(Vec::new(), table.schema_ref()).unwrap();
    writer.write(&table).unwrap();
    spans("small", std::mem::take(writer.get_mut()))
}

/// The refusal a client over its own share gets: not retried, and the
/// stream ends.
fn assert_own_share(answer: &Answer, what: &str) {
    assert_eq!(
        answer.code(),
        StatusCode::InvalidArgument,
        "{}",
        answer.ack.status_message
    );
    assert!(
        answer.ack.status_message.contains(what),
        "{}",
        answer.ack.status_message
    );
    let closed = answer.closed.as_ref().unwrap();
    assert_eq!(closed.code(), tonic::Code::ResourceExhausted, "{closed}");
}

/// Under the default limits, eight streams of one connection each keep a
/// dictionary just under the per-stream limit: 248 MiB of the 256 MiB all
/// streams share, for a few kilobytes sent. The connection gets its share
/// of the pool and no more, so another client is still served.
#[tokio::test]
async fn one_connection_cannot_pin_the_pool_all_streams_share() {
    let receiver = Receiver::new(OtapLimits::default());
    let pin = pinning(31 * MIB);
    let mut answers = Vec::new();
    for _ in 0..8 {
        answers.push(receiver.send("192.0.2.1:40000", pin.clone()).await);
    }
    assert_eq!(answers[0].code(), StatusCode::Ok, "the first one fits");
    for refused in &answers[1..] {
        assert_own_share(refused, "retained bytes per connection");
    }
    let held = receiver.guards.pool().used();
    assert!(
        (31 * MIB..=32 * MIB).contains(&held),
        "the connection holds {held} bytes"
    );

    let other = receiver.send("192.0.2.2:40000", pinning(10 * MIB)).await;
    assert_eq!(
        other.code(),
        StatusCode::Ok,
        "another client's first batch: {}",
        other.ack.status_message
    );

    drop((answers, other));
    let emptied = tokio::time::timeout(Duration::from_secs(120), async {
        while receiver.guards.pool().used() > 0 {
            tokio::task::yield_now().await;
        }
    });
    emptied.await.expect("closed streams still hold bytes");
}

#[tokio::test]
async fn a_peer_with_many_connections_gets_one_share() {
    let receiver = Receiver::new(OtapLimits {
        max_connection_retained_bytes: 100 * 1024,
        max_peer_retained_bytes: 150 * 1024,
        max_retained_bytes: 220 * 1024,
        ..OtapLimits::default()
    });
    let first = receiver.send("192.0.2.1:40000", small()).await;
    assert_eq!(first.code(), StatusCode::Ok);
    let kept = receiver.guards.pool().used();
    assert!((56 * 1024..73 * 1024).contains(&kept), "{kept} bytes kept");

    // The same connection again: its own share.
    let again = receiver.send("192.0.2.1:40000", small()).await;
    assert_own_share(&again, "retained bytes per connection");
    // A second connection of the peer has a share of its own ...
    let second = receiver.send("192.0.2.1:40001", small()).await;
    assert_eq!(second.code(), StatusCode::Ok);
    // ... but a third one is over what the peer may hold.
    let third = receiver.send("192.0.2.1:40002", small()).await;
    assert_own_share(&third, "retained bytes per peer");
    assert_eq!(receiver.guards.pool().used(), 2 * kept);

    // Other peers are served until the pool itself is full, and only that
    // is a shortage worth a retry.
    let other = receiver.send("192.0.2.2:40000", small()).await;
    assert_eq!(other.code(), StatusCode::Ok);
    let late = receiver.send("192.0.2.3:40000", small()).await;
    assert_eq!(
        late.code(),
        StatusCode::Unavailable,
        "{}",
        late.ack.status_message
    );
    assert!(
        late.ack
            .status_message
            .contains("retained bytes across streams"),
        "{}",
        late.ack.status_message
    );
}
