use tumult_agentic::scenarios::bundled_packs;

#[test]
fn bundled_scenario_packs_cover_mvp_catalog() {
    let packs = bundled_packs();
    let names = packs.iter().map(|pack| pack.name).collect::<Vec<_>>();

    assert_eq!(packs.len(), 6);
    assert!(names.contains(&"concurrency-storm"));
    assert!(names.contains(&"hallucination-under-timeout"));
    assert!(names.contains(&"cost-explosion-detector"));
    assert!(names.contains(&"malformed-json-recovery"));
    assert!(names.contains(&"tool-timeout-fallback"));
    assert!(names.contains(&"retrieval-poisoning"));

    for pack in packs {
        assert!(
            !pack.supported_adapters.is_empty(),
            "{} missing adapter support",
            pack.name
        );
        assert!(!pack.faults.is_empty(), "{} missing faults", pack.name);
        assert!(
            !pack.contracts.is_empty(),
            "{} missing contracts",
            pack.name
        );
    }
}
