-- The SQL that functions of tumult-lake/src/schema/mod.rs build, as the
-- lake runs it: each view as create_views() makes it, then a statement for
-- each function that no view uses. scripts/lint-embedded-sql.sh lints this
-- file with sqlfluff, and a test of tumult-lake keeps it the SQL of the
-- functions. Change the SQL in the functions, then write this file again
-- and read the difference:
--   TUMULT_BLESS=1 cargo test -p tumult-lake -- sql_built_by_functions

CREATE OR REPLACE VIEW run_roots AS
SELECT
    s.ts_ns,
    s.trace_id,
    s.span_id,
    s.parent_span_id,
    s.span_name,
    s.span_kind,
    s.duration_ns,
    s.status_code,
    s.status_message,
    s.service_name,
    s.service_version,
    s.experiment_id,
    s.experiment_name,
    s.outcome_status,
    s.fault_type,
    s.fault_subtype,
    s.fault_severity,
    s.blast_radius,
    s.target_system,
    s.target_technology,
    s.target_environment,
    s.plugin_name,
    s.hypothesis_met,
    s.recovery_time_s,
    s.span_attrs,
    s.resource_attrs,
    s.events,
    FALSE AS journal_only
FROM spans AS s
WHERE s.span_name = 'resilience.experiment'
UNION ALL
SELECT
    e.started_at_ns AS ts_ns,
    NULL AS trace_id,
    NULL AS span_id,
    NULL AS parent_span_id,
    'resilience.experiment' AS span_name,
    'Internal' AS span_kind,
    e.ended_at_ns - e.started_at_ns AS duration_ns,
    CASE WHEN lower(e.status) = 'completed' THEN 'Ok' ELSE 'Error' END AS status_code,
    '' AS status_message,
    'tumult' AS service_name,
    NULL AS service_version,
    e.experiment_id,
    e.title AS experiment_name,
    lower(e.status) AS outcome_status,
    NULL AS fault_type,
    NULL AS fault_subtype,
    NULL AS fault_severity,
    NULL AS blast_radius,
    json_extract_string(g.attrs, '$.target_system') AS target_system,
    json_extract_string(g.attrs, '$.target_technology') AS target_technology,
    json_extract_string(g.attrs, '$.target_environment') AS target_environment,
    NULL AS plugin_name,
    coalesce(
        e.hypothesis_after_met,
        CASE WHEN NOT e.hypothesis_before_met THEN FALSE END
    ) AS hypothesis_met,
    e.rto_measured_s AS recovery_time_s,
    NULL AS span_attrs,
    NULL AS resource_attrs,
    NULL AS events,
    TRUE AS journal_only
FROM experiments AS e
LEFT JOIN graph_nodes AS g ON g.id = 'run:' || e.experiment_id
WHERE NOT EXISTS (
    SELECT 1 FROM spans AS r
    WHERE
        r.span_name = 'resilience.experiment'
        AND r.experiment_id = e.experiment_id
);

CREATE OR REPLACE VIEW run_completions AS
SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE body = 'experiment.completed'
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1;

CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value(lower(coalesce(s.outcome_status, l.log_attrs['status']))) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM run_roots AS s
LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id
GROUP BY s.experiment_id;

CREATE OR REPLACE VIEW entities_current AS
SELECT
    entity_key,
    environment,
    entity_type,
    entity_id,
    state_hash,
    description,
    relationships,
    valid_from_ns,
    last_seen_ns,
    last_received_ns,
    interval_s,
    deleted_at_ns,
    delete_reason,
    wire_form,
    observer,
    schema_url,
    CASE
        WHEN deleted_at_ns IS NOT NULL THEN 'gone'
        WHEN
            interval_s > 0
            AND epoch_ns(now()) - last_received_ns > 3 * interval_s * 1000000000
            THEN 'expired'
        ELSE 'live'
    END AS liveness
FROM entity_versions
QUALIFY
    row_number() OVER (
        PARTITION BY entity_key, environment
        ORDER BY valid_from_ns DESC
    ) = 1;

-- run_completions_sql with a window, as a reader joins it.
SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774980000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1;

-- run_verdict_sql of the outcome of each run.
SELECT
    s.experiment_id,
    CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END AS verdict
FROM run_roots AS s
LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id;

-- mean_recovery_time_sql over the roots of the runs.
SELECT avg(s.recovery_time_s) AS mean_recovery_time_s
FROM run_roots AS s;
