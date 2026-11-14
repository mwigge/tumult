//! Sends one hand-built entity event to a running daemon over OTLP/gRPC and
//! prints what the daemon answered. Used by
//! `scripts/proof/entity-events/proof.sh` for the events a Collector on one
//! quiet host never produces: a changed state, a delete, a description with
//! labels and annotations, a record over the size limit.
//!
//! ```text
//! entity_probe <http://host:port> <case> <pod uid> [bearer-token]
//! ```
//!
//! Every case is one export of one `k8s.pod` event in the specification
//! wire form, identified by `k8s.pod.uid`, stamped with the time it is sent
//! and compressed with gzip, as a Collector sends it:
//!
//! * `state:<phase>`: a state whose description is the pod's name and
//!   `k8s.pod.phase` = `<phase>`.
//! * `delete`: a delete with the reason `probe`.
//! * `labelled`: a state whose description also carries two labels, an
//!   annotation holding a manifest, a container label and a nested map of
//!   labels.
//! * `oversized`: a state whose description is larger than the daemon
//!   stores.
//!
//! Output: `sent: <case>, rejected <n>` when the export was answered, the
//! gRPC status and a non-zero exit when it was refused.

#![allow(clippy::print_stdout)] // an example: what the server answered is its output

use std::time::{SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

type Failure = Box<dyn std::error::Error>;

/// The report interval of the probe's states, seconds. Long enough that an
/// entity does not expire while a proof looks at it.
const INTERVAL_S: i64 = 600;

/// One byte more than a description may hold
/// (`tumult_otlp::entity::MAX_STATE_BYTES`).
const OVERSIZED: usize = 64 * 1024 + 1;

/// What a `kubectl apply` leaves on an object: the whole manifest, with the
/// values of its environment variables.
const MANIFEST: &str =
    r#"{"spec":{"containers":[{"env":[{"name":"DB_PASSWORD","value":"proof-secret"}]}]}}"#;

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        ..KeyValue::default()
    }
}

fn text(key: &str, value: &str) -> KeyValue {
    kv(key, Value::StringValue(value.into()))
}

fn map(values: Vec<KeyValue>) -> Value {
    Value::KvlistValue(KeyValueList { values })
}

fn now_ns() -> Result<u64, Failure> {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(u64::try_from(since_epoch.as_nanos())?)
}

fn event(name: &str, uid: &str, mut attributes: Vec<KeyValue>) -> Result<LogRecord, Failure> {
    attributes.push(text("entity.type", "k8s.pod"));
    attributes.push(kv("entity.id", map(vec![text("k8s.pod.uid", uid)])));
    Ok(LogRecord {
        time_unix_nano: now_ns()?,
        event_name: name.into(),
        attributes,
        ..LogRecord::default()
    })
}

fn state(uid: &str, mut description: Vec<KeyValue>) -> Result<LogRecord, Failure> {
    description.push(text("k8s.pod.name", "probe"));
    event(
        "entity.state",
        uid,
        vec![
            kv("entity.description", map(description)),
            kv("entity.report.interval", Value::IntValue(INTERVAL_S)),
        ],
    )
}

fn labelled(uid: &str) -> Result<LogRecord, Failure> {
    state(
        uid,
        vec![
            text("k8s.pod.phase", "Running"),
            text("k8s.pod.label.app", "checkout"),
            text("k8s.pod.label.team", "payments"),
            text(
                "k8s.pod.annotation.kubectl.kubernetes.io/last-applied-configuration",
                MANIFEST,
            ),
            text("container.label.maintainer", "someone@example.com"),
            kv("labels", map(vec![text("app", "checkout")])),
        ],
    )
}

fn record(case: &str, uid: &str) -> Result<LogRecord, Failure> {
    if let Some(phase) = case.strip_prefix("state:") {
        return state(uid, vec![text("k8s.pod.phase", phase)]);
    }
    match case {
        "delete" => event(
            "entity.delete",
            uid,
            vec![text("entity.delete.reason", "probe")],
        ),
        "labelled" => labelled(uid),
        "oversized" => state(uid, vec![text("host.name", &"x".repeat(OVERSIZED))]),
        other => Err(format!("unknown case '{other}'").into()),
    }
}

/// One export holding `record`, under the scope a Collector marks its
/// entity events with.
fn export(record: LogRecord) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    attributes: vec![kv("otel.entity.event_as_log", Value::BoolValue(true))],
                    ..InstrumentationScope::default()
                }),
                log_records: vec![record],
                ..ScopeLogs::default()
            }],
            ..ResourceLogs::default()
        }],
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(endpoint), Some(case), Some(uid)) = (args.first(), args.get(1), args.get(2)) else {
        return Err(
            "usage: entity_probe <http://host:port> <case> <pod uid> [bearer-token]".into(),
        );
    };
    let mut request = tonic::Request::new(export(record(case, uid)?));
    if let Some(token) = args.get(3) {
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
    }
    let channel = Channel::from_shared(endpoint.clone())?.connect().await?;
    let mut client = LogsServiceClient::new(channel).send_compressed(CompressionEncoding::Gzip);
    let answer = client.export(request).await?.into_inner();
    let rejected = answer
        .partial_success
        .map_or(0, |partial| partial.rejected_log_records);
    println!("sent: {case}, rejected {rejected}");
    Ok(())
}
