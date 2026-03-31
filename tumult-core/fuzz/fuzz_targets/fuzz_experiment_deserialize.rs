//! Fuzz target: deserialize arbitrary bytes as a `tumult_core::types::Experiment`.
//!
//! The goal is to verify that `toon_format::decode_default::<Experiment>` never
//! panics (only returns `Err`) for any input, and that a round-trip on a
//! successfully decoded value is stable.
//!
//! Run with:
//!   cargo fuzz run fuzz_experiment_deserialize

#![no_main]

use libfuzzer_sys::fuzz_target;
use tumult_core::types::Experiment;

fuzz_target!(|data: &[u8]| {
    // Reject non-UTF-8 input early; the decoder requires a valid string.
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    // Attempt to deserialize arbitrary bytes as an Experiment.
    // This must never panic; a decoding error is acceptable.
    let Ok(experiment) = toon_format::decode_default::<Experiment>(input) else {
        return;
    };

    // Round-trip: encode the decoded experiment and decode it again.
    // The result must be identical to the first decoded value.
    let encoded = toon_format::encode_default(&experiment)
        .expect("re-encoding a decoded Experiment must not fail");
    let round_tripped = toon_format::decode_default::<Experiment>(&encoded)
        .expect("decoding a freshly encoded Experiment must not fail");

    assert_eq!(
        format!("{experiment:?}"),
        format!("{round_tripped:?}"),
        "round-trip must produce an identical Experiment"
    );
});
