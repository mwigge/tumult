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
    coalesce(e.mttr_s, e.rto_measured_s) AS recovery_time_s,
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

CREATE OR REPLACE VIEW dr_tests AS
WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(list_value('backup', 'drill', 'failover', 'restore'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged;

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

-- dr_register_sql with the default DR types, as of a time.
WITH
dr_tests AS (
    WITH
    rto AS (
        SELECT
            experiment_id,
            bool_and(recovered) AS recovered
        FROM nfr_verdicts
        WHERE kind = 'rto' AND nfr_id = 'rto'
        GROUP BY experiment_id
    ),

    roots AS (
        SELECT
            experiment_id,
            any_value(target_system) AS target_system,
            any_value(target_environment) AS target_environment
        FROM run_roots
        GROUP BY experiment_id
    ),

    automated AS (
        SELECT
            'automated' AS origin,
            e.experiment_id AS evidence_id,
            e.title AS experiment_name,
            NULL AS exercise_type,
            r.target_system,
            r.target_environment,
            e.started_at_ns AS executed_at_ns,
            lower(e.status) AS outcome,
            e.rto_target_s,
            coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
            coalesce(
                v.recovered,
                CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
            ) AS recovered,
            NOT e.impact_observed AS no_outage,
            e.rpo_target_s,
            e.rpo_measured_s,
            NULL AS data_verified,
            NULL AS reviewer,
            NULL AS plan_id
        FROM experiments AS e
        LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
        LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
        WHERE e.test_kind = 'dr'
    ),

    manual AS (
        SELECT
            'manual' AS origin,
            m.id AS evidence_id,
            m.experiment_name,
            m.exercise_type,
            m.target_system,
            m.target_environment,
            m.executed_at_ns,
            m.outcome_status AS outcome,
            m.rto_target_s,
            m.recovery_time_s AS rto_measured_s,
            CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
            NULL AS no_outage,
            m.rpo_target_s,
            m.rpo_measured_s,
            m.data_verified,
            m.reviewed_by AS reviewer,
            m.plan_id
        FROM manual_experiments AS m
        WHERE
            m.status IN ('submitted', 'verified')
            AND list_contains(list_value('backup', 'drill', 'failover', 'restore'), m.exercise_type)
    ),

    tests AS (
        SELECT * FROM automated
        UNION ALL
        SELECT * FROM manual
    ),

    judged AS (
        SELECT
            *,
            origin || ':' || evidence_id AS test_id,
            CASE
                WHEN NOT recovered THEN 'not_recovered'
                WHEN
                    rto_measured_s > rto_target_s
                    OR rpo_measured_s > rpo_target_s
                    THEN 'breached'
                WHEN
                    coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                    AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                    AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                    THEN 'met'
                ELSE 'not_measured'
            END AS verdict
        FROM tests
    )

    SELECT
        test_id,
        origin,
        experiment_name,
        exercise_type,
        target_system,
        target_environment,
        executed_at_ns,
        outcome,
        rto_target_s,
        rto_measured_s,
        recovered,
        no_outage,
        rpo_target_s,
        rpo_measured_s,
        data_verified,
        verdict,
        evidence_id,
        reviewer,
        plan_id
    FROM judged
),

counted AS (
    SELECT
        target_system,
        target_environment,
        test_id,
        executed_at_ns
    FROM dr_tests
    WHERE
        target_system IS NOT NULL
        AND executed_at_ns <= 1774980000000000000
        AND outcome NOT IN ('aborted', 'interrupted', 'inconclusive')
        AND NOT coalesce(no_outage, FALSE)
),

last_tests AS (
    SELECT
        target_system,
        target_environment,
        max(executed_at_ns) AS last_test_at_ns,
        arg_max(test_id, executed_at_ns) AS last_test_id
    FROM counted
    GROUP BY target_system, target_environment
),

dr_plans AS (
    SELECT
        id,
        target_system,
        target_environment,
        interval_days,
        due_at_ns
    FROM manual_exercise_plans
    WHERE
        status = 'planned'
        AND target_system IS NOT NULL
        AND list_contains(list_value('backup', 'drill', 'failover', 'restore'), exercise_type)
),

declared AS (
    SELECT
        r.target_system,
        r.target_environment
    FROM experiments AS e
    INNER JOIN run_roots AS r ON e.experiment_id = r.experiment_id
    WHERE e.rto_target_s IS NOT NULL
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_experiments
    WHERE rto_target_s IS NOT NULL AND status IN ('submitted', 'verified')
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_exercise_plans
    WHERE rto_target_s IS NOT NULL AND status = 'planned'
),

seen AS (
    SELECT
        target_system,
        target_environment
    FROM dr_tests
    UNION
    SELECT
        target_system,
        target_environment
    FROM declared
),

targets AS (
    SELECT
        target_system,
        target_environment
    FROM seen
    WHERE target_system IS NOT NULL
    UNION
    SELECT
        p.target_system,
        p.target_environment
    FROM dr_plans AS p
    WHERE NOT EXISTS (
        SELECT 1 FROM seen AS s
        WHERE
            s.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR s.target_environment = p.target_environment
            )
    )
),

covering_plans AS (
    SELECT
        t.target_system,
        t.target_environment,
        arg_min(p.id, p.due_at_ns) AS plan_id,
        min(p.due_at_ns) AS plan_due_at_ns,
        min(p.interval_days) AS interval_days
    FROM targets AS t
    INNER JOIN dr_plans AS p
        ON
            t.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR t.target_environment = p.target_environment
            )
    GROUP BY t.target_system, t.target_environment
),

planned_tests AS (
    SELECT
        function_id,
        unnest(
            from_json(test_plan, '[{"kind": "VARCHAR", "interval_days": "BIGINT"}]')
        ) AS planned
    FROM ict_functions
    WHERE active
),

function_intervals AS (
    SELECT
        function_id,
        min(
            least(struct_extract(planned, 'interval_days'), 3650)
        ) AS interval_days
    FROM planned_tests
    WHERE
        struct_extract(planned, 'kind') = 'dr'
        AND struct_extract(planned, 'interval_days') > 0
    GROUP BY function_id
),

target_names AS (
    SELECT DISTINCT target_system FROM targets
),

service_patterns AS (
    SELECT
        function_id,
        kind,
        pattern,
        replace(
            replace(replace(replace(pattern, '!', '!!'), '%', '!%'), '_', '!_'),
            '*',
            '%'
        ) AS like_pattern
    FROM ict_function_services
),

bound_functions AS (
    SELECT
        t.target_system,
        f.function_id
    FROM target_names AS t
    INNER JOIN function_intervals AS f ON t.target_system = f.function_id
    UNION
    SELECT
        t.target_system,
        s.function_id
    FROM target_names AS t
    INNER JOIN service_patterns AS s
        ON
            (s.kind = 'entity' AND t.target_system = s.pattern)
            OR (
                s.kind = 'service'
                AND t.target_system LIKE s.like_pattern ESCAPE '!'
            )
),

target_intervals AS (
    SELECT
        b.target_system,
        min(f.interval_days) AS interval_days
    FROM bound_functions AS b
    INNER JOIN function_intervals AS f ON b.function_id = f.function_id
    GROUP BY b.target_system
),

due_rows AS (
    SELECT
        t.target_system,
        t.target_environment,
        l.last_test_at_ns,
        l.last_test_id,
        c.plan_id,
        c.plan_due_at_ns,
        coalesce(c.interval_days, i.interval_days, 365) AS interval_days,
        CASE
            WHEN c.interval_days IS NOT NULL THEN 'plan'
            WHEN i.interval_days IS NOT NULL THEN 'function'
            ELSE 'default'
        END AS interval_source
    FROM targets AS t
    LEFT JOIN last_tests AS l
        ON
            t.target_system = l.target_system
            AND t.target_environment IS NOT DISTINCT FROM l.target_environment
    LEFT JOIN covering_plans AS c
        ON
            t.target_system = c.target_system
            AND t.target_environment IS NOT DISTINCT FROM c.target_environment
    LEFT JOIN target_intervals AS i ON t.target_system = i.target_system
)

SELECT
    target_system,
    target_environment,
    last_test_at_ns,
    last_test_id,
    interval_days,
    interval_source,
    plan_id,
    plan_due_at_ns,
    last_test_at_ns + interval_days * 86400000000000 AS next_due_at_ns,
    CASE
        WHEN last_test_at_ns IS NULL THEN 'never_tested'
        WHEN last_test_at_ns + interval_days * 86400000000000 < 1774980000000000000 THEN 'overdue'
        WHEN
            last_test_at_ns + interval_days * 86400000000000 - 1774980000000000000
            <= 2592000000000000
            THEN 'due'
        ELSE 'ok'
    END AS state
FROM due_rows;
