//! Wire format of the tolerance method.
//!
//! `Method` is a tagged enum that is stored with a baseline and read back by
//! other processes, so its JSON encoding is pinned here. The other data-model
//! types are plain derives with no encoding decisions of ours.

use tumult_baseline::tolerance::Method;

#[test]
fn method_encodes_as_an_externally_tagged_variant() {
    let cases = [
        (
            Method::Static {
                lower: 10.0,
                upper: 90.0,
            },
            r#"{"Static":{"lower":10.0,"upper":90.0}}"#,
        ),
        (
            Method::Percentile {
                percentile: 95.0,
                multiplier: 1.2,
            },
            r#"{"Percentile":{"percentile":95.0,"multiplier":1.2}}"#,
        ),
        (
            Method::MeanStddev { sigma: 2.0 },
            r#"{"MeanStddev":{"sigma":2.0}}"#,
        ),
        (Method::Iqr, r#""Iqr""#),
    ];
    for (method, json) in cases {
        assert_eq!(serde_json::to_string(&method).unwrap(), json);
        assert_eq!(serde_json::from_str::<Method>(json).unwrap(), method);
    }
}
