-- Rows written into each shipped shape before today's open upgrades it.
-- The tests of tumult-lake/tests/lake_shapes.rs check that every row is
-- still there with the same values after the upgrade.
--
-- Every insert names its columns, and every column exists since schema v8,
-- so one file fits all the shapes. The rows touch the tables of the run
-- rebuild of v5 (run_registry, runs, run_audit), of the manual rebuild of
-- v8 (manual_experiments), of the legacy user of v6 (users), and the tables
-- that later versions gain columns on.
INSERT INTO run_registry (
    id, name, definition_toon, content_hash, registered_at_ns, registered_by
) VALUES
    ('reg-seed', 'seed drill', 'title: seed drill', 'hash-seed', 1767225600000000000, 'alice');
INSERT INTO runs (
    id, registry_id, state, params_json, experiment_id, rollback_status, error,
    queued_at_ns, started_at_ns, ended_at_ns
) VALUES
    ('run-seed-1', 'reg-seed', 'completed', '{"rate": 2}', 'exp-seed', 'ok', NULL,
        1767225601000000000, 1767225602000000000, 1767225662000000000),
    ('run-seed-2', 'reg-seed', 'running', NULL, NULL, NULL, NULL,
        1767225700000000000, 1767225701000000000, NULL);
INSERT INTO run_audit (run_id, at_ns, event, detail, actor, prev_hash, new_hash) VALUES
    ('run-seed-1', 1767225601000000000, 'enqueued', NULL, 'alice', NULL, 'audit-hash-1'),
    ('run-seed-1', 1767225662000000000, 'completed', 'ok', NULL, 'audit-hash-1', 'audit-hash-2');
INSERT INTO users (
    id, username, password_hash, role, must_change, disabled, created_at_ns
) VALUES
    ('user-seed', 'alice', '$argon2id$v=19$seed', 'admin', FALSE, FALSE, 1767225500000000000),
    ('user-seed-2', 'bob', '$argon2id$v=19$seed-2', 'approver', TRUE, FALSE, 1767225500000000001);
INSERT INTO sessions (id_hash, user_id, created_at_ns, expires_at_ns) VALUES
    ('session-hash', 'user-seed', 1767225500000000000, 1767311900000000000);
INSERT INTO tokens (
    id, user_id, name, token_hash, created_at_ns, last_used_at_ns, revoked
) VALUES
    ('token-seed', 'user-seed', 'deploy script', 'token-hash', 1767225500000000000, NULL, FALSE);
INSERT INTO user_env_scopes (user_id, environment) VALUES ('user-seed-2', 'staging');
INSERT INTO manual_experiments (
    id, experiment_name, exercise_type, executed_at_ns, hypothesis, method,
    outcome_status, hypothesis_met, findings, action_items, target_system,
    target_environment, blast_radius, recovery_time_s, duration_s, origin,
    entered_by, entered_at_ns, attestation, status, reviewed_by,
    reviewed_at_ns, review_note, renewal_due_ns, framework_refs, batch_id,
    content_hash
) VALUES
    ('manual-seed', 'failover drill', 'failover', 1767225800000000000,
        'the replica takes over', 'stop the primary', 'completed', TRUE,
        'it took 40 s', '["tune the probe"]', 'orders', 'staging',
        'single-service', 40.0, 300.0, 'manual', 'alice', 1767225900000000000,
        'I ran the drill', 'verified', 'bob', 1767226000000000000, 'fine',
        NULL, ['DORA Art. 24'], NULL, 'manual-hash');
INSERT INTO manual_experiment_audit (
    id, experiment_id, changed_by, changed_at_ns, action, diff, prev_hash, new_hash
) VALUES
    ('manual-audit-seed', 'manual-seed', 'alice', 1767225900000000000, 'created', NULL, NULL,
        'manual-audit-hash');
INSERT INTO experiments (
    experiment_id, title, status, started_at_ns, ended_at_ns, duration_ms,
    method_step_count, rollback_count, hypothesis_before_met,
    hypothesis_after_met, estimate_accuracy, resilience_score
) VALUES
    ('exp-seed', 'seed drill', 'completed', 1767225602000000000, 1767225662000000000, 60000,
        2, 1, TRUE, TRUE, 0.9, 0.8);
INSERT INTO spans (
    ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, duration_ns,
    status_code, status_message, service_name, service_version, experiment_id,
    experiment_name, outcome_status, target_system, hypothesis_met,
    recovery_time_s, span_attrs, resource_attrs
) VALUES
    (1767225602000000000, 'trace-seed', 'span-seed', NULL, 'resilience.experiment', 'Internal',
        60000000000, 'Ok', '', 'orders', '1.4.0', 'exp-seed', 'seed drill', 'completed',
        'orders', TRUE, 12.5, MAP {'fault': 'latency'}, MAP {'host': 'node-1'});
INSERT INTO logs (
    ts_ns, severity_text, body, trace_id, span_id, service_name, log_attrs, resource_attrs
) VALUES
    (1767225662000000000, 'INFO', 'experiment.completed', 'trace-seed', 'span-seed', 'tumult',
        MAP {'experiment_id': 'exp-seed', 'status': 'Completed'}, NULL);
INSERT INTO graph_nodes (id, kind, label, attrs) VALUES
    ('run:exp-seed', 'run', 'seed drill', '{"target_system": "orders"}');
INSERT INTO graph_edges (src, rel, dst, run_id, ts, attrs) VALUES
    ('run:exp-seed', 'targets', 'service:orders', 'exp-seed', 1767225602, NULL);
