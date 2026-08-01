#!/usr/bin/env bash
#
# signoz-bulk-import.sh — backfill Tumult lake spans (Parquet) into SigNoz's
# ClickHouse trace tables, bypassing the OTel collector.
#
# Reads '<lake>/spans/date=YYYY-MM-DD/data-*.parquet' and inserts into
# signoz_traces.signoz_index_v3 (via its Distributed wrapper when present)
# plus the companion resource table (traces_v3_resource) that the SigNoz UI
# needs to resolve services. Logs and metric_* tables are out of scope.
#
# Two transport modes:
#   docker  SIGNOZ_DOCKER_CONTAINER=<name> — SQL runs via `docker exec` and
#           each parquet file is staged into the container's user_files dir
#           with `docker cp`. Works with the stock SigNoz containers, no
#           extra mounts needed. This is the recommended mode.
#   local   clickhouse-client on this machine — the parquet files must be
#           readable by the ClickHouse *server* under its user_files
#           directory; set PARQUET_SERVER_DIR to the server-side path that
#           corresponds to KRONIKA_LAKE_DIR.
#
# Idempotency: signoz_index_v3 is a plain MergeTree (no dedup), so the
# script keeps a ledger of imported files (one line per file) and skips
# them on re-runs. --force re-imports everything and WILL duplicate rows.
#
# Usage:
#   scripts/signoz-bulk-import.sh [--dry-run] [--force]
#
# Config (env):
#   SIGNOZ_DOCKER_CONTAINER  docker exec target (enables docker mode)
#   SIGNOZ_CLICKHOUSE_DSN    clickhouse://[user[:pass]@]host[:port][/db]
#   CLICKHOUSE_HOST          default localhost (local mode)
#   CLICKHOUSE_PORT          default 9000
#   CLICKHOUSE_USER          default default
#   CLICKHOUSE_PASSWORD      default empty
#   CLICKHOUSE_DB            default signoz_traces
#   KRONIKA_LAKE_DIR         default ~/.tumult/lake
#   PARQUET_GLOB             explicit local glob, overrides the lake layout
#   PARQUET_SERVER_DIR       server-side path for KRONIKA_LAKE_DIR (local mode)
#   SIGNOZ_IMPORT_STATE      ledger path, default <lake>/.signoz-import-ledger
#
# Exit codes: 0 ok (or dry-run), 1 runtime failure, 2 usage/config error.

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LAKE_DIR="${KRONIKA_LAKE_DIR:-${HOME}/.tumult/lake}"
PARQUET_GLOB="${PARQUET_GLOB:-${LAKE_DIR}/spans/date=*/*.parquet}"
SERVER_DIR="${PARQUET_SERVER_DIR:-}"
STATE_FILE="${SIGNOZ_IMPORT_STATE:-${LAKE_DIR}/.signoz-import-ledger}"
DOCKER_CTR="${SIGNOZ_DOCKER_CONTAINER:-}"
DSN="${SIGNOZ_CLICKHOUSE_DSN:-}"
CH_HOST="${CLICKHOUSE_HOST:-localhost}"
CH_PORT="${CLICKHOUSE_PORT:-9000}"
CH_USER="${CLICKHOUSE_USER:-default}"
CH_PASSWORD="${CLICKHOUSE_PASSWORD:-}"
CH_DB="${CLICKHOUSE_DB:-signoz_traces}"
DRY_RUN=0
FORCE=0

# ── Args ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)  DRY_RUN=1; shift ;;
    --force)    FORCE=1; shift ;;
    -h|--help)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# ── Output helpers ────────────────────────────────────────────────
c_green=$'\033[32m'; c_red=$'\033[31m'; c_yellow=$'\033[33m'; c_reset=$'\033[0m'
pass() { printf "  %sPASS%s  %s\n" "$c_green" "$c_reset" "$1"; }
fail() { printf "  %sFAIL%s  %s\n" "$c_red" "$c_reset" "$1"; }
warn() { printf "  %sWARN%s  %s\n" "$c_yellow" "$c_reset" "$1"; }
info() { printf "  ----  %s\n" "$1"; }
die()  { fail "$1" >&2; exit "${2:-1}"; }

# ── DSN parsing (clickhouse://[user[:pass]@]host[:port][/db]) ─────
if [[ -n "$DSN" ]]; then
  dsn_rest="${DSN#clickhouse://}"
  if [[ "$dsn_rest" == "$DSN" ]]; then
    die "SIGNOZ_CLICKHOUSE_DSN must start with clickhouse://" 2
  fi
  dsn_auth=""; dsn_hostport="$dsn_rest"
  if [[ "$dsn_rest" == *@* ]]; then
    dsn_auth="${dsn_rest%%@*}"; dsn_hostport="${dsn_rest#*@}"
    CH_USER="${dsn_auth%%:*}"
    if [[ "$dsn_auth" == *:* ]]; then CH_PASSWORD="${dsn_auth#*:}"; fi
  fi
  dsn_db=""
  if [[ "$dsn_hostport" == */* ]]; then
    dsn_db="${dsn_hostport#*/}"; dsn_hostport="${dsn_hostport%%/*}"
  fi
  CH_HOST="${dsn_hostport%%:*}"
  if [[ "$dsn_hostport" == *:* ]]; then CH_PORT="${dsn_hostport#*:}"; fi
  if [[ -n "$dsn_db" ]]; then CH_DB="$dsn_db"; fi
fi

# ── clickhouse-client wrapper ─────────────────────────────────────
# All SQL goes through ch_query; output on stdout, errors abort via -euo.
CH_ARGS=(--user "$CH_USER" --database "$CH_DB")
if [[ -n "$CH_PASSWORD" ]]; then CH_ARGS+=(--password "$CH_PASSWORD"); fi

ch_query() {
  if [[ -n "$DOCKER_CTR" ]]; then
    docker exec -i "$DOCKER_CTR" clickhouse-client "${CH_ARGS[@]}" --query "$1"
  else
    clickhouse-client --host "$CH_HOST" --port "$CH_PORT" "${CH_ARGS[@]}" --query "$1"
  fi
}

# Stage one local parquet file where the server can read it; prints the
# path to use inside file(). Docker mode: docker cp into user_files under
# a flattened name (ClickHouse file() globs do not cross directory levels,
# and the flat name keeps the 'date=' partition marker for dry-run output).
# Local mode: SERVER_DIR/<relative-path> (must sit under user_files).
STAGE_DIR="signoz-bulk-import-$$"
stage_file() {
  local src="$1" rel="$2" flat
  flat="${rel//\//__}"
  if [[ -n "$DOCKER_CTR" ]]; then
    docker exec "$DOCKER_CTR" mkdir -p "/var/lib/clickhouse/user_files/${STAGE_DIR}" >/dev/null
    docker cp "$src" "$DOCKER_CTR:/var/lib/clickhouse/user_files/${STAGE_DIR}/${flat}" >/dev/null
    printf '%s/%s' "$STAGE_DIR" "$flat"
  else
    [[ -n "$SERVER_DIR" ]] || die "local mode needs PARQUET_SERVER_DIR (server-side path under user_files that maps to $LAKE_DIR)" 2
    printf '%s/%s' "${SERVER_DIR%/}" "$rel"
  fi
}

unstage_all() {
  [[ -n "$DOCKER_CTR" ]] || return 0
  docker exec "$DOCKER_CTR" rm -rf "/var/lib/clickhouse/user_files/${STAGE_DIR}" >/dev/null 2>&1 || true
}
trap unstage_all EXIT

# ── SQL fragments ─────────────────────────────────────────────────
# One SELECT mapped onto signoz_index_v3's insert columns (positional, so
# the SELECT aliases only need to be unique — output aliases that would
# shadow source columns get an _o suffix to avoid cyclic-alias errors).
# __REF__ is replaced with the server-side parquet reference. Materialized
# resilience dims are re-emitted as span attributes under their lake column
# names; a 'tumult.import' marker attribute identifies backfilled rows.
# shellcheck disable=SC2016
MAPPED_SELECT='
WITH
  mapConcat(CAST(resource_attrs AS Map(String, String)),
    mapFilter((k, v) -> v != '"''"', map(
      '"'"'service.name'"'"', coalesce(service_name, '"''"'),
      '"'"'service.version'"'"', coalesce(service_version, '"''"')))) AS res0,
  if(mapContains(res0, '"'"'service.name'"'"'), res0,
    mapUpdate(res0, map('"'"'service.name'"'"', '"'"'unknown'"'"'))) AS res,
  toJSONString(mapSort(res)) AS labels,
  multiIf(lowerUTF8(coalesce(span_kind, '"''"')) = '"'"'server'"'"', 2,
          lowerUTF8(coalesce(span_kind, '"''"')) = '"'"'client'"'"', 3,
          lowerUTF8(coalesce(span_kind, '"''"')) = '"'"'producer'"'"', 4,
          lowerUTF8(coalesce(span_kind, '"''"')) = '"'"'consumer'"'"', 5,
          lowerUTF8(coalesce(span_kind, '"''"')) = '"'"'internal'"'"', 1, 0) AS kind_v,
  multiIf(lowerUTF8(coalesce(status_code, '"''"')) = '"'"'ok'"'"', 1,
          lowerUTF8(coalesce(status_code, '"''"')) = '"'"'error'"'"', 2, 0) AS status_v
SELECT
  toUInt64(intDiv(coalesce(ts_ns, 0), 1800000000000) * 1800) AS ts_bucket_start,
  concat('"'"'service.name='"'"', res['"'"'service.name'"'"'],
         '"'"';host.name='"'"', if(res['"'"'host.name'"'"'] = '"''"', '"'"'unknown'"'"', res['"'"'host.name'"'"']),
         '"'"';hash='"'"', toString(cityHash64(labels))) AS resource_fingerprint,
  fromUnixTimestamp64Nano(coalesce(ts_ns, 0)) AS timestamp,
  substring(coalesce(trace_id, '"''"'), 1, 32) AS trace_id_o,
  coalesce(span_id, '"''"') AS span_id_o,
  coalesce(parent_span_id, '"''"') AS parent_span_id_o,
  coalesce(span_name, '"''"') AS name,
  kind_v AS kind,
  multiIf(kind_v = 2, '"'"'Server'"'"', kind_v = 3, '"'"'Client'"'"', kind_v = 4, '"'"'Producer'"'"',
          kind_v = 5, '"'"'Consumer'"'"', kind_v = 1, '"'"'Internal'"'"', '"'"'Unspecified'"'"') AS kind_string,
  toUInt64(greatest(coalesce(duration_ns, 0), 0)) AS duration_nano,
  status_v AS status_code_o,
  coalesce(status_message, '"''"') AS status_message_o,
  multiIf(status_v = 1, '"'"'Ok'"'"', status_v = 2, '"'"'Error'"'"', '"'"'Unset'"'"') AS status_code_string,
  mapConcat(CAST(span_attrs AS Map(String, String)),
    mapFilter((k, v) -> v != '"''"', map(
      '"'"'experiment_id'"'"', coalesce(experiment_id, '"''"'),
      '"'"'experiment_name'"'"', coalesce(experiment_name, '"''"'),
      '"'"'outcome_status'"'"', coalesce(outcome_status, '"''"'),
      '"'"'fault_type'"'"', coalesce(fault_type, '"''"'),
      '"'"'fault_subtype'"'"', coalesce(fault_subtype, '"''"'),
      '"'"'fault_severity'"'"', coalesce(fault_severity, '"''"'),
      '"'"'blast_radius'"'"', coalesce(blast_radius, '"''"'),
      '"'"'target_system'"'"', coalesce(target_system, '"''"'),
      '"'"'target_technology'"'"', coalesce(target_technology, '"''"'),
      '"'"'target_environment'"'"', coalesce(target_environment, '"''"'),
      '"'"'plugin_name'"'"', coalesce(plugin_name, '"''"'),
      '"'"'tumult.import'"'"', '"'"'signoz-bulk-import'"'"'))) AS attributes_string,
  if(recovery_time_s IS NULL, CAST(map(), '"'"'Map(String, Float64)'"'"'),
    map('"'"'recovery_time_s'"'"', recovery_time_s)) AS attributes_number,
  if(hypothesis_met IS NULL, CAST(map(), '"'"'Map(String, Bool)'"'"'),
    map('"'"'hypothesis_met'"'"', hypothesis_met)) AS attributes_bool,
  res AS resources_string,
  if(coalesce(events, '"''"') IN ('"''"', '"'"'[]'"'"'), CAST('"'"'[]'"'"', '"'"'Array(String)'"'"'),
    JSONExtractArrayRaw(coalesce(events, '"''"'))) AS events_o,
  status_v = 2 AS has_error,
  '"'"'no'"'"' AS is_remote
FROM file('"'"'__REF__'"'"', Parquet)
'

INDEX_COLS="(ts_bucket_start, resource_fingerprint, timestamp, trace_id, span_id, parent_span_id, name, kind, kind_string, duration_nano, status_code, status_message, status_code_string, attributes_string, attributes_number, attributes_bool, resources_string, events, has_error, is_remote)"

# Resource-table insert: one row per distinct resource in the file.
# shellcheck disable=SC2016
RESOURCE_INSERT_SUFFIX='
WITH
  mapConcat(CAST(resource_attrs AS Map(String, String)),
    mapFilter((k, v) -> v != '"''"', map(
      '"'"'service.name'"'"', coalesce(service_name, '"''"'),
      '"'"'service.version'"'"', coalesce(service_version, '"''"')))) AS res0,
  if(mapContains(res0, '"'"'service.name'"'"'), res0,
    mapUpdate(res0, map('"'"'service.name'"'"', '"'"'unknown'"'"'))) AS res,
  toJSONString(mapSort(res)) AS labels
SELECT DISTINCT
  labels,
  concat('"'"'service.name='"'"', res['"'"'service.name'"'"'],
         '"'"';host.name='"'"', if(res['"'"'host.name'"'"'] = '"''"', '"'"'unknown'"'"', res['"'"'host.name'"'"']),
         '"'"';hash='"'"', toString(cityHash64(labels))) AS fingerprint,
  toInt64(intDiv(coalesce(ts_ns, 0), 1800000000000) * 1800) AS seen_at_ts_bucket_start
FROM file('"'"'__REF__'"'"', Parquet)
'

# ── Preflight ─────────────────────────────────────────────────────
shopt -s nullglob
# Expand the local glob (handles both the lake layout and PARQUET_GLOB).
FILES=( $PARQUET_GLOB )
shopt -u nullglob
[[ ${#FILES[@]} -gt 0 ]] || die "no parquet files matched: $PARQUET_GLOB (set KRONIKA_LAKE_DIR or PARQUET_GLOB)" 2

if [[ -n "$DOCKER_CTR" ]]; then
  docker inspect "$DOCKER_CTR" >/dev/null 2>&1 || die "docker container not found: $DOCKER_CTR" 2
else
  command -v clickhouse-client >/dev/null 2>&1 || die "clickhouse-client not found; install it or set SIGNOZ_DOCKER_CONTAINER" 2
fi

ch_query "SELECT 1" >/dev/null 2>&1 || die "cannot reach ClickHouse (check SIGNOZ_CLICKHOUSE_DSN / CLICKHOUSE_HOST / SIGNOZ_DOCKER_CONTAINER)"

# Prefer the Distributed wrappers (cluster-safe); fall back to local tables.
INDEX_TABLE="signoz_index_v3"; RESOURCE_TABLE="traces_v3_resource"
if [[ "$(ch_query "SELECT count() FROM system.tables WHERE database='${CH_DB}' AND name='distributed_signoz_index_v3'")" == "1" ]]; then
  INDEX_TABLE="distributed_signoz_index_v3"
fi
if [[ "$(ch_query "SELECT count() FROM system.tables WHERE database='${CH_DB}' AND name='distributed_traces_v3_resource'")" == "1" ]]; then
  RESOURCE_TABLE="distributed_traces_v3_resource"
fi
[[ "$(ch_query "SELECT count() FROM system.tables WHERE database='${CH_DB}' AND name IN ('${INDEX_TABLE}','${RESOURCE_TABLE}')")" == "2" ]] \
  || die "target tables not found in ${CH_DB} (${INDEX_TABLE}, ${RESOURCE_TABLE}); is this a SigNoz ClickHouse?"

# Ledger: one '<relpath>\t<rows>\t<iso-time>' line per imported file.
touch "$STATE_FILE"
is_imported() {
  [[ $FORCE -eq 0 ]] || return 1
  awk -F '\t' -v r="$1" '$1 == r { found=1 } END { exit !found }' "$STATE_FILE"
}

# Base dir for relative paths: the lake root, or the glob's static prefix.
BASE_DIR="$LAKE_DIR"
if [[ "$PARQUET_GLOB" != "${LAKE_DIR}/spans/date=*/*.parquet" ]]; then
  BASE_DIR="${PARQUET_GLOB%%\**}"; BASE_DIR="${BASE_DIR%/}"
fi
rel_path() { local p="${1#$BASE_DIR/}"; printf '%s' "${p#/}"; }

# ── Dry run ───────────────────────────────────────────────────────
echo "=================================================================="
echo " SigNoz bulk import — $([[ $DRY_RUN -eq 1 ]] && echo 'DRY RUN' || echo 'IMPORT')"
echo " source:  $PARQUET_GLOB"
echo " target:  ${CH_DB}.${INDEX_TABLE} + ${CH_DB}.${RESOURCE_TABLE}"
echo " mode:    $([[ -n "$DOCKER_CTR" ]] && echo "docker exec ($DOCKER_CTR)" || echo "clickhouse-client ($CH_HOST:$CH_PORT)")"
echo "=================================================================="

if [[ $DRY_RUN -eq 1 ]]; then
  # Count each file server-side and aggregate per date partition in bash.
  # Per-file queries keep this identical in docker and local modes.
  declare -A part_rows=() part_files=()
  pending=0; done_ct=0; total=0
  for f in "${FILES[@]}"; do
    rel="$(rel_path "$f")"
    part="$(basename "$(dirname "$f")")"
    [[ "$part" == date=* ]] || part="(no partition)"
    ref="$(stage_file "$f" "$rel")"
    rows="$(ch_query "SELECT count() FROM file('${ref}', Parquet)")"
    part_rows[$part]=$(( ${part_rows[$part]:-0} + rows ))
    part_files[$part]=$(( ${part_files[$part]:-0} + 1 ))
    total=$((total + rows))
    if is_imported "$rel"; then done_ct=$((done_ct + 1)); else pending=$((pending + 1)); fi
  done
  for part in "${!part_rows[@]}"; do
    printf "  %-18s %8d rows  (%d file(s))\n" "$part" "${part_rows[$part]}" "${part_files[$part]}"
  done | sort
  echo ""
  info "files: ${#FILES[@]} total ($total rows), $pending pending, $done_ct already in ledger ($STATE_FILE)"
  if [[ $FORCE -eq 1 ]]; then warn "--force: ledger ignored, re-import duplicates rows"; fi
  exit 0
fi

# ── Import ────────────────────────────────────────────────────────
imported_files=0; imported_rows=0; skipped=0
for f in "${FILES[@]}"; do
  rel="$(rel_path "$f")"
  if is_imported "$rel"; then
    skipped=$((skipped + 1)); continue
  fi
  ref="$(stage_file "$f" "$rel")"
  rows="$(ch_query "SELECT count() FROM file('${ref}', Parquet)")"
  ch_query "INSERT INTO ${CH_DB}.${INDEX_TABLE} ${INDEX_COLS} ${MAPPED_SELECT//__REF__/$ref}"
  ch_query "INSERT INTO ${CH_DB}.${RESOURCE_TABLE} (labels, fingerprint, seen_at_ts_bucket_start) ${RESOURCE_INSERT_SUFFIX//__REF__/$ref}"
  printf '%s\t%s\t%s\n' "$rel" "$rows" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$STATE_FILE"
  pass "$rel  ($rows rows)"
  imported_files=$((imported_files + 1)); imported_rows=$((imported_rows + rows))
done

echo ""
echo "------------------------------------------------------------------"
pass "imported $imported_rows rows from $imported_files file(s); skipped $skipped already-ledgered"
if [[ $skipped -gt 0 ]]; then info "use --force to re-import ledgered files (duplicates rows!)"; fi
echo "------------------------------------------------------------------"
