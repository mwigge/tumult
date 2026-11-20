//! A key written twice in one object is an error.
//!
//! The TOON decoder keeps the last of two equal keys and says nothing, so a
//! block left behind by an edit silently replaces the one above it. These
//! tests pin what `parse_experiment` refuses and, as important, what it must
//! keep accepting.

use tumult_core::engine::parse_experiment;
use tumult_core::types::Experiment;

fn refusal(toon: &str) -> String {
    match parse_experiment(toon) {
        Ok(experiment) => panic!("parsed as '{}':\n{toon}", experiment.title),
        Err(e) => e.to_string(),
    }
}

fn accepted(toon: &str) -> Experiment {
    parse_experiment(toon).unwrap_or_else(|e| panic!("{e}:\n{toon}"))
}

#[test]
fn a_top_level_key_written_twice_is_refused_with_both_lines() {
    let error = refusal("title: first\ndescription: d\ntitle: second\n");

    assert!(
        error.contains("key 'title' is written twice in one object (lines 1 and 3)"),
        "{error}"
    );
}

#[test]
fn a_block_written_twice_in_a_step_is_refused() {
    let error = refusal(
        r#"title: t

method[1]:
  - name: still-up
    activity_type: probe
    provider:
      type: process
      path: echo
    tolerance:
      type: regex
      pattern: "true"
    tolerance:
      type: regex
      pattern: "alive"
"#,
    );

    assert!(
        error.contains("key 'tolerance' is written twice in one object (lines 9 and 12)"),
        "{error}"
    );
}

#[test]
fn the_first_key_of_a_list_item_counts() {
    let error = refusal(
        r"title: t

method[1]:
  - name: one
    activity_type: action
    name: two
    provider:
      type: process
      path: echo
",
    );

    assert!(error.contains("key 'name'"), "{error}");
    assert!(error.contains("(lines 4 and 6)"), "{error}");
}

#[test]
fn a_key_written_twice_in_a_nested_object_is_refused() {
    let error = refusal(
        r"title: t

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
      path: ls
",
    );

    assert!(error.contains("key 'path'"), "{error}");
    assert!(error.contains("(lines 8 and 9)"), "{error}");
}

#[test]
fn a_key_of_a_free_form_map_written_twice_is_refused() {
    let error = refusal(
        r"title: t

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
      env:
        REGION: eu
        ZONE: a
        REGION: us
",
    );

    assert!(error.contains("key 'REGION'"), "{error}");
}

#[test]
fn a_quoted_and_a_plain_spelling_are_the_same_key() {
    let error = refusal("title: first\n\"title\": second\n");

    assert!(error.contains("key 'title'"), "{error}");
}

#[test]
fn a_key_after_a_table_still_belongs_to_its_object() {
    let error = refusal(
        r"title: t
nfr[2]{id,kind,metric,operator,threshold}:
  a,availability,during.availability,gte,0.9
  b,availability,post.availability,gte,0.9
title: again

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
",
    );

    assert!(error.contains("key 'title'"), "{error}");
    assert!(error.contains("(lines 1 and 5)"), "{error}");
}

#[test]
fn the_same_key_in_two_list_items_is_fine() {
    let experiment = accepted(
        r"title: t

method[2]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
  - name: two
    activity_type: action
    provider:
      type: process
      path: echo
",
    );

    assert_eq!(experiment.method.len(), 2);
}

#[test]
fn the_same_key_in_sibling_objects_and_at_other_depths_is_fine() {
    let experiment = accepted(
        r"title: t
target:
  system: checkout
  environment: demo

steady_state_hypothesis:
  title: steady
  probes[1]:
    - name: health
      activity_type: probe
      provider:
        type: process
        path: echo
      tolerance:
        type: regex
        pattern: ok

method[1]:
  - name: health
    activity_type: probe
    provider:
      type: process
      path: echo
    tolerance:
      type: regex
      pattern: ok

rollbacks[1]:
  - name: health
    activity_type: action
    provider:
      type: process
      path: echo
",
    );

    assert_eq!(experiment.rollbacks.len(), 1);
}

#[test]
fn rows_of_a_table_are_not_keys() {
    let experiment = accepted(
        r#"title: t
nfr[2]{id,kind,metric,operator,threshold,description}:
  a,availability,during.availability,gte,0.9,"note: same"
  b,availability,during.availability,gte,0.9,"note: same"

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
"#,
    );

    assert_eq!(experiment.nfr.len(), 2);
}

#[test]
fn list_values_that_look_alike_are_not_keys() {
    let experiment = accepted(
        r#"title: t
tags[2]:
  - "team: payments"
  - "team: payments"

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]:
        - "echo a: b"
        - "echo a: b"
"#,
    );

    assert_eq!(experiment.tags.len(), 2);
}

#[test]
fn what_the_encoder_writes_is_read_back() {
    let original = accepted(
        r#"title: t
tags[2]: a, b
nfr[2]:
  - id: a
    kind: availability
    metric: during.availability
    operator: gte
    threshold: 0.9
  - id: b
    kind: availability
    metric: post.availability
    operator: gte
    threshold: 0.9

method[2]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo a: b"
      env:
        REGION: eu
  - name: two
    activity_type: action
    provider:
      type: process
      path: echo
"#,
    );
    let encoded = toon_format::encode_default(&original).expect("encode");

    let read_back = accepted(&encoded);

    assert_eq!(read_back.method.len(), 2);
    assert_eq!(read_back.nfr.len(), 2);
}

#[test]
fn a_key_of_the_file_written_again_after_a_list_is_refused() {
    let error = refusal(
        r"title: t

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
title: again
",
    );

    assert!(error.contains("key 'title'"), "{error}");
    assert!(error.contains("(lines 1 and 9)"), "{error}");
}

#[test]
fn a_key_written_twice_in_a_probe_of_the_hypothesis_is_refused() {
    let error = refusal(
        r"title: t

steady_state_hypothesis:
  title: steady
  probes[1]:
    - name: health
      activity_type: probe
      name: liveness
      provider:
        type: process
        path: echo

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
",
    );

    assert!(error.contains("key 'name'"), "{error}");
    assert!(error.contains("(lines 6 and 8)"), "{error}");
}

#[test]
fn rows_of_a_table_that_start_like_a_key_are_not_keys() {
    let experiment = accepted(
        r"title: t
regulatory:
  frameworks[1]: DORA
  requirements[2]{id,description,evidence}:
    Art: 24,testing programme,one run
    Art: 24,scenario tests,one run

method[1]:
  - name: one
    activity_type: action
    provider:
      type: process
      path: echo
",
    );

    let regulatory = experiment.regulatory.expect("the mapping is read");
    let ids: Vec<&str> = regulatory
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .collect();
    assert_eq!(ids, ["Art: 24", "Art: 24"]);
}
