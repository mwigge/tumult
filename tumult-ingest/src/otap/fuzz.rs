//! Mutation fuzzing of the decoder, as an ordinary test.
//!
//! Valid encoder output for each signal is damaged a few thousand times
//! with a fixed seed (bit flips, edge-value integers over flatbuffer fields,
//! truncation, cut and spliced ranges, and batch headers rewritten after a
//! valid parse so they still pass the flatbuffer verifier) and decoded. Whatever the damage,
//! decoding must come back with a batch or an error: the daemon aborts on
//! panic. A failing case is replayed by its number.

use std::panic::{catch_unwind, AssertUnwindSafe};

use super::craft::{metadata_ranges, Mutator};
use super::limits::OtapLimits;
use super::proto::BatchArrowRecords;
use super::test_encoder::Encoder;
use super::tests::{logs_fixture, metrics_fixture, traces_fixture};
use super::{to_logs_request, to_metrics_request, to_trace_request, OtapError, StreamConsumer};

const CASES: u64 = 6000;
const SEED: u64 = 0x6f74_6170_2d34;

#[derive(Clone, Copy, Debug)]
enum Signal {
    Traces,
    Logs,
    Metrics,
}

/// A stream's first batch (schemas and dictionaries) and its second.
struct Sample {
    signal: Signal,
    first: BatchArrowRecords,
    second: BatchArrowRecords,
}

fn samples() -> Vec<Sample> {
    let mut out = Vec::new();
    for compress in [false, true] {
        let mut encoder = Encoder::new(compress);
        let traces = traces_fixture();
        out.push(Sample {
            signal: Signal::Traces,
            first: encoder.traces(&traces),
            second: encoder.traces(&traces),
        });
        let mut encoder = Encoder::new(compress);
        let logs = logs_fixture();
        out.push(Sample {
            signal: Signal::Logs,
            first: encoder.logs(&logs),
            second: encoder.logs(&logs),
        });
        let mut encoder = Encoder::new(compress);
        let metrics = metrics_fixture();
        out.push(Sample {
            signal: Signal::Metrics,
            first: encoder.metrics(&metrics),
            second: encoder.metrics(&metrics),
        });
    }
    out
}

fn limits() -> OtapLimits {
    OtapLimits {
        max_rows_per_table: 10_000,
        max_decoded_bytes_per_batch: 1024 * 1024,
        max_stream_retained_bytes: 1024 * 1024,
        ..OtapLimits::default()
    }
}

fn convert(
    signal: Signal,
    consumer: &mut StreamConsumer,
    records: BatchArrowRecords,
) -> Result<(), OtapError> {
    let batch = consumer.consume(records)?;
    match signal {
        Signal::Traces => to_trace_request(&batch).map(drop),
        Signal::Logs => to_logs_request(&batch).map(drop),
        Signal::Metrics => to_metrics_request(&batch).map(drop),
    }
}

/// Damage one payload of `records`. A third of the cases rewrite what a
/// batch header declares and leave a well-formed message, so the checks
/// behind the flatbuffer verifier are what meets the damage. The others
/// damage bytes: half the time anywhere, half the time inside the
/// flatbuffer metadata of one of the IPC messages.
fn damage(mutator: &mut Mutator, records: &mut BatchArrowRecords) {
    let count = records.arrow_payloads.len();
    let payload = &mut records.arrow_payloads[mutator.below(count)];
    if mutator.below(3) == 0 && mutator.mutate_structure(&mut payload.record) {
        return;
    }
    for _ in 0..=mutator.below(3) {
        let ranges = metadata_ranges(&payload.record);
        if ranges.is_empty() || mutator.below(2) == 0 {
            mutator.mutate(&mut payload.record);
            continue;
        }
        let (start, end) = ranges[mutator.below(ranges.len())];
        mutator.mutate_within(&mut payload.record, start, end);
    }
    match mutator.below(40) {
        0 => payload.r#type = i32::try_from(mutator.below(64)).unwrap_or(0),
        1 => payload.schema_id.push_str("\n\u{1b}x"),
        _ => {}
    }
}

/// Whether every IPC message of every payload still frames: the damage
/// got past the framing and the flatbuffer verifier, so what refuses the
/// batch (if anything does) is a check of the decoder itself.
fn frames(records: &BatchArrowRecords) -> bool {
    records.arrow_payloads.iter().all(|payload| {
        let mut rest = payload.record.as_slice();
        while !rest.is_empty() {
            let Ok(framed) = super::ipc::frame("fuzz", rest) else {
                return false;
            };
            rest = framed.rest;
        }
        true
    })
}

/// How one fuzz case ended.
struct Outcome {
    /// See [`frames`].
    framed: bool,
    decoded: bool,
}

/// One fuzz case; `Err` is a defect.
fn run_case(samples: &[Sample], case: u64) -> Result<Outcome, String> {
    let mut mutator = Mutator::new(SEED.wrapping_add(case.wrapping_mul(0x9e37_79b9_7f4a_7c15)));
    let sample = &samples[mutator.below(samples.len())];
    let continuation = mutator.below(2) == 1;
    let mut consumer = StreamConsumer::new(limits());
    let mut records = sample.first.clone();
    if continuation {
        convert(sample.signal, &mut consumer, records)
            .map_err(|e| format!("pristine first batch refused: {e}"))?;
        records = sample.second.clone();
    }
    damage(&mut mutator, &mut records);
    let framed = frames(&records);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        convert(sample.signal, &mut consumer, records)
    }));
    match outcome {
        Err(_) => Err(format!(
            "panicked ({:?}, continuation {continuation})",
            sample.signal
        )),
        Ok(Ok(())) => Ok(Outcome {
            framed,
            decoded: true,
        }),
        Ok(Err(e)) => {
            let text = e.to_string();
            if text.len() > 512 || text.chars().any(char::is_control) {
                return Err(format!("error text is not tame: {text:?}"));
            }
            Ok(Outcome {
                framed,
                decoded: false,
            })
        }
    }
}

#[test]
fn mutated_payloads_never_panic() {
    let samples = samples();
    let (mut decoded, mut refused) = (0u64, 0u64);
    // Cases the decoder's own checks refused, not the framing.
    let mut reached = 0u64;
    let mut defects = Vec::new();
    for case in 0..CASES {
        match run_case(&samples, case) {
            Ok(outcome) => {
                decoded += u64::from(outcome.decoded);
                refused += u64::from(!outcome.decoded);
                reached += u64::from(outcome.framed && !outcome.decoded);
            }
            Err(defect) => defects.push(format!("case {case}: {defect}")),
        }
    }
    assert!(
        defects.is_empty(),
        "{} of {CASES} cases failed, first: {:#?}",
        defects.len(),
        &defects[..defects.len().min(5)]
    );
    assert!(refused > CASES / 4, "damage is noticed: {refused} refused");
    assert!(decoded > 0, "harmless damage still decodes: {decoded}");
    assert!(
        reached > CASES / 3,
        "the decoder's own checks are what the damage meets: {reached}"
    );
    eprintln!(
        "fuzz: {CASES} cases, {decoded} decoded, {refused} refused ({reached} of them by the \
         decoder's own checks, past framing and the flatbuffer verifier), 0 panics"
    );
}

#[test]
fn every_sample_decodes_before_it_is_damaged() {
    for sample in samples() {
        let mut consumer = StreamConsumer::new(limits());
        convert(sample.signal, &mut consumer, sample.first).unwrap();
        convert(sample.signal, &mut consumer, sample.second).unwrap();
    }
}
