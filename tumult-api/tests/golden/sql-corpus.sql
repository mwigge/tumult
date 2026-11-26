-- The SQL corpus of tumult-api: every statement the crate runs, as its
-- code renders it, each under the file and the item that hold it. A test
-- of the crate prepares each statement on a fresh lake and keeps this
-- file; CI lints it with sqlfluff. Change the SQL in the code, then write
-- this file again and read the difference:
--   TUMULT_BLESS=1 cargo test -p tumult-api -- sql_corpus

-- tumult-api/src/sql.rs:GAMEDAYS
SELECT id, name, content_hash, registered_at_ns, registered_by FROM run_registry WHERE kind = 'gameday' ORDER BY registered_at_ns DESC LIMIT 500;

-- tumult-api/src/sql.rs:METRIC_POINTS
SELECT 'sum' AS kind, ts_ns, metric_name, value, outcome_status, plugin_name FROM metric_sums WHERE experiment_name = $1 AND ts_ns BETWEEN $2 AND $3 UNION ALL SELECT 'gauge', ts_ns, metric_name, value, outcome_status, plugin_name FROM metric_gauges WHERE experiment_name = $1 AND ts_ns BETWEEN $2 AND $3 ORDER BY 2 LIMIT 1000;

-- tumult-api/src/sql.rs:DEFINITIONS
SELECT rg.id, rg.name, (SELECT COUNT(*) FROM runs r WHERE r.registry_id = rg.id) AS runs FROM run_registry rg WHERE rg.kind IS DISTINCT FROM 'gameday' ORDER BY rg.name;

-- tumult-api/src/sql.rs:EVIDENCE_EDGES
SELECT DISTINCT src, rel, dst FROM graph_edges WHERE rel IN ('evidences', 'maps_to_compliance');

-- tumult-api/src/sql.rs:DEPENDS_ON_EDGES
SELECT DISTINCT src, dst FROM graph_edges WHERE rel = 'depends_on';

-- tumult-api/src/approvals.rs:runs_since_sql(1_774_900_000_000_000_000)
SELECT COUNT(*) AS n FROM runs WHERE started_at_ns > 1774900000000000000;

-- tumult-api/src/ask.rs:GOLDEN[how many experiments ran]
SELECT COUNT(*) AS experiments FROM run_roots;

-- tumult-api/src/ask.rs:GOLDEN[what is the pass rate]
SELECT count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS pass_rate FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id;

-- tumult-api/src/ask.rs:GOLDEN[which experiments failed]
SELECT s.experiment_id, s.experiment_name, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status FROM spans s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.span_name = 'resilience.experiment' AND CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'fail' ORDER BY s.ts_ns DESC;

-- tumult-api/src/ask.rs:GOLDEN[what faults were injected]
SELECT fault_type, COUNT(*) AS count FROM spans WHERE fault_type IS NOT NULL GROUP BY fault_type ORDER BY count DESC;

-- tumult-api/src/ask.rs:GOLDEN[experiments per day]
SELECT (ts_ns // 86400000000000) * 86400000000000 // 1000000000 AS day_s, COUNT(*) AS experiments FROM spans WHERE span_name = 'resilience.experiment' GROUP BY 1 ORDER BY 1;

-- tumult-api/src/ask.rs:GOLDEN[show recent experiments]
SELECT s.experiment_id, s.experiment_name, s.ts_ns AS started_ns, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status FROM spans s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.span_name = 'resilience.experiment' ORDER BY s.ts_ns DESC LIMIT 20;

-- tumult-api/src/auth/scopes.rs:run_in_scope_sql("run-1", &predicate)
SELECT r.id FROM runs r LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.id = 'run-1' AND (COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod'));

-- tumult-api/src/auth/scopes.rs:run_in_scope_sql("run-1", &run_scope_sql(&[]))
SELECT r.id FROM runs r LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.id = 'run-1' AND (TRUE);

-- tumult-api/src/events.rs:events_sql(&none, &[], 100)
SELECT a.*, g.name AS definition_name FROM run_audit a LEFT JOIN runs r ON r.id = a.run_id LEFT JOIN run_registry g ON g.id = r.registry_id LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id  ORDER BY a.at_ns DESC LIMIT 100;

-- tumult-api/src/events.rs:events_sql(&every, &["prod".to_string()], 100)
SELECT a.*, g.name AS definition_name FROM run_audit a LEFT JOIN runs r ON r.id = a.run_id LEFT JOIN run_registry g ON g.id = r.registry_id LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE a.run_id = 'run-1' AND a.event = 'enqueued' AND a.at_ns < 1774986400000000000 AND COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod') ORDER BY a.at_ns DESC LIMIT 100;

-- tumult-api/src/forecast/metric.rs:series_sql(action_duration_p95)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, quantile_cont(CAST(duration_ns AS DOUBLE), 0.95) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod') AND target_system = 'checkout')
WHERE span_name = 'resilience.action'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(action_duration_s)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, SUM(CAST(sum AS DOUBLE)) / NULLIF(SUM(CAST(count AS DOUBLE)), 0) AS value
FROM (SELECT * FROM metric_histograms WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')) AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_system = 'checkout'))
WHERE metric_name = 'tumult.action.duration'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(coverage)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(DISTINCT target_system) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod') AND target_system = 'checkout')
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(deviation_rate)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) FILTER (WHERE metric_name = 'tumult.hypothesis.deviations.total') / NULLIF(COUNT(*) FILTER (WHERE metric_name = 'tumult.experiments.total'), 0) AS value
FROM (SELECT * FROM metric_sums WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')) AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_system = 'checkout'))
WHERE ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(experiment_count)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod') AND target_system = 'checkout')
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(experiment_coverage)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(DISTINCT experiment_name) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod') AND target_system = 'checkout')
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(experiment_duration_s)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, SUM(CAST(sum AS DOUBLE)) / NULLIF(SUM(CAST(count AS DOUBLE)), 0) AS value
FROM (SELECT * FROM metric_histograms WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')) AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_system = 'checkout'))
WHERE metric_name = 'tumult.experiment.duration'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(hypothesis_pass_rate)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) FILTER (WHERE outcome_status = 'success') / NULLIF(COUNT(*), 0) AS value
FROM (SELECT * FROM metric_sums WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')) AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_system = 'checkout'))
WHERE metric_name = 'tumult.experiments.total'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/metric.rs:series_sql(mttr)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, AVG(recovery_time_s) AS value
FROM (SELECT * FROM run_roots WHERE target_environment IN ('prod') AND target_system = 'checkout')
WHERE ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/forecast/run_impact.rs:experiment_of_run_sql("run-1", &predicate)
SELECT r.experiment_id FROM runs r LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.id = 'run-1' AND (COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod'));

-- tumult-api/src/forecast/run_impact.rs:samples_sql("exp-1")
SELECT probe_name, phase, ts_ns, value FROM probe_samples WHERE experiment_id = 'exp-1' ORDER BY ts_ns ASC;

-- tumult-api/src/gamedays.rs:gameday_sql("gameday-1")
SELECT * FROM run_registry WHERE id = 'gameday-1' AND kind = 'gameday';

-- tumult-api/src/handlers/conformance.rs:sql(1_774_900_000_000_000_000, "")
SELECT service_name, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns, COUNT(*) FILTER (WHERE target_system IS NOT NULL) AS k0, COUNT(*) FILTER (WHERE target_technology IS NOT NULL) AS k1, COUNT(*) FILTER (WHERE target_environment IS NOT NULL) AS k2, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.schema.version') OR list_contains(map_keys(span_attrs), 'ict.schema.version')) AS k3, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.id') OR list_contains(map_keys(span_attrs), 'ict.entity.id')) AS k4, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.tier') OR list_contains(map_keys(span_attrs), 'ict.entity.tier')) AS k5, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.owner') OR list_contains(map_keys(span_attrs), 'ict.entity.owner')) AS k6, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'resilience.target.component') OR list_contains(map_keys(span_attrs), 'resilience.target.component')) AS k7, COUNT(*) FILTER (WHERE service_version IS NOT NULL) AS k8 FROM spans WHERE ts_ns >= 1774900000000000000 AND service_name <> '' GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/conformance.rs:sql(1_774_900_000_000_000_000, &scope)
SELECT service_name, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns, COUNT(*) FILTER (WHERE target_system IS NOT NULL) AS k0, COUNT(*) FILTER (WHERE target_technology IS NOT NULL) AS k1, COUNT(*) FILTER (WHERE target_environment IS NOT NULL) AS k2, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.schema.version') OR list_contains(map_keys(span_attrs), 'ict.schema.version')) AS k3, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.id') OR list_contains(map_keys(span_attrs), 'ict.entity.id')) AS k4, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.tier') OR list_contains(map_keys(span_attrs), 'ict.entity.tier')) AS k5, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'ict.entity.owner') OR list_contains(map_keys(span_attrs), 'ict.entity.owner')) AS k6, COUNT(*) FILTER (WHERE list_contains(map_keys(resource_attrs), 'resilience.target.component') OR list_contains(map_keys(span_attrs), 'resilience.target.component')) AS k7, COUNT(*) FILTER (WHERE service_version IS NOT NULL) AS k8 FROM spans WHERE ts_ns >= 1774900000000000000 AND service_name <> '' AND target_environment IN ('prod') GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/conformance.rs:entity_ids_sql(1_774_900_000_000_000_000, &scope)
SELECT service_name, LEFT(COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id']), 64) AS entity_id, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns, COUNT(*) FILTER (WHERE COALESCE(resource_attrs['tumult.ict.entity.id.malformed'], span_attrs['tumult.ict.entity.id.malformed']) = 'true') AS flagged, MAX(length(COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id']))) > 64 AS cut FROM spans WHERE ts_ns >= 1774900000000000000 AND service_name <> '' AND COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id']) IS NOT NULL AND (length(COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id'])) > 63 OR NOT regexp_full_match(COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id']), '^[a-z][a-z0-9]*(-[a-z0-9]+)+$')) AND target_environment IN ('prod') GROUP BY 1, 2 ORDER BY 1, 2 LIMIT 501;

-- tumult-api/src/handlers/dr.rs:RTO_AT_LEAST_SQL
SELECT experiment_id, max(observed) AS rto_at_least_s FROM nfr_verdicts WHERE kind = 'rto' AND nfr_id = 'rto' AND NOT recovered AND observed IS NOT NULL AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), experiment_id) GROUP BY experiment_id;

-- tumult-api/src/handlers/entities.rs:list_count_sql()
SELECT count(*) AS total FROM entities_current WHERE (CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) AND last_received_ns >= CAST($3 AS BIGINT) AND ($4 IS NULL OR list_contains(from_json(CAST($4 AS VARCHAR), '["VARCHAR"]'), environment));

-- tumult-api/src/handlers/entities.rs:list_page_sql(Sort::Key, 100, MAX_PAGE_BYTES)
WITH candidates AS (SELECT entity_key, environment, entity_type, entity_id, CAST(description AS VARCHAR) AS description, CAST(relationships AS VARCHAR) AS relationships, valid_from_ns, last_seen_ns, last_received_ns, interval_s, deleted_at_ns, delete_reason, wire_form, observer, schema_url, liveness FROM entities_current WHERE (CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) AND last_received_ns >= CAST($3 AS BIGINT) AND ($4 IS NULL OR list_contains(from_json(CAST($4 AS VARCHAR), '["VARCHAR"]'), environment)) AND (CAST($5 AS VARCHAR) IS NULL OR entity_key > CAST($5 AS VARCHAR) OR (entity_key = CAST($5 AS VARCHAR) AND environment > CAST($6 AS VARCHAR))) ORDER BY entity_key, environment LIMIT 101), weighed AS (SELECT *, count(*) OVER () AS page_candidates, sum(512 + strlen(description) + strlen(relationships)) OVER (ORDER BY entity_key, environment ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS page_bytes_before FROM candidates) SELECT * FROM weighed WHERE coalesce(page_bytes_before, 0) < 4194304 ORDER BY entity_key, environment LIMIT 100;

-- tumult-api/src/handlers/entities.rs:list_page_sql(Sort::Recent, 100, MAX_PAGE_BYTES)
WITH candidates AS (SELECT entity_key, environment, entity_type, entity_id, CAST(description AS VARCHAR) AS description, CAST(relationships AS VARCHAR) AS relationships, valid_from_ns, last_seen_ns, last_received_ns, interval_s, deleted_at_ns, delete_reason, wire_form, observer, schema_url, liveness FROM entities_current WHERE (CAST($1 AS VARCHAR) IS NULL OR entity_type = CAST($1 AS VARCHAR)) AND (CAST($2 AS VARCHAR) IS NULL OR liveness = CAST($2 AS VARCHAR)) AND last_received_ns >= CAST($3 AS BIGINT) AND ($4 IS NULL OR list_contains(from_json(CAST($4 AS VARCHAR), '["VARCHAR"]'), environment)) AND (CAST($5 AS BIGINT) IS NULL OR last_received_ns < CAST($5 AS BIGINT) OR (last_received_ns = CAST($5 AS BIGINT) AND (entity_key > CAST($6 AS VARCHAR) OR (entity_key = CAST($6 AS VARCHAR) AND environment > CAST($7 AS VARCHAR))))) ORDER BY last_received_ns DESC, entity_key, environment LIMIT 101), weighed AS (SELECT *, count(*) OVER () AS page_candidates, sum(512 + strlen(description) + strlen(relationships)) OVER (ORDER BY last_received_ns DESC, entity_key, environment ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS page_bytes_before FROM candidates) SELECT * FROM weighed WHERE coalesce(page_bytes_before, 0) < 4194304 ORDER BY last_received_ns DESC, entity_key, environment LIMIT 100;

-- tumult-api/src/handlers/entities.rs:history_environments_sql()
SELECT environment, count(*) AS versions FROM entity_versions WHERE entity_key = CAST($1 AS VARCHAR) AND ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), environment)) GROUP BY environment ORDER BY environment;

-- tumult-api/src/handlers/entities.rs:history_count_sql()
SELECT count(*) AS total FROM entity_versions WHERE entity_key = CAST($1 AS VARCHAR) AND ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), environment)) AND (CAST($3 AS VARCHAR) IS NULL OR environment = CAST($3 AS VARCHAR));

-- tumult-api/src/handlers/entities.rs:history_page_sql(100, MAX_PAGE_BYTES)
WITH candidates AS (SELECT entity_key, environment, entity_type, entity_id, CAST(description AS VARCHAR) AS description, CAST(relationships AS VARCHAR) AS relationships, valid_from_ns, last_seen_ns, last_received_ns, interval_s, deleted_at_ns, delete_reason, wire_form, observer, schema_url FROM entity_versions WHERE entity_key = CAST($1 AS VARCHAR) AND ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), environment)) AND (CAST($3 AS VARCHAR) IS NULL OR environment = CAST($3 AS VARCHAR)) AND (CAST($4 AS BIGINT) IS NULL OR valid_from_ns < CAST($4 AS BIGINT) OR (valid_from_ns = CAST($4 AS BIGINT) AND environment > CAST($5 AS VARCHAR))) ORDER BY valid_from_ns DESC, environment LIMIT 101), weighed AS (SELECT *, count(*) OVER () AS page_candidates, sum(512 + strlen(description) + strlen(relationships)) OVER (ORDER BY valid_from_ns DESC, environment ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS page_bytes_before FROM candidates) SELECT * FROM weighed WHERE coalesce(page_bytes_before, 0) < 4194304 ORDER BY valid_from_ns DESC, environment LIMIT 100;

-- tumult-api/src/handlers/evidence.rs:limited_sql(SUMMARY, 1)
SELECT
    test_kind AS kind,
    started_at_ns,
    ended_at_ns,
    {
        'before_met': hypothesis_before_met,
        'after_met': hypothesis_after_met
    } AS hypothesis,
    {
        'rto_target_s': rto_target_s,
        'rto_measured_s': rto_measured_s,
        'rpo_target_s': rpo_target_s,
        'rpo_measured_s': rpo_measured_s,
        'impact_observed': impact_observed,
        'outage_started_at_ns': outage_started_at_ns,
        'recovered_at_ns': recovered_at_ns
    } AS recovery
FROM experiments
WHERE experiment_id = $1 LIMIT 1;

-- tumult-api/src/handlers/evidence.rs:limited_sql(STEPS, MAX_ROWS)
SELECT
    phase,
    name,
    activity_type,
    status,
    started_at_ns,
    duration_ms,
    output,
    error
FROM activity_results
WHERE experiment_id = $1
ORDER BY
    CASE phase
        WHEN 'hypothesis_before' THEN 0
        WHEN 'method' THEN 1
        WHEN 'hypothesis_after' THEN 2
        ELSE 3
    END,
    started_at_ns,
    rowid LIMIT 2000;

-- tumult-api/src/handlers/evidence.rs:limited_sql(OBJECTIVES, MAX_ROWS)
SELECT
    nfr_id,
    kind,
    metric,
    operator,
    threshold,
    observed,
    state,
    margin,
    criticality,
    recovered,
    reason
FROM nfr_verdicts
WHERE experiment_id = $1
ORDER BY rowid LIMIT 2000;

-- tumult-api/src/handlers/evidence.rs:limited_sql(LOAD, 1)
SELECT
    tool,
    started_at_ns,
    ended_at_ns,
    duration_s,
    vus,
    throughput_rps,
    latency_p50_ms,
    latency_p95_ms,
    latency_p99_ms,
    error_rate,
    total_requests,
    thresholds_met
FROM load_results
WHERE experiment_id = $1 LIMIT 1;

-- tumult-api/src/handlers/evidence.rs:load_values_sql()
SELECT ts_ns, probe_name, value FROM probe_samples WHERE experiment_id = $1 AND list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), probe_name) ORDER BY ts_ns, rowid LIMIT 20000;

-- tumult-api/src/handlers/evidence.rs:probe_samples_sql()
SELECT probe_name AS probe, phase, ts_ns, value AS duration_ms, ok FROM probe_samples WHERE experiment_id = $1 AND NOT list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), probe_name) ORDER BY ts_ns, probe_name, rowid LIMIT 5000;

-- tumult-api/src/handlers/evidence.rs:probe_count_sql()
SELECT count(*) AS n FROM probe_samples WHERE experiment_id = $1 AND NOT list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), probe_name);

-- tumult-api/src/handlers/experiments.rs:spans_only_sql(&spans)
SELECT s.experiment_id AS id, s.experiment_name AS name, s.ts_ns AS started_ns, s.duration_ns, s.trace_id, s.target_system, s.target_technology, s.target_environment, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status, l.log_attrs['deviations'] AS deviations, l.log_attrs['duration_ms'] AS duration_ms, s.journal_only, (SELECT string_agg(DISTINCT c.fault_type, ',') FROM spans c WHERE c.trace_id = s.trace_id AND c.fault_type IS NOT NULL) AS faults, 'automated' AS origin, NULL AS review_status FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.span_name = 'resilience.experiment' AND s.target_environment IN ('prod') AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND lower(coalesce(s.outcome_status, l.log_attrs['status'])) = 'failed' AND s.target_system = 'payments' AND EXISTS (SELECT 1 FROM spans c WHERE c.trace_id = s.trace_id AND c.fault_type = 'network.latency') AND (s.experiment_name ILIKE '%checkout%' ESCAPE '\' OR s.experiment_id ILIKE '%checkout%' ESCAPE '\') AND s.experiment_name = 'checkout-latency' ORDER BY s.ts_ns DESC LIMIT 500;

-- tumult-api/src/handlers/experiments.rs:both_sql(&spans, &manual)
SELECT * FROM (SELECT s.experiment_id AS id, s.experiment_name AS name, s.ts_ns AS started_ns, s.duration_ns, s.trace_id, s.target_system, s.target_technology, s.target_environment, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status, l.log_attrs['deviations'] AS deviations, l.log_attrs['duration_ms'] AS duration_ms, s.journal_only, (SELECT string_agg(DISTINCT c.fault_type, ',') FROM spans c WHERE c.trace_id = s.trace_id AND c.fault_type IS NOT NULL) AS faults, 'automated' AS origin, NULL AS review_status FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.span_name = 'resilience.experiment' AND s.target_environment IN ('prod') AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND lower(coalesce(s.outcome_status, l.log_attrs['status'])) = 'failed' AND s.target_system = 'payments' AND EXISTS (SELECT 1 FROM spans c WHERE c.trace_id = s.trace_id AND c.fault_type = 'network.latency') AND (s.experiment_name ILIKE '%checkout%' ESCAPE '\' OR s.experiment_id ILIKE '%checkout%' ESCAPE '\') AND s.experiment_name = 'checkout-latency' UNION ALL SELECT m.id, m.experiment_name AS name, m.executed_at_ns AS started_ns, CAST(m.duration_s * 1000000000 AS BIGINT) AS duration_ns, NULL AS trace_id, m.target_system, NULL AS target_technology, m.target_environment, m.outcome_status AS status, NULL AS deviations, NULL AS duration_ms, false AS journal_only, NULL AS faults, 'manual' AS origin, m.status AS review_status FROM manual_experiments m WHERE m.status <> 'draft' AND m.executed_at_ns >= 1774900000000000000 AND m.executed_at_ns < 1774986400000000000 AND m.target_system = 'payments' AND (m.experiment_name ILIKE '%restore%' ESCAPE '\' OR m.id ILIKE '%restore%' ESCAPE '\') AND m.experiment_name = 'restore-drill') ORDER BY started_ns DESC LIMIT 500;

-- tumult-api/src/handlers/experiments.rs:manual_only_sql(&manual)
SELECT m.id, m.experiment_name AS name, m.executed_at_ns AS started_ns, CAST(m.duration_s * 1000000000 AS BIGINT) AS duration_ns, NULL AS trace_id, m.target_system, NULL AS target_technology, m.target_environment, m.outcome_status AS status, NULL AS deviations, NULL AS duration_ms, false AS journal_only, NULL AS faults, 'manual' AS origin, m.status AS review_status FROM manual_experiments m WHERE m.status <> 'draft' AND m.executed_at_ns >= 1774900000000000000 AND m.executed_at_ns < 1774986400000000000 AND m.target_system = 'payments' AND (m.experiment_name ILIKE '%restore%' ESCAPE '\' OR m.id ILIKE '%restore%' ESCAPE '\') AND m.experiment_name = 'restore-drill' ORDER BY started_ns DESC LIMIT 500;

-- tumult-api/src/handlers/experiments.rs:run_windows_sql(1_774_900_000_000_000_000, 1_774_986_400_000_000_000, &[])
SELECT experiment_id AS id, experiment_name AS name, started_at_ns AS start_ns, ended_at_ns AS end_ns, outcome_status AS outcome FROM experiment_runs WHERE started_at_ns < 1774986400000000000 AND ended_at_ns > 1774900000000000000 ORDER BY started_at_ns LIMIT 200;

-- tumult-api/src/handlers/experiments.rs:run_windows_sql(1_774_900_000_000_000_000, 1_774_986_400_000_000_000, &prod)
SELECT experiment_id AS id, experiment_name AS name, started_at_ns AS start_ns, ended_at_ns AS end_ns, outcome_status AS outcome FROM experiment_runs WHERE started_at_ns < 1774986400000000000 AND ended_at_ns > 1774900000000000000 AND EXISTS (SELECT 1 FROM run_roots se WHERE se.experiment_id = experiment_runs.experiment_id AND se.target_environment IN ('prod')) ORDER BY started_at_ns LIMIT 200;

-- tumult-api/src/handlers/experiments.rs:detail_sql(&id_sql, "")
SELECT s.experiment_id AS id, s.experiment_name AS name, s.ts_ns AS started_ns, s.duration_ns, s.trace_id, s.target_system, s.target_technology, s.target_environment, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status, l.log_attrs['deviations'] AS deviations, l.log_attrs['duration_ms'] AS duration_ms, s.journal_only FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.experiment_id = 'run-1';

-- tumult-api/src/handlers/experiments.rs:detail_sql(&id_sql, " AND s.target_environment IN ('prod')")
SELECT s.experiment_id AS id, s.experiment_name AS name, s.ts_ns AS started_ns, s.duration_ns, s.trace_id, s.target_system, s.target_technology, s.target_environment, lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS status, l.log_attrs['deviations'] AS deviations, l.log_attrs['duration_ms'] AS duration_ms, s.journal_only FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE s.experiment_id = 'run-1' AND s.target_environment IN ('prod');

-- tumult-api/src/handlers/experiments.rs:detail_spans_sql(&id_sql)
SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, duration_ns, status_code, status_message, service_name, fault_type, fault_subtype, span_attrs, events FROM spans WHERE experiment_id = 'run-1' OR trace_id IN (SELECT trace_id FROM spans WHERE experiment_id = 'run-1') ORDER BY ts_ns LIMIT 2000;

-- tumult-api/src/handlers/experiments.rs:detail_logs_sql(&id_sql, &[])
SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs FROM logs WHERE log_attrs['experiment_id'] = 'run-1' ORDER BY ts_ns LIMIT 1000;

-- tumult-api/src/handlers/experiments.rs:detail_logs_sql(&id_sql, &[sql_string("trace-1"), sql_string("trace-2")])
SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs FROM logs WHERE log_attrs['experiment_id'] = 'run-1' OR trace_id IN ('trace-1', 'trace-2') ORDER BY ts_ns LIMIT 1000;

-- tumult-api/src/handlers/experiments.rs:outcomes_sql(&prod)
SELECT DISTINCT lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS v FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id WHERE lower(coalesce(s.outcome_status, l.log_attrs['status'])) IS NOT NULL AND s.target_environment IN ('prod') ORDER BY 1;

-- tumult-api/src/handlers/experiments.rs:targets_sql(&prod)
SELECT DISTINCT target_system AS v FROM ( SELECT target_system, target_environment FROM spans UNION ALL SELECT target_system, target_environment FROM run_roots WHERE journal_only) WHERE target_system IS NOT NULL AND target_environment IN ('prod') ORDER BY 1;

-- tumult-api/src/handlers/experiments.rs:faults_sql(&prod)
SELECT DISTINCT fault_type AS v FROM spans WHERE fault_type IS NOT NULL AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) ORDER BY 1;

-- tumult-api/src/handlers/experiments.rs:names_sql(&[])
SELECT DISTINCT experiment_name AS v FROM run_roots WHERE experiment_name IS NOT NULL ORDER BY 1;

-- tumult-api/src/handlers/logs.rs:logs_sql(&window, 200)
SELECT ts_ns, severity_text, body, trace_id, span_id, service_name, log_attrs['experiment_id'] AS experiment_id, log_attrs, resource_attrs FROM logs WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 ORDER BY ts_ns DESC LIMIT 200;

-- tumult-api/src/handlers/logs.rs:logs_sql(&every, 200)
SELECT ts_ns, severity_text, body, trace_id, span_id, service_name, log_attrs['experiment_id'] AS experiment_id, log_attrs, resource_attrs FROM logs WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.target_environment IN ('prod') AND (se.trace_id = logs.trace_id OR se.experiment_id = logs.log_attrs['experiment_id'])) AND severity_text ILIKE 'ERROR' ESCAPE '\' AND service_name = 'checkout' AND body ILIKE '%timeout%' ESCAPE '\' AND (log_attrs['http.method'] = 'GET' OR resource_attrs['http.method'] = 'GET') AND NOT (COALESCE(log_attrs['k8s.pod.name'] = 'api-0', false) OR COALESCE(resource_attrs['k8s.pod.name'] = 'api-0', false)) ORDER BY ts_ns DESC LIMIT 200;

-- tumult-api/src/handlers/logs.rs:volume_sql(3_600, &every)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COALESCE(severity_text, 'UNKNOWN') AS severity, COUNT(*) AS count FROM logs WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.target_environment IN ('prod') AND (se.trace_id = logs.trace_id OR se.experiment_id = logs.log_attrs['experiment_id'])) AND severity_text ILIKE 'ERROR' ESCAPE '\' AND service_name = 'checkout' AND body ILIKE '%timeout%' ESCAPE '\' AND (log_attrs['http.method'] = 'GET' OR resource_attrs['http.method'] = 'GET') AND NOT (COALESCE(log_attrs['k8s.pod.name'] = 'api-0', false) OR COALESCE(resource_attrs['k8s.pod.name'] = 'api-0', false)) GROUP BY 1, 2 ORDER BY 1;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(action_duration_p95)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, quantile_cont(CAST(duration_ns AS DOUBLE), 0.95) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod'))
WHERE span_name = 'resilience.action'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(action_duration_s)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, SUM(CAST(sum AS DOUBLE)) / NULLIF(SUM(CAST(count AS DOUBLE)), 0) AS value
FROM (SELECT * FROM metric_histograms WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')))
WHERE metric_name = 'tumult.action.duration'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(coverage)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(DISTINCT target_system) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod'))
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(deviation_rate)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) FILTER (WHERE metric_name = 'tumult.hypothesis.deviations.total') / NULLIF(COUNT(*) FILTER (WHERE metric_name = 'tumult.experiments.total'), 0) AS value
FROM (SELECT * FROM metric_sums WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')))
WHERE ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(experiment_count)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod'))
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(experiment_coverage)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(DISTINCT experiment_name) AS value
FROM (SELECT * FROM spans WHERE target_environment IN ('prod'))
WHERE span_name = 'resilience.experiment'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(experiment_duration_s)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, SUM(CAST(sum AS DOUBLE)) / NULLIF(SUM(CAST(count AS DOUBLE)), 0) AS value
FROM (SELECT * FROM metric_histograms WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')))
WHERE metric_name = 'tumult.experiment.duration'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(hypothesis_pass_rate)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, COUNT(*) FILTER (WHERE outcome_status = 'success') / NULLIF(COUNT(*), 0) AS value
FROM (SELECT * FROM metric_sums WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')))
WHERE metric_name = 'tumult.experiments.total'
  AND ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:scoped_series_sql(mttr)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS bucket_s, AVG(recovery_time_s) AS value
FROM (SELECT * FROM run_roots WHERE target_environment IN ('prod'))
WHERE ts_ns >= 1774900000000000000
  AND ts_ns < 1774986400000000000
GROUP BY bucket_s
ORDER BY bucket_s;

-- tumult-api/src/handlers/metrics.rs:catalog_sql(metric_sums)
SELECT metric_name, list_distinct(flatten(list(k))) AS k FROM (SELECT DISTINCT metric_name, map_keys(attrs) AS k FROM metric_sums WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod'))) AS key_lists GROUP BY metric_name;

-- tumult-api/src/handlers/metrics.rs:metric_hit_sql(metric_sums)
SELECT 1 AS hit FROM metric_sums WHERE metric_name = $1 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')) LIMIT 1;

-- tumult-api/src/handlers/metrics.rs:points_sql(metric_sums)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, SUM(value) AS v, attrs['http.route'] AS grp FROM metric_sums WHERE metric_name = 'http.server.duration' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')) GROUP BY 1, 3 ORDER BY 1;

-- tumult-api/src/handlers/metrics.rs:catalog_sql(metric_gauges)
SELECT metric_name, list_distinct(flatten(list(k))) AS k FROM (SELECT DISTINCT metric_name, map_keys(attrs) AS k FROM metric_gauges WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_gauges.experiment_name AND se.target_environment IN ('prod'))) AS key_lists GROUP BY metric_name;

-- tumult-api/src/handlers/metrics.rs:metric_hit_sql(metric_gauges)
SELECT 1 AS hit FROM metric_gauges WHERE metric_name = $1 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_gauges.experiment_name AND se.target_environment IN ('prod')) LIMIT 1;

-- tumult-api/src/handlers/metrics.rs:points_sql(metric_gauges)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, AVG(value) AS v, attrs['http.route'] AS grp FROM metric_gauges WHERE metric_name = 'http.server.duration' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_gauges.experiment_name AND se.target_environment IN ('prod')) GROUP BY 1, 3 ORDER BY 1;

-- tumult-api/src/handlers/metrics.rs:catalog_sql(metric_histograms)
SELECT metric_name, list_distinct(flatten(list(k))) AS k FROM (SELECT DISTINCT metric_name, map_keys(attrs) AS k FROM metric_histograms WHERE EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod'))) AS key_lists GROUP BY metric_name;

-- tumult-api/src/handlers/metrics.rs:metric_hit_sql(metric_histograms)
SELECT 1 AS hit FROM metric_histograms WHERE metric_name = $1 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')) LIMIT 1;

-- tumult-api/src/handlers/metrics.rs:histogram_sql(metric_histograms)
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, count, sum, bucket_counts, explicit_bounds, attrs['http.route'] AS grp FROM metric_histograms WHERE metric_name = 'http.server.duration' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_histograms.experiment_name AND se.target_environment IN ('prod')) ORDER BY 1 LIMIT 10000;

-- tumult-api/src/handlers/metrics.rs:catalog_sql("metric_sums", &[])
SELECT metric_name, list_distinct(flatten(list(k))) AS k FROM (SELECT DISTINCT metric_name, map_keys(attrs) AS k FROM metric_sums) AS key_lists GROUP BY metric_name;

-- tumult-api/src/handlers/metrics.rs:points_sql(&MetricQuery { table: "metric_gauges", name: "process.cpu", cur, bucket_s: 300, group_by: None, }, "gauge", &[])
SELECT (ts_ns // 300000000000) * 300000000000 // 1000000000 AS ts, AVG(value) AS v FROM metric_gauges WHERE metric_name = 'process.cpu' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[experiments].value in all
SELECT COUNT(*) AS v FROM run_roots WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000;

-- tumult-api/src/handlers/overview.rs:KPIS[experiments].spark_num in all
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(*) AS v FROM run_roots WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[pass_rate].value in all
SELECT count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS v FROM run_roots AS s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) AS l ON s.experiment_id = l.experiment_id WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000;

-- tumult-api/src/handlers/overview.rs:KPIS[pass_rate].spark_num in all
SELECT (s.ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS v FROM run_roots AS s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) AS l ON s.experiment_id = l.experiment_id WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].value in all
SELECT (SELECT COALESCE(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums WHERE metric_name = 'tumult.hypothesis.deviations.total' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000) / NULLIF((SELECT COUNT(*) FROM spans WHERE span_name = 'resilience.experiment' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000), 0) AS v;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].spark_num in all
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COALESCE(SUM(CAST(value AS DOUBLE)), 0) AS v FROM metric_sums WHERE metric_name = 'tumult.hypothesis.deviations.total' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].spark_den in all
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(*) AS v FROM spans WHERE span_name = 'resilience.experiment' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[mttr_s].value in all
SELECT avg(s.recovery_time_s) AS v FROM run_roots AS s WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000;

-- tumult-api/src/handlers/overview.rs:KPIS[mttr_s].spark_num in all
SELECT (s.ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, avg(s.recovery_time_s) AS v FROM run_roots AS s WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[coverage].value in all
SELECT COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000;

-- tumult-api/src/handlers/overview.rs:KPIS[coverage].spark_num in all
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[experiments].value in prod
SELECT COUNT(*) AS v FROM run_roots WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND target_environment IN ('prod');

-- tumult-api/src/handlers/overview.rs:KPIS[experiments].spark_num in prod
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(*) AS v FROM run_roots WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND target_environment IN ('prod') GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[pass_rate].value in prod
SELECT count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS v FROM run_roots AS s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) AS l ON s.experiment_id = l.experiment_id WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod');

-- tumult-api/src/handlers/overview.rs:KPIS[pass_rate].spark_num in prod
SELECT (s.ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS v FROM run_roots AS s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) AS l ON s.experiment_id = l.experiment_id WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod') GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].value in prod
SELECT (SELECT COALESCE(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums WHERE metric_name = 'tumult.hypothesis.deviations.total' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod'))) / NULLIF((SELECT COUNT(*) FROM spans WHERE span_name = 'resilience.experiment' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod'))), 0) AS v;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].spark_num in prod
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COALESCE(SUM(CAST(value AS DOUBLE)), 0) AS v FROM metric_sums WHERE metric_name = 'tumult.hypothesis.deviations.total' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod')) GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[deviation_rate].spark_den in prod
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(*) AS v FROM spans WHERE span_name = 'resilience.experiment' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[mttr_s].value in prod
SELECT avg(s.recovery_time_s) AS v FROM run_roots AS s WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod');

-- tumult-api/src/handlers/overview.rs:KPIS[mttr_s].spark_num in prod
SELECT (s.ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, avg(s.recovery_time_s) AS v FROM run_roots AS s WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod') GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:KPIS[coverage].value in prod
SELECT COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod'));

-- tumult-api/src/handlers/overview.rs:KPIS[coverage].spark_num in prod
SELECT (ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts, COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:sql_per_day(f, t, &prod)
SELECT (ts_ns // 86400000000000) * 86400000000000 // 1000000000 AS ts, COUNT(*) AS v FROM run_roots WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND target_environment IN ('prod') GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/overview.rs:sql_targets(f, t, &prod)
SELECT s.target_system AS target, count(*) AS experiments, count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END = 'pass') / nullif(count(*) FILTER (WHERE CASE lower(lower(coalesce(s.outcome_status, l.log_attrs['status'])))
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END IN ('pass', 'fail')), 0) AS pass_rate FROM run_roots AS s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) AS l ON s.experiment_id = l.experiment_id WHERE s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod') AND s.target_system IS NOT NULL GROUP BY 1 ORDER BY experiments DESC LIMIT 10;

-- tumult-api/src/handlers/overview.rs:sql_faults(f, t, &[])
SELECT s.fault_type, COALESCE(s.fault_subtype, s.plugin_name) AS fault_subtype, COUNT(*) AS count FROM spans AS s WHERE s.fault_type IS NOT NULL AND s.fault_type <> 'observation' AND NOT EXISTS (SELECT 1 FROM spans AS r WHERE r.trace_id = s.trace_id AND r.span_id = s.parent_span_id AND r.span_name = 'resilience.rollback') AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 10;

-- tumult-api/src/handlers/overview.rs:sql_faults(f, t, &prod)
SELECT s.fault_type, COALESCE(s.fault_subtype, s.plugin_name) AS fault_subtype, COUNT(*) AS count FROM spans AS s WHERE s.fault_type IS NOT NULL AND s.fault_type <> 'observation' AND NOT EXISTS (SELECT 1 FROM spans AS r WHERE r.trace_id = s.trace_id AND r.span_id = s.parent_span_id AND r.span_name = 'resilience.rollback') AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = s.trace_id AND se.target_environment IN ('prod')) GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 10;

-- tumult-api/src/handlers/perf/evidence.rs:runs_sql()
WITH roots AS (SELECT experiment_id AS id, min(ts_ns) AS started, max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, max(span_attrs['resilience.target.version']) AS span_version, max(span_attrs['resilience.target.version_source']) AS span_source FROM spans WHERE span_name = 'resilience.experiment' AND target_system = $1 AND experiment_id IS NOT NULL AND ts_ns >= $2 AND ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), target_environment)) GROUP BY experiment_id), runs AS (SELECT r.id, r.started, r.ended, r.env, e.status, coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source ELSE r.span_source END AS version_source FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id) SELECT u.id, u.started, u.ended, u.env, u.status, u.version, u.version_source, CAST(l.aborted AS INTEGER) AS aborted, CAST(l.truncated AS INTEGER) AS truncated, l.planned_windows AS planned, l.completed_windows AS completed, l.unanswered_windows AS unanswered, l.lost_to_unanswered AS lost, l.requests_sent AS sent, l.requests_ok AS ok FROM runs AS u LEFT JOIN load_results AS l ON u.id = l.experiment_id ORDER BY u.started, u.id;

-- tumult-api/src/handlers/perf/evidence.rs:inside_sql()
WITH roots AS (SELECT experiment_id AS id, min(ts_ns) AS started, max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, max(span_attrs['resilience.target.version']) AS span_version, max(span_attrs['resilience.target.version_source']) AS span_source FROM spans WHERE span_name = 'resilience.experiment' AND target_system = $1 AND experiment_id IS NOT NULL AND ts_ns >= $2 AND ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), target_environment)) GROUP BY experiment_id), runs AS (SELECT r.id, r.started, r.ended, r.env, e.status, coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source ELSE r.span_source END AS version_source FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id) SELECT u.id AS id, s.service_version AS v, count(*) AS n, count(*) FILTER (WHERE s.ts_ns >= u.started AND s.ts_ns <= u.ended) AS own FROM runs AS u INNER JOIN spans AS s ON s.target_environment IS NOT DISTINCT FROM u.env WHERE u.version IS NULL AND s.service_name = $1 AND s.service_version IS NOT NULL AND s.service_version <> '' AND s.ts_ns >= $4 AND s.ts_ns >= u.started - 2000000000 AND s.ts_ns <= u.ended + 2000000000 AND (upper(s.span_kind) IN ('SERVER', 'CONSUMER') OR s.parent_span_id IS NULL OR s.parent_span_id = '') GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version;

-- tumult-api/src/handlers/perf/evidence.rs:before_sql()
WITH roots AS (SELECT experiment_id AS id, min(ts_ns) AS started, max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, max(span_attrs['resilience.target.version']) AS span_version, max(span_attrs['resilience.target.version_source']) AS span_source FROM spans WHERE span_name = 'resilience.experiment' AND target_system = $1 AND experiment_id IS NOT NULL AND ts_ns >= $2 AND ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), target_environment)) GROUP BY experiment_id), runs AS (SELECT r.id, r.started, r.ended, r.env, e.status, coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source ELSE r.span_source END AS version_source FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id) SELECT u.id AS id, s.service_version AS v, max(s.ts_ns) AS last FROM runs AS u INNER JOIN spans AS s ON s.target_environment IS NOT DISTINCT FROM u.env WHERE u.version IS NULL AND s.service_name = $1 AND s.service_version IS NOT NULL AND s.service_version <> '' AND s.ts_ns >= $4 AND s.ts_ns < u.started AND s.ts_ns >= u.started - 3600000000000 GROUP BY u.id, s.service_version ORDER BY u.id, s.service_version;

-- tumult-api/src/handlers/perf/evidence.rs:recorded_sql()
WITH roots AS (SELECT experiment_id AS id, min(ts_ns) AS started, max(ts_ns + duration_ns) AS ended, min(target_environment) AS env, max(span_attrs['resilience.target.version']) AS span_version, max(span_attrs['resilience.target.version_source']) AS span_source FROM spans WHERE span_name = 'resilience.experiment' AND target_system = $1 AND experiment_id IS NOT NULL AND ts_ns >= $2 AND ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), target_environment)) GROUP BY experiment_id), runs AS (SELECT r.id, r.started, r.ended, r.env, e.status, coalesce(nullif(e.target_version, ''), nullif(r.span_version, '')) AS version, CASE WHEN nullif(e.target_version, '') IS NOT NULL THEN e.version_source ELSE r.span_source END AS version_source FROM roots AS r LEFT JOIN experiments AS e ON r.id = e.experiment_id) SELECT u.env AS env, u.version AS version, min(u.started) AS first, max(u.ended) AS last FROM runs AS u WHERE u.version IS NOT NULL AND coalesce(u.version_source, '') <> 'mixed' GROUP BY u.env, u.version ORDER BY u.env, u.version;

-- tumult-api/src/handlers/perf/evidence.rs:samples_sql()
WITH mine AS (SELECT experiment_id, ts_ns, probe_name, value FROM probe_samples WHERE phase = 'during' AND experiment_id IN (SELECT experiment_id FROM spans WHERE span_name = 'resilience.experiment' AND target_system = $1 AND experiment_id IS NOT NULL AND ts_ns >= $2 AND ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), target_environment)))) SELECT p.experiment_id AS id, p.value AS value, w.value AS weight, t.value AS timeout FROM mine AS p LEFT JOIN (SELECT experiment_id, ts_ns, max(value) AS value FROM mine WHERE probe_name = 'load.window_s' GROUP BY experiment_id, ts_ns) AS w ON w.experiment_id = p.experiment_id AND w.ts_ns = p.ts_ns LEFT JOIN (SELECT experiment_id, ts_ns, max(value) AS value FROM mine WHERE probe_name = 'load.request_timeout_ms' GROUP BY experiment_id, ts_ns) AS t ON t.experiment_id = p.experiment_id AND t.ts_ns = p.ts_ns WHERE p.probe_name = $4 ORDER BY p.ts_ns, p.experiment_id, p.value;

-- tumult-api/src/handlers/perf/evidence.rs:targets_sql()
SELECT target_system AS target, target_environment AS env, count(DISTINCT experiment_id) AS runs, max(ts_ns) AS last_run FROM spans WHERE span_name = 'resilience.experiment' AND target_system IS NOT NULL AND experiment_id IS NOT NULL AND ts_ns >= $1 AND ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), target_environment)) GROUP BY target_system, target_environment ORDER BY target_system, target_environment NULLS LAST;

-- tumult-api/src/handlers/recommend.rs:registered_sql("checkout-latency")
SELECT id, name, definition_toon FROM run_registry WHERE name = 'checkout-latency' ORDER BY registered_at_ns DESC LIMIT 1;

-- tumult-api/src/handlers/risks.rs:automated_kinds_sql(&ids)
SELECT experiment_id, test_kind, rto_target_s, rto_measured_s, rpo_target_s, rpo_measured_s FROM experiments WHERE experiment_id IN ('run-1', 'run-2');

-- tumult-api/src/handlers/risks.rs:manual_kinds_sql(&ids)
SELECT id, exercise_type, recovery_time_s, rto_target_s, rpo_target_s, rpo_measured_s FROM manual_experiments WHERE id IN ('run-1', 'run-2');

-- tumult-api/src/handlers/risks.rs:verdicts_sql(&ids)
SELECT experiment_id, nfr_id, kind, state, criticality, threshold, observed, margin, recovered FROM nfr_verdicts WHERE experiment_id IN ('run-1', 'run-2') ORDER BY nfr_id;

-- tumult-api/src/handlers/risks.rs:dependencies_sql(&[])
SELECT c.service_name AS service, COUNT(DISTINCT p.service_name) AS callers, COUNT(*) AS calls FROM spans c JOIN spans p ON c.parent_span_id = p.span_id WHERE p.service_name <> c.service_name AND c.service_name <> '' GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/risks.rs:dependencies_sql(&["prod".to_string()])
SELECT c.service_name AS service, COUNT(DISTINCT p.service_name) AS callers, COUNT(*) AS calls FROM spans c JOIN spans p ON c.parent_span_id = p.span_id WHERE p.service_name <> c.service_name AND c.service_name <> '' AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = c.trace_id AND se.target_environment IN ('prod')) GROUP BY 1 ORDER BY 1;

-- tumult-api/src/handlers/starmap.rs:services_sql(&open)
SELECT service_name AS service, NULLIF(resource_attrs['ict.entity.id'], '') AS entity_id, NULLIF(resource_attrs['ict.entity.tier'], '') AS entity_tier, NULLIF(resource_attrs['ict.entity.owner'], '') AS entity_owner, NULLIF(target_system, '') AS system, NULLIF(resource_attrs['resilience.target.component'], '') AS component, NULLIF(target_environment, '') AS environment, NULLIF(resource_attrs['k8s.namespace.name'], '') AS k8s_namespace, NULLIF(resource_attrs['k8s.deployment.name'], '') AS k8s_deployment, MAX(service_version) AS version, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns FROM spans WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND service_name <> '' AND span_name <> 'resilience.experiment' GROUP BY 1, 2, 3, 4, 5, 6, 7, 8, 9;

-- tumult-api/src/handlers/starmap.rs:services_sql(&prod)
SELECT service_name AS service, NULLIF(resource_attrs['ict.entity.id'], '') AS entity_id, NULLIF(resource_attrs['ict.entity.tier'], '') AS entity_tier, NULLIF(resource_attrs['ict.entity.owner'], '') AS entity_owner, NULLIF(target_system, '') AS system, NULLIF(resource_attrs['resilience.target.component'], '') AS component, NULLIF(target_environment, '') AS environment, NULLIF(resource_attrs['k8s.namespace.name'], '') AS k8s_namespace, NULLIF(resource_attrs['k8s.deployment.name'], '') AS k8s_deployment, MAX(service_version) AS version, COUNT(*) AS spans, MAX(ts_ns) AS last_seen_ns FROM spans WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND service_name <> '' AND span_name <> 'resilience.experiment' AND target_environment IN ('prod') GROUP BY 1, 2, 3, 4, 5, 6, 7, 8, 9;

-- tumult-api/src/handlers/starmap.rs:calls_sql(&prod)
SELECT p.service_name AS client, c.service_name AS server, COUNT(*) AS calls, COUNT(*) FILTER (WHERE c.status_code = 'Error') AS errors, AVG(c.duration_ns) AS avg_duration_ns FROM spans c JOIN spans p ON c.parent_span_id = p.span_id WHERE p.service_name <> c.service_name AND p.service_name <> '' AND c.service_name <> '' AND c.ts_ns >= 1774900000000000000 AND c.ts_ns < 1774986400000000000 AND p.ts_ns >= 1774896400000000000 AND p.ts_ns < 1774986400000000000 AND c.target_environment IN ('prod') AND p.target_environment IN ('prod') GROUP BY 1, 2;

-- tumult-api/src/handlers/starmap.rs:runs_sql(&open)
SELECT s.experiment_id AS id, any_value(s.experiment_name) AS experiment, any_value(s.target_system) AS system, any_value(s.target_environment) AS environment, MIN(s.ts_ns) AS started_ns, any_value(lower(coalesce(s.outcome_status, l.log_attrs['status']))) AS status, false AS manual FROM run_roots s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) l ON s.experiment_id = l.experiment_id WHERE s.experiment_id IS NOT NULL AND s.experiment_name IS NOT NULL AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 GROUP BY s.experiment_id UNION ALL SELECT m.id, m.experiment_name, m.target_system, m.target_environment, m.executed_at_ns, m.outcome_status, true FROM manual_experiments m WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' AND m.executed_at_ns >= 1774900000000000000 AND m.executed_at_ns < 1774986400000000000;

-- tumult-api/src/handlers/starmap.rs:runs_sql(&prod)
SELECT s.experiment_id AS id, any_value(s.experiment_name) AS experiment, any_value(s.target_system) AS system, any_value(s.target_environment) AS environment, MIN(s.ts_ns) AS started_ns, any_value(lower(coalesce(s.outcome_status, l.log_attrs['status']))) AS status, false AS manual FROM run_roots s LEFT JOIN (SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774900000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1) l ON s.experiment_id = l.experiment_id WHERE s.experiment_id IS NOT NULL AND s.experiment_name IS NOT NULL AND s.ts_ns >= 1774900000000000000 AND s.ts_ns < 1774986400000000000 AND s.target_environment IN ('prod') GROUP BY s.experiment_id UNION ALL SELECT m.id, m.experiment_name, m.target_system, m.target_environment, m.executed_at_ns, m.outcome_status, true FROM manual_experiments m WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' AND m.executed_at_ns >= 1774900000000000000 AND m.executed_at_ns < 1774986400000000000 AND m.target_environment IN ('prod');

-- tumult-api/src/handlers/starmap.rs:capped_sql(crate::sql::EVIDENCE_EDGES, 5_000)
SELECT DISTINCT src, rel, dst FROM graph_edges WHERE rel IN ('evidences', 'maps_to_compliance') ORDER BY ALL LIMIT 5001;

-- tumult-api/src/handlers/starmap.rs:capped_sql(crate::sql::DEPENDS_ON_EDGES, 5_000)
SELECT DISTINCT src, dst FROM graph_edges WHERE rel = 'depends_on' ORDER BY ALL LIMIT 5001;

-- tumult-api/src/handlers/starmap.rs:manual_refs_sql(&prod)
SELECT m.id AS id, unnest(m.framework_refs) AS reference FROM manual_experiments m WHERE m.status = 'verified' AND m.executed_at_ns >= 1774900000000000000 AND m.executed_at_ns < 1774986400000000000 AND m.target_environment IN ('prod');

-- tumult-api/src/handlers/starmap.rs:capped_sql(&nodes_of_kind_sql("compliance_article"), 5_000)
SELECT id, label FROM graph_nodes WHERE kind = 'compliance_article' ORDER BY ALL LIMIT 5001;

-- tumult-api/src/handlers/starmap.rs:capped_sql(&nodes_of_kind_sql("coverage_gap"), 5_000)
SELECT id, label FROM graph_nodes WHERE kind = 'coverage_gap' ORDER BY ALL LIMIT 5001;

-- tumult-api/src/handlers/topology.rs:service_nodes_sql(&window)
SELECT 'svc:' || replace(replace(replace(service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS id, service_name AS name, 'service' AS type, COUNT(*) AS runs, COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, AVG(duration_ns) AS avg_duration_ns FROM spans WHERE service_name <> '' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) GROUP BY 1, 2;

-- tumult-api/src/handlers/topology.rs:target_nodes_sql(&window)
SELECT CASE WHEN CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END IS NOT NULL THEN 'cmp:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') || '/' || replace(replace(replace(CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END, '%', '%25'), '/', '%2F'), '|', '%7C') ELSE 'sys:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') END AS id, COALESCE(CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END, COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], ''))) AS name, 'target' AS type, COUNT(*) AS runs, COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, AVG(duration_ns) AS avg_duration_ns FROM spans WHERE COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')) IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) GROUP BY 1, 2;

-- tumult-api/src/handlers/topology.rs:service_edges_sql(&cwindow)
SELECT 'svc:' || replace(replace(replace(p.service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS from_id, 'svc:' || replace(replace(replace(c.service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS to_id, COUNT(*) AS weight FROM spans c JOIN spans p ON c.parent_span_id = p.span_id WHERE (p.service_name <> c.service_name OR p.span_name <> c.span_name) AND c.ts_ns >= 1774900000000000000 AND c.ts_ns < 1774986400000000000 AND p.ts_ns >= 1774896400000000000 AND p.ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = c.trace_id AND se.target_environment IN ('prod')) GROUP BY 1, 2;

-- tumult-api/src/handlers/topology.rs:target_edges_sql(&window)
SELECT 'svc:' || replace(replace(replace(service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS from_id, CASE WHEN CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END IS NOT NULL THEN 'cmp:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') || '/' || replace(replace(replace(CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END, '%', '%25'), '/', '%2F'), '|', '%7C') ELSE 'sys:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') END AS to_id, COUNT(*) AS weight FROM spans WHERE COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')) IS NOT NULL AND service_name <> '' AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) GROUP BY 1, 2;

-- tumult-api/src/handlers/topology.rs:service_edges_sql(&copen)
SELECT 'svc:' || replace(replace(replace(p.service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS from_id, 'svc:' || replace(replace(replace(c.service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS to_id, COUNT(*) AS weight FROM spans c JOIN spans p ON c.parent_span_id = p.span_id WHERE (p.service_name <> c.service_name OR p.span_name <> c.span_name) AND c.ts_ns >= 1774900000000000000 AND c.ts_ns < 1774986400000000000 AND p.ts_ns >= 1774896400000000000 AND p.ts_ns < 1774986400000000000 GROUP BY 1, 2;

-- tumult-api/src/handlers/topology.rs:target_nodes_sql(&open)
SELECT CASE WHEN CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END IS NOT NULL THEN 'cmp:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') || '/' || replace(replace(replace(CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END, '%', '%25'), '/', '%2F'), '|', '%7C') ELSE 'sys:' || CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END || '/' || replace(replace(replace(COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')), '%', '%25'), '/', '%2F'), '|', '%7C') END AS id, COALESCE(CASE WHEN NULLIF(target_system, '') IS NOT NULL THEN NULLIF(resource_attrs['resilience.target.component'], '') END, COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], ''))) AS name, 'target' AS type, COUNT(*) AS runs, COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, AVG(duration_ns) AS avg_duration_ns FROM spans WHERE COALESCE(NULLIF(target_system, ''), NULLIF(span_attrs['resilience.target.name'], '')) IS NOT NULL AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY 1, 2;

-- tumult-api/src/handlers/traces.rs:traces_sql("TRUE", "")
SELECT t.trace_id, t.started_ns, t.duration_ns, t.span_count, t.error_count, COALESCE(r.span_name, t.first_span) AS root_name, COALESCE(r.service_name, t.first_service) AS service_name, t.experiment_id, t.experiment_name, lower(coalesce(t.outcome_status, l.log_attrs['status'])) AS status FROM ( SELECT trace_id, MIN(ts_ns) AS started_ns, MAX(ts_ns + duration_ns) - MIN(ts_ns) AS duration_ns, COUNT(*) AS span_count, COUNT(*) FILTER (WHERE status_code = 'Error') AS error_count, MAX(experiment_id) AS experiment_id, MAX(experiment_name) AS experiment_name, MAX(outcome_status) AS outcome_status, arg_min(span_name, ts_ns) AS first_span, arg_min(service_name, ts_ns) AS first_service FROM spans WHERE TRUE GROUP BY trace_id ) t LEFT JOIN ( SELECT trace_id, span_name, service_name, ROW_NUMBER() OVER (PARTITION BY trace_id ORDER BY ts_ns) AS rn FROM spans WHERE parent_span_id IS NULL ) r ON r.trace_id = t.trace_id AND r.rn = 1 LEFT JOIN run_completions l ON t.experiment_id = l.experiment_id  ORDER BY t.started_ns DESC LIMIT 200;

-- tumult-api/src/handlers/traces.rs:traces_sql(&w(cur.0, cur.1), &trace_where)
SELECT t.trace_id, t.started_ns, t.duration_ns, t.span_count, t.error_count, COALESCE(r.span_name, t.first_span) AS root_name, COALESCE(r.service_name, t.first_service) AS service_name, t.experiment_id, t.experiment_name, lower(coalesce(t.outcome_status, l.log_attrs['status'])) AS status FROM ( SELECT trace_id, MIN(ts_ns) AS started_ns, MAX(ts_ns + duration_ns) - MIN(ts_ns) AS duration_ns, COUNT(*) AS span_count, COUNT(*) FILTER (WHERE status_code = 'Error') AS error_count, MAX(experiment_id) AS experiment_id, MAX(experiment_name) AS experiment_name, MAX(outcome_status) AS outcome_status, arg_min(span_name, ts_ns) AS first_span, arg_min(service_name, ts_ns) AS first_service FROM spans WHERE ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 GROUP BY trace_id ) t LEFT JOIN ( SELECT trace_id, span_name, service_name, ROW_NUMBER() OVER (PARTITION BY trace_id ORDER BY ts_ns) AS rn FROM spans WHERE parent_span_id IS NULL ) r ON r.trace_id = t.trace_id AND r.rn = 1 LEFT JOIN run_completions l ON t.experiment_id = l.experiment_id WHERE EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = t.trace_id AND se.target_environment IN ('prod')) AND t.duration_ns >= 5000000 AND EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id AND sx.service_name = 'checkout') AND lower(coalesce(t.outcome_status, l.log_attrs['status'])) = 'failed' ORDER BY t.started_ns DESC LIMIT 200;

-- tumult-api/src/handlers/traces.rs:traces_sql("TRUE", &format!("WHERE {}", attr_trace_sql("http.method", "GET", true)))
SELECT t.trace_id, t.started_ns, t.duration_ns, t.span_count, t.error_count, COALESCE(r.span_name, t.first_span) AS root_name, COALESCE(r.service_name, t.first_service) AS service_name, t.experiment_id, t.experiment_name, lower(coalesce(t.outcome_status, l.log_attrs['status'])) AS status FROM ( SELECT trace_id, MIN(ts_ns) AS started_ns, MAX(ts_ns + duration_ns) - MIN(ts_ns) AS duration_ns, COUNT(*) AS span_count, COUNT(*) FILTER (WHERE status_code = 'Error') AS error_count, MAX(experiment_id) AS experiment_id, MAX(experiment_name) AS experiment_name, MAX(outcome_status) AS outcome_status, arg_min(span_name, ts_ns) AS first_span, arg_min(service_name, ts_ns) AS first_service FROM spans WHERE TRUE GROUP BY trace_id ) t LEFT JOIN ( SELECT trace_id, span_name, service_name, ROW_NUMBER() OVER (PARTITION BY trace_id ORDER BY ts_ns) AS rn FROM spans WHERE parent_span_id IS NULL ) r ON r.trace_id = t.trace_id AND r.rn = 1 LEFT JOIN run_completions l ON t.experiment_id = l.experiment_id WHERE NOT EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id AND (sx.span_attrs['http.method'] = 'GET' OR sx.resource_attrs['http.method'] = 'GET')) ORDER BY t.started_ns DESC LIMIT 200;

-- tumult-api/src/handlers/traces.rs:durations_sql(&durations_where(&[], None))
SELECT trace_id, ts_ns, duration_ns / 1000000.0 AS duration_ms FROM spans WHERE parent_span_id IS NULL ORDER BY ts_ns LIMIT 1000;

-- tumult-api/src/handlers/traces.rs:durations_sql(&durations_where(&prod, Some(cur)))
SELECT trace_id, ts_ns, duration_ns / 1000000.0 AS duration_ms FROM spans WHERE parent_span_id IS NULL AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000 ORDER BY ts_ns LIMIT 1000;

-- tumult-api/src/handlers/traces.rs:quantiles_sql(&durations_where(&prod, Some(cur)))
SELECT quantile_cont(duration_ns, 0.5) / 1000000.0 AS p50, quantile_cont(duration_ns, 0.95) / 1000000.0 AS p95, quantile_cont(duration_ns, 0.99) / 1000000.0 AS p99 FROM spans WHERE parent_span_id IS NULL AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) AND ts_ns >= 1774900000000000000 AND ts_ns < 1774986400000000000;

-- tumult-api/src/handlers/traces.rs:trace_spans_sql(&sql_string("trace-1"), "")
SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, duration_ns, status_code, status_message, service_name, experiment_id, experiment_name, fault_type, fault_subtype, span_attrs, events FROM spans WHERE trace_id = 'trace-1' ORDER BY ts_ns LIMIT 2000;

-- tumult-api/src/handlers/traces.rs:trace_spans_sql(&sql_string("trace-1"), &env)
SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, duration_ns, status_code, status_message, service_name, experiment_id, experiment_name, fault_type, fault_subtype, span_attrs, events FROM spans WHERE trace_id = 'trace-1' AND EXISTS (SELECT 1 FROM spans se WHERE se.trace_id = spans.trace_id AND se.target_environment IN ('prod')) ORDER BY ts_ns LIMIT 2000;

-- tumult-api/src/handlers/traces.rs:trace_logs_sql(&sql_string("trace-1"))
SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs FROM logs WHERE trace_id = 'trace-1' ORDER BY ts_ns LIMIT 1000;

-- tumult-api/src/runs/control.rs:haltable_sql(&[])
SELECT id FROM runs WHERE state IN ('queued', 'validating', 'running', 'stopping', 'pending_approval');

-- tumult-api/src/runs/control.rs:haltable_sql(&["prod".to_string()])
SELECT r.id FROM runs r LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.state IN ('queued', 'validating', 'running', 'stopping', 'pending_approval') AND (COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod'));

-- tumult-api/src/runs/read.rs:runs_sql(&[], &clauses, 100)
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE TRUE AND r.state = 'running' AND r.gameday_id = 'run-0'  ORDER BY r.queued_at_ns DESC LIMIT 100;

-- tumult-api/src/runs/read.rs:runs_sql(&prod, &clauses, 100)
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod') AND r.state = 'running' AND r.gameday_id = 'run-0'  ORDER BY r.queued_at_ns DESC LIMIT 100;

-- tumult-api/src/runs/read.rs:run_sql("run-1", &predicate)
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.id = 'run-1' AND (COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod'));

-- tumult-api/src/runs/read.rs:run_id_sql("run-1", &predicate)
SELECT r.id FROM runs r LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id WHERE r.id = 'run-1' AND (COALESCE((SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ('prod'));

-- tumult-api/src/sql_util.rs:run_outcome_where("incomplete", "s", "l").expect("an outcome")
SELECT count(*) AS n FROM run_roots AS s LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id WHERE lower(coalesce(s.outcome_status, l.log_attrs['status'])) IS NULL;

-- tumult-api/src/sql_util.rs:in_scopes(1, "target_environment")
SELECT count(*) AS n FROM run_roots WHERE ($1 IS NULL OR list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), target_environment));

-- tumult-api/src/sql_util.rs:id_segment_sql("service_name")
SELECT replace(replace(replace(service_name, '%', '%25'), '/', '%2F'), '|', '%7C') AS id FROM spans;

-- tumult-api/src/sql_util.rs:env_segment_sql("target_environment")
SELECT CASE WHEN target_environment IS NULL OR target_environment = '' THEN '-' WHEN target_environment = '-' THEN '%2D' ELSE replace(replace(replace(target_environment, '%', '%25'), '/', '%2F'), '|', '%7C') END AS env FROM spans;

-- tumult-api/src/sql_util.rs:bucket_of("s.ts_ns", 3_600)
SELECT (s.ts_ns // 3600000000000) * 3600000000000 // 1000000000 AS ts FROM spans AS s;

-- tumult-api/src/sql_util.rs:attr_wheres(Some("k=v"), Some("a=b"), ("log_attrs", "resource_attrs")).expect("valid filters").join(" AND ")
SELECT count(*) AS n FROM logs WHERE (log_attrs['k'] = 'v' OR resource_attrs['k'] = 'v') AND NOT (COALESCE(log_attrs['a'] = 'b', false) OR COALESCE(resource_attrs['a'] = 'b', false));
