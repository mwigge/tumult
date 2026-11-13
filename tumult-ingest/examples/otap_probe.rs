//! Opens one OTel-Arrow traces stream against a running daemon, sends one
//! hostile batch, and prints exactly what the server answered. Used by
//! `scripts/proof/otap/proof.sh` for the cases a well-behaved Collector
//! never produces.
//!
//! ```text
//! otap_probe <http://host:port> <no-batch|malformed|oversized> [bearer-token]
//! ```
//!
//! Output, one line each: `open: <code>: <message>` when the stream is
//! refused, `ack: <batch id> <status> <message>` per batch status, then
//! `closed: <code>: <message>` (or `closed: clean`).

use tumult_ingest::otap::proto::arrow_traces_service_client::ArrowTracesServiceClient;
use tumult_ingest::otap::proto::{ArrowPayload, ArrowPayloadType, BatchArrowRecords, StatusCode};

/// One byte more than the daemon's default `KRONIKA_OTAP_MAX_MESSAGE_BYTES`.
const OVERSIZED: usize = tumult_ingest::otap::limits::DEFAULT_MAX_MESSAGE_BYTES + 1;

fn batch(case: &str) -> Result<Option<BatchArrowRecords>, String> {
    let record = match case {
        "no-batch" => return Ok(None),
        "malformed" => b"definitely not arrow".to_vec(),
        "oversized" => vec![0; OVERSIZED],
        other => return Err(format!("unknown case {other:?}")),
    };
    Ok(Some(BatchArrowRecords {
        batch_id: 77,
        arrow_payloads: vec![ArrowPayload {
            schema_id: "probe".into(),
            r#type: ArrowPayloadType::Spans as i32,
            record,
        }],
        headers: vec![],
    }))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(endpoint), Some(case)) = (args.next(), args.next()) else {
        return Err("usage: otap_probe <endpoint> <case> [bearer-token]".into());
    };
    let token = args.next();
    let batch = batch(&case)?;

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut request = tonic::Request::new(
        tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx),
    );
    if let Some(token) = token {
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
    }
    let mut client = ArrowTracesServiceClient::connect(endpoint)
        .await?
        // Let the request through so the server is the one to refuse it.
        .max_encoding_message_size(2 * OVERSIZED);
    let mut acks = match client.arrow_traces(request).await {
        Ok(response) => response.into_inner(),
        Err(status) => {
            println!("open: {:?}: {}", status.code(), status.message());
            return Ok(());
        }
    };
    match batch {
        // The server may already have closed the stream; its answer is
        // what gets printed below.
        Some(batch) => drop(tx.send(batch).await),
        None => drop(tx),
    }
    loop {
        match acks.message().await {
            Ok(Some(ack)) => {
                let code =
                    StatusCode::try_from(ack.status_code).map_or("UNKNOWN", |c| c.as_str_name());
                println!("ack: {} {code} {}", ack.batch_id, ack.status_message);
            }
            Ok(None) => {
                println!("closed: clean");
                return Ok(());
            }
            Err(status) => {
                println!("closed: {:?}: {}", status.code(), status.message());
                return Ok(());
            }
        }
    }
}
