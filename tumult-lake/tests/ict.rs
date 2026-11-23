//! Function-register storage tests: the v20 `ict_legal_entities`,
//! `ict_functions`, `ict_function_services` and `requirement_status_daily`
//! tables — register round-trip, stable register codes across syncs, and
//! the daily status snapshot upsert.

#![cfg(feature = "duckdb")]

use tumult_lake::{
    IctFunction, IctFunctionService, IctLegalEntity, IctRegister, MaturityDailyRow, PlannedTest,
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

const V19_TABLES: [&str; 1] = ["forecasts"];
const V20_TABLES: [&str; 5] = [
    "ict_legal_entities",
    "ict_functions",
    "ict_function_services",
    "requirement_status_daily",
    "maturity_daily",
];

fn tables_present(store: &Store, names: &[&str]) -> Vec<String> {
    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows("SELECT table_name FROM information_schema.tables ORDER BY table_name")
        .unwrap();
    rows.iter()
        .filter_map(|r| r["table_name"].as_str())
        .filter(|t| names.contains(t))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_fresh_lake_is_current_with_the_forecast_and_the_register_tables() {
    let (_d, store) = fixture();
    assert_eq!(CURRENT_SCHEMA_VERSION, 22);
    assert_eq!(
        store.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(tables_present(&store, &V19_TABLES).len(), V19_TABLES.len());
    assert_eq!(tables_present(&store, &V20_TABLES).len(), V20_TABLES.len());
}

#[test]
fn a_v19_lake_opens_keeps_its_forecasts_and_gains_the_v20_tables() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for table in V20_TABLES {
            writer.execute(&format!("DROP TABLE {table}"), []).unwrap();
        }
        writer
            .execute(
                "UPDATE schema_meta SET value = 19 WHERE key = 'version'",
                [],
            )
            .unwrap();
        drop(writer);
        assert!(
            tables_present(&store, &V20_TABLES).is_empty(),
            "the seeded lake is v19-shaped"
        );
        assert_eq!(tables_present(&store, &V19_TABLES).len(), 1);
    }
    let store = Store::open(&path).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    drop(writer);
    assert_eq!(tables_present(&store, &V19_TABLES).len(), 1);
    assert_eq!(tables_present(&store, &V20_TABLES).len(), V20_TABLES.len());
    let rows = store
        .read_only()
        .unwrap()
        .requirement_status_between(0, 30_000, 1_000)
        .unwrap();
    assert!(rows.is_empty());
}

/// A lake two versions back has neither the forecasts nor the register
/// tables: opening it runs both steps and leaves what it held alone.
#[test]
fn a_v18_lake_opens_and_gains_the_v19_and_the_v20_tables() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for table in V20_TABLES.iter().chain(&V19_TABLES) {
            writer.execute(&format!("DROP TABLE {table}"), []).unwrap();
        }
        writer
            .execute(
                "UPDATE schema_meta SET value = 18 WHERE key = 'version'",
                [],
            )
            .unwrap();
        assert_eq!(writer.schema_version().unwrap(), 18);
        drop(writer);
        assert!(
            tables_present(&store, &V20_TABLES).is_empty()
                && tables_present(&store, &V19_TABLES).is_empty(),
            "the seeded lake is v18-shaped"
        );
    }
    let store = Store::open(&path).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    writer.sync_ict_register(&register(), 1_000).unwrap();
    drop(writer);
    assert_eq!(tables_present(&store, &V19_TABLES).len(), V19_TABLES.len());
    assert_eq!(tables_present(&store, &V20_TABLES).len(), V20_TABLES.len());
    let reader = store.read_only().unwrap();
    let forecasts = reader
        .query_json_rows("SELECT count(*) AS c FROM forecasts")
        .unwrap();
    assert_eq!(forecasts[0]["c"], 0);
    let functions = reader
        .query_json_rows("SELECT count(*) AS c FROM ict_functions")
        .unwrap();
    assert!(functions[0]["c"].as_u64().unwrap() > 0);
    // Opening it again changes nothing.
    drop(reader);
    drop(store);
    let again = Store::open(&path).unwrap();
    assert_eq!(
        again.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
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
    let all = reader.requirement_status_between(0, 30_000, 1_000).unwrap();
    assert_eq!(
        all,
        vec![
            row(20_000, "existing", Some(1.0)),
            row(20_001, "existing", Some(0.75))
        ]
    );
    let recent = reader
        .requirement_status_between(20_001, 30_000, 1_000)
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].as_of_day, 20_001);
}

fn status_row(day: i64) -> RequirementStatusRow {
    RequirementStatusRow {
        as_of_day: day,
        function_id: "cif-0042".into(),
        requirement_id: "rts1774.art40".into(),
        status: "existing".into(),
        value: Some(1.0),
        source: "snapshot".into(),
    }
}

#[test]
fn status_rows_are_read_in_a_bounded_range_and_the_newest_are_kept() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let rows: Vec<RequirementStatusRow> = (20_000..20_010).map(status_row).collect();
    writer.upsert_requirement_status(&rows, 5).unwrap();
    drop(writer);
    let reader = store.read_only().unwrap();
    let days = |from: i64, to: i64, limit: usize| -> Vec<i64> {
        reader
            .requirement_status_between(from, to, limit)
            .unwrap()
            .iter()
            .map(|r| r.as_of_day)
            .collect()
    };
    assert_eq!(days(20_003, 20_005, 100), [20_003, 20_004, 20_005]);
    // A day after the range is not read, however many rows are allowed.
    assert_eq!(days(20_008, 20_008, 100), [20_008]);
    // Over the limit the oldest rows go: a shorter streak, never a longer one.
    assert_eq!(days(20_000, 20_009, 3), [20_007, 20_008, 20_009]);
}

fn maturity_row(day: i64, lei: &str, level: u8, index: f64) -> MaturityDailyRow {
    MaturityDailyRow {
        as_of_day: day,
        lei: lei.into(),
        level,
        index,
        unknown_share: 0.25,
        source: "snapshot".into(),
    }
}

#[test]
fn maturity_snapshots_upsert_per_day_and_legal_entity() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer
        .upsert_maturity_daily(
            &[
                maturity_row(20_000, LEI, 2, 2.5),
                maturity_row(20_001, LEI, 2, 2.6),
                maturity_row(20_001, OTHER_LEI, 4, 4.4),
            ],
            5,
        )
        .unwrap();
    writer
        .upsert_maturity_daily(&[maturity_row(20_001, LEI, 3, 3.1)], 6)
        .unwrap();
    drop(writer);
    let reader = store.read_only().unwrap();
    assert_eq!(
        reader.maturity_daily_between(LEI, 0, 30_000).unwrap(),
        vec![
            maturity_row(20_000, LEI, 2, 2.5),
            maturity_row(20_001, LEI, 3, 3.1)
        ]
    );
    assert_eq!(
        reader
            .maturity_daily_between(OTHER_LEI, 20_001, 20_001)
            .unwrap(),
        vec![maturity_row(20_001, OTHER_LEI, 4, 4.4)]
    );
    assert!(reader
        .maturity_daily_between(LEI, 20_002, 30_000)
        .unwrap()
        .is_empty());
    // The LEI of a request is compared, never parsed.
    for hostile in ["x' OR '1'='1", "'; DROP TABLE maturity_daily; --", "?"] {
        let rows = reader.maturity_daily_between(hostile, 0, 30_000).unwrap();
        assert!(rows.is_empty(), "{hostile}: {rows:?}");
    }
    assert_eq!(
        reader.maturity_daily_between(LEI, 0, 30_000).unwrap().len(),
        2
    );
}

#[test]
fn pruning_drops_only_the_snapshot_days_before_the_cut() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    let rows: Vec<RequirementStatusRow> = (20_000..20_004).map(status_row).collect();
    writer.upsert_requirement_status(&rows, 5).unwrap();
    let levels: Vec<MaturityDailyRow> = (20_000..20_004)
        .map(|day| maturity_row(day, LEI, 2, 2.5))
        .collect();
    writer.upsert_maturity_daily(&levels, 5).unwrap();
    assert_eq!(writer.prune_score_snapshots(20_002).unwrap(), 4);
    assert_eq!(writer.prune_score_snapshots(20_002).unwrap(), 0);
    drop(writer);
    let reader = store.read_only().unwrap();
    let status: Vec<i64> = reader
        .requirement_status_between(0, 30_000, 100)
        .unwrap()
        .iter()
        .map(|r| r.as_of_day)
        .collect();
    assert_eq!(status, [20_002, 20_003]);
    let maturity: Vec<i64> = reader
        .maturity_daily_between(LEI, 0, 30_000)
        .unwrap()
        .iter()
        .map(|r| r.as_of_day)
        .collect();
    assert_eq!(maturity, [20_002, 20_003]);
}
