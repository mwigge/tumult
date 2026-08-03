//! Tests for `tumult templates` and `tumult new` (template instantiation,
//! overrides, output handling, and the non-TTY interactive guard).

use super::super::*;
use tempfile::TempDir;

#[test]
fn templates_lists_every_curated_starter() {
    // Rendering must not fail; the catalog it prints is the one `new --from`
    // instantiates from.
    cmd_templates().unwrap();
}

#[test]
fn new_from_template_writes_validated_experiment() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("cpu.toon");

    cmd_new(Some("cpu-stress"), &[], Some(&out)).unwrap();

    let content = std::fs::read_to_string(&out).unwrap();
    // The written file is a parseable, valid experiment.
    let experiment = tumult_core::engine::parse_experiment(&content).unwrap();
    assert!(!experiment.method.is_empty());
}

#[test]
fn new_from_template_applies_set_overrides() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("cpu.toon");

    cmd_new(
        Some("cpu-stress"),
        &[
            "target=staging-host".to_string(),
            "duration=30s".to_string(),
        ],
        Some(&out),
    )
    .unwrap();

    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("staging-host"), "{content}");
    assert!(content.contains("30s"), "{content}");
}

#[test]
fn new_unknown_template_errors_listing_available() {
    let dir = TempDir::new().unwrap();
    let err = cmd_new(
        Some("no-such-template"),
        &[],
        Some(&dir.path().join("x.toon")),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown template 'no-such-template'"),
        "{err}"
    );
    assert!(err.to_string().contains("cpu-stress"), "{err}");
}

#[test]
fn new_rejects_override_without_equals() {
    let dir = TempDir::new().unwrap();
    let err = cmd_new(
        Some("cpu-stress"),
        &["not-a-key-value".to_string()],
        Some(&dir.path().join("x.toon")),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not-a-key-value"), "{err}");
}

#[test]
fn new_refuses_to_clobber_existing_output() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("existing.toon");
    std::fs::write(&out, "precious").unwrap();

    let err = cmd_new(Some("cpu-stress"), &[], Some(&out)).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    // The pre-existing file is untouched.
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "precious");
}
