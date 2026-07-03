//! Background-activity execution tests (Task 77).

use crate::common::*;

// ═══════════════════════════════════════════════════════════════
// Task 77: Background activities execute concurrently
// ═══════════════════════════════════════════════════════════════

#[test]
fn background_and_sequential_activities_all_execute() {
    // Note: The current runner executes all activities sequentially
    // (background support requires async). This test verifies that
    // background-flagged activities still execute in the method.
    let mut exp = experiment_builder();
    exp.method = vec![
        action("sequential-1"),
        background_action("background-1"),
        action("sequential-2"),
        background_action("background-2"),
    ];

    let mock_plugin = MockPlugin::new();
    let execution_log = mock_plugin.execution_log.clone();
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(mock_plugin);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(journal.method_results.len(), 4);

    // All activities should have executed
    let log = execution_log.lock().unwrap();
    assert_eq!(log.len(), 4);
    assert!(log.contains(&"sequential-1".to_string()));
    assert!(log.contains(&"background-1".to_string()));
    assert!(log.contains(&"sequential-2".to_string()));
    assert!(log.contains(&"background-2".to_string()));
}
