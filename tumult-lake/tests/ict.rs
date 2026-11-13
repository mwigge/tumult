//! Function-register storage tests: the v20 `ict_legal_entities`,
//! `ict_functions`, `ict_function_services` and `requirement_status_daily`
//! tables — register round-trip, stable register codes across syncs, and
//! the daily status snapshot upsert.

#![cfg(feature = "duckdb")]

use tumult_lake::{
    IctFunction, IctFunctionService, IctLegalEntity, IctRegister, PlannedTest,
    RequirementStatusRow, Store, CURRENT_SCHEMA_VERSION,
};

const LEI: &str = "529900T8BM49AURSDO55";
const OTHER_LEI: &str = "506700GE1G29325QX363";

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn legal_entity(lei: &str) -> IctLegalEntity {
    IctLegalEntity {
        lei: lei.into(),
        name: "Demo Firm".into(),
        country: Some("SE".into()),
        entity_type: Some("investment firm".into()),
        competent_authority: Some("Finansinspektionen".into()),
        profile: "simplified".into(),
        flags: vec!["nis2".into()],
        feeds: vec!["incident".into()],
        registry_signed_by: Some("compliance owner".into()),
        registry_signed_at: Some("2026-11-13".into()),
    }
}

fn function(id: &str, lei: &str, activity: &str, tier: &str) -> IctFunction {
    IctFunction {
        function_id: id.into(),
        register_code: String::new(),
        lei: lei.into(),
        name: format!("function {id}"),
        licensed_activity: activity.into(),
        tier: tier.into(),
        reasons: Some("customer payments".into()),
        last_assessed: Some("2026-09-01".into()),
        rto_s: Some(14_400.0),
        rpo_s: None,
        impact: Some("high".into()),
        org_unit: Some("platform/edge".into()),
        declared: vec!["dora.art28.8".into()],
        test_plan: vec![PlannedTest {
            kind: "dr".into(),
            interval_days: 365,
        }],
        active: true,
    }
}

fn service(function_id: &str, kind: &str, pattern: &str) -> IctFunctionService {
    IctFunctionService {
        function_id: function_id.into(),
        kind: kind.into(),
        pattern: pattern.into(),
    }
}

fn register() -> IctRegister {
    IctRegister {
        legal_entities: vec![legal_entity(LEI)],
        functions: vec![
            function("cif-0042", LEI, "payment services", "critical"),
            function("cif-0042", LEI, "investment services", "critical"),
            function("cif-0050", LEI, "payment services", "important"),
        ],
        services: vec![
            service("cif-0042", "entity", "cif-0042"),
            service("cif-0042", "service", "checkout*"),
            service("cif-0050", "service", "reports"),
        ],
    }
}

fn code_of(register: &IctRegister, id: &str, activity: &str) -> String {
    register
        .functions
        .iter()
        .find(|f| f.function_id == id && f.licensed_activity == activity)
        .unwrap_or_else(|| panic!("{id} / {activity} missing"))
        .register_code
        .clone()
}

#[test]
fn schema_is_v20_and_skips_the_reserved_v19() {
    let (_d, store) = fixture();
    assert_eq!(CURRENT_SCHEMA_VERSION, 20);
    assert_eq!(store.writer().unwrap().schema_version().unwrap(), 20);
}

#[test]
fn a_database_already_at_v19_migrates_to_v20() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        writer
            .execute(
                "UPDATE schema_meta SET value = 19 WHERE key = 'version'",
                [],
            )
            .unwrap();
        writer
            .execute("DROP TABLE requirement_status_daily", [])
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), 20);
    drop(writer);
    let rows = store
        .read_only()
        .unwrap()
        .requirement_status_since(0)
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn register_round_trips_through_the_tables() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.sync_ict_register(&register(), 1_000).unwrap();
    drop(writer);

    let stored = store.read_only().unwrap().ict_register().unwrap();
    assert_eq!(stored.legal_entities, vec![legal_entity(LEI)]);
    assert_eq!(stored.services, register().services);
    assert_eq!(stored.functions.len(), 3);
    let mut expected = register().functions;
    for (function, code) in expected.iter_mut().zip(["F1", "F2", "F3"]) {
        function.register_code = code.into();
    }
    assert_eq!(stored.functions, expected);
}

#[test]
fn one_id_under_two_licensed_activities_gets_two_stable_codes() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.sync_ict_register(&register(), 1_000).unwrap();
    let first = writer.ict_register().unwrap();
    assert_eq!(code_of(&first, "cif-0042", "payment services"), "F1");
    assert_eq!(code_of(&first, "cif-0042", "investment services"), "F2");

    // A second sync that adds a function ahead of the existing ones must
    // leave every assigned code unchanged.
    let mut grown = register();
    grown
        .functions
        .insert(0, function("cif-0007", LEI, "payment services", "other"));
    writer.sync_ict_register(&grown, 2_000).unwrap();
    let second = writer.ict_register().unwrap();
    assert_eq!(code_of(&second, "cif-0042", "payment services"), "F1");
    assert_eq!(code_of(&second, "cif-0042", "investment services"), "F2");
    assert_eq!(code_of(&second, "cif-0050", "payment services"), "F3");
    assert_eq!(code_of(&second, "cif-0007", "payment services"), "F4");
}

#[test]
fn a_removed_function_keeps_its_code_and_the_code_is_never_reused() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.sync_ict_register(&register(), 1_000).unwrap();

    let mut shrunk = register();
    shrunk.functions.retain(|f| f.function_id != "cif-0050");
    writer.sync_ict_register(&shrunk, 2_000).unwrap();
    let after_removal = writer.ict_register().unwrap();
    let removed = after_removal
        .functions
        .iter()
        .find(|f| f.function_id == "cif-0050")
        .expect("retired rows stay in the register");
    assert!(!removed.active);
    assert_eq!(removed.register_code, "F3");

    let mut regrown = shrunk.clone();
    regrown
        .functions
        .push(function("cif-0099", LEI, "payment services", "other"));
    regrown
        .functions
        .push(function("cif-0050", LEI, "payment services", "important"));
    writer.sync_ict_register(&regrown, 3_000).unwrap();
    let last = writer.ict_register().unwrap();
    assert_eq!(code_of(&last, "cif-0099", "payment services"), "F4");
    assert_eq!(
        code_of(&last, "cif-0050", "payment services"),
        "F3",
        "a returning function takes its old code back"
    );
}

#[test]
fn register_codes_are_numbered_per_legal_entity() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let mut two = register();
    two.legal_entities.push(legal_entity(OTHER_LEI));
    two.functions.push(function(
        "cif-0042",
        OTHER_LEI,
        "payment services",
        "critical",
    ));
    writer.sync_ict_register(&two, 1_000).unwrap();
    let stored = writer.ict_register().unwrap();
    let other = stored
        .functions
        .iter()
        .find(|f| f.lei == OTHER_LEI)
        .unwrap();
    assert_eq!(other.register_code, "F1");
}

#[test]
fn status_snapshots_upsert_per_day_function_and_requirement() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let row = |day: i64, status: &str, value: Option<f64>| RequirementStatusRow {
        as_of_day: day,
        function_id: "cif-0042".into(),
        requirement_id: "rts1774.art40".into(),
        status: status.into(),
        value,
        source: "snapshot".into(),
    };
    writer
        .upsert_requirement_status(
            &[
                row(20_000, "existing", Some(1.0)),
                row(20_001, "none", None),
            ],
            5,
        )
        .unwrap();
    // Re-running a day replaces that day's row instead of duplicating it.
    writer
        .upsert_requirement_status(&[row(20_001, "existing", Some(0.75))], 6)
        .unwrap();
    drop(writer);

    let reader = store.read_only().unwrap();
    let all = reader.requirement_status_since(0).unwrap();
    assert_eq!(
        all,
        vec![
            row(20_000, "existing", Some(1.0)),
            row(20_001, "existing", Some(0.75))
        ]
    );
    let recent = reader.requirement_status_since(20_001).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].as_of_day, 20_001);
}
