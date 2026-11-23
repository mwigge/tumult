//! v25 DR history: the view `dr_tests` and the SQL of the due register.
//!
//! `dr_tests` holds one row per DR test. An automated one is a run of kind
//! `dr`, placed on its target by the root of the run. A manual one is a
//! submitted or verified record of an exercise type that counts as DR. The
//! view counts the types of the default house parameters; a reader that
//! knows the house passes its own list (see `crate::dr`).
//!
//! The due register is a query over the same rows, not a view: it is read
//! as of a time and with the DR types of the house, which a view cannot
//! take.

/// Nanoseconds in one day.
const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// Days between two DR tests when no plan and no function says.
const DEFAULT_INTERVAL_DAYS: i64 = 365;

/// A target whose next DR test is due within this many days is `due`.
const DUE_WITHIN_DAYS: i64 = 30;

/// The longest interval the register reads from a test plan, ten years,
/// as the longest interval a plan takes. A longer one counts as ten years.
const MAX_INTERVAL_DAYS: i64 = 3650;

/// The outcomes of a test that prove nothing either way: a run aborted or
/// interrupted before its method (`RunVerdict::NoEvidence`), and an
/// inconclusive manual record. They are no last test of a target.
const NO_EVIDENCE_OUTCOMES: [&str; 3] = ["aborted", "interrupted", "inconclusive"];

/// `items` as a list of SQL text literals. Each item is a constant of
/// this crate, never a value from outside.
fn literals(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("'{item}'")).collect();
    quoted.join(", ")
}

/// The view of the DR tests, with the manual types of the default house.
pub(super) fn dr_tests_view(default_types: &[&str]) -> String {
    format!(
        "\nCREATE OR REPLACE VIEW dr_tests AS\n{};\n",
        dr_tests_sql(&format!("list_value({})", literals(default_types)))
    )
}

/// The rows of `dr_tests`, as a query. `manual_types` is an SQL list of
/// the manual exercise types that count as DR.
///
/// The verdict reads the numbers of the test. `not_recovered`: the run did
/// not recover while it was watched. `breached`: an RTO or an RPO measured
/// is over its target. `met`: every target declared was measured and met.
/// `not_measured`: no target, or a target that nothing measured. A run
/// that did not recover keeps no measured RTO: its verdict holds the time
/// it watched, and this view does not turn that into one. `no_outage`
/// says that the probes of a run saw no outage, so it measured no
/// recovery; it is empty for a run stored before v24 and for a manual
/// record.
pub(crate) fn dr_tests_sql(manual_types: &str) -> String {
    format!(
        "WITH
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
        AND list_contains({manual_types}, m.exercise_type)
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
FROM judged"
    )
}

/// The due register, as a query: one row per target, read as of
/// `as_of_ns` (an SQL expression), with `manual_types` as in
/// [`dr_tests_sql`].
///
/// The targets are those with a DR test, those that declare an RTO (a run
/// of any kind, a submitted or verified record, a planned plan) and those
/// of a planned DR plan. The last test is the latest DR test up to
/// `as_of_ns` that proves something: not a run aborted or interrupted
/// before its method, not an inconclusive manual record, and not a run
/// whose probes saw no outage. The next one is due one interval
/// after it: the interval of a planned DR plan that covers the target,
/// else of the DR test plan of a function of the register the target
/// binds to, else [`DEFAULT_INTERVAL_DAYS`]. A plan covers the target of
/// its system, in its environment or, with none, in any. A target binds to
/// a function by its id, an `entity` mapping or a `service` glob, as the
/// DORA score binds a run.
pub(crate) fn dr_register_sql(manual_types: &str, as_of_ns: &str) -> String {
    let tests: Vec<String> = dr_tests_sql(manual_types)
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect();
    let tests = tests.join("\n");
    let no_evidence = literals(&NO_EVIDENCE_OUTCOMES);
    format!(
        "WITH
dr_tests AS (
{tests}
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
        AND executed_at_ns <= {as_of_ns}
        AND outcome NOT IN ({no_evidence})
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
        AND list_contains({manual_types}, exercise_type)
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
            from_json(test_plan, '[{{\"kind\": \"VARCHAR\", \"interval_days\": \"BIGINT\"}}]')
        ) AS planned
    FROM ict_functions
    WHERE active
),

function_intervals AS (
    SELECT
        function_id,
        min(
            least(struct_extract(planned, 'interval_days'), {MAX_INTERVAL_DAYS})
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
        coalesce(c.interval_days, i.interval_days, {DEFAULT_INTERVAL_DAYS}) AS interval_days,
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
    last_test_at_ns + interval_days * {DAY_NS} AS next_due_at_ns,
    CASE
        WHEN last_test_at_ns IS NULL THEN 'never_tested'
        WHEN last_test_at_ns + interval_days * {DAY_NS} < {as_of_ns} THEN 'overdue'
        WHEN
            last_test_at_ns + interval_days * {DAY_NS} - {as_of_ns}
            <= {due_within_ns}
            THEN 'due'
        ELSE 'ok'
    END AS state
FROM due_rows",
        due_within_ns = DUE_WITHIN_DAYS * DAY_NS,
    )
}
