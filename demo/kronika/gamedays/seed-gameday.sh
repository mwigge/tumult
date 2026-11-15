#!/bin/sh
# Krönika demo: register the platform GameDay, bind its four experiments to
# the `dev` environment, and run one campaign that a second person approves.
#
# Why the binding: the daemon treats every unbound execution as tier T3, and
# the approval queue cannot release a T3 run that an operator requested (the
# autopilot gate never enacts it). An administrator must first review each
# experiment's execution fingerprint and bind it. In this demo the seed is
# that administrator: the four experiments are the shipped files in
# demo/kronika/experiments, reviewed with this repository. The binding only
# restores normal tiering (T1/T2 here); approvals still need a second person
# (bob), and self-approval is still refused.
#
# Usage: seed-gameday.sh <api-base> <admin-auth-header> <approver-auth-header>
# Env:   TUMULT_LAKE_PATH  daemon store path; the bindings file is written next
#                          to it (<store>.execution-bindings.json, which the
#                          daemon reads on every classification)
#        GAMEDAY_FILE      default /gamedays/platform-resilience.gameday.toon
#        EXPERIMENTS_DIR   default /experiments
#        GAMEDAY_ENV       default dev (the env the GameDays page launches in)
#        GAMEDAY_WAIT_S    default 300 (campaign deadline)
set -eu

API=$1
AUTH=$2
APPROVER_AUTH=$3
GAMEDAY_FILE=${GAMEDAY_FILE:-/gamedays/platform-resilience.gameday.toon}
EXPERIMENTS_DIR=${EXPERIMENTS_DIR:-/experiments}
GAMEDAY_ENV=${GAMEDAY_ENV:-dev}
GAMEDAY_WAIT_S=${GAMEDAY_WAIT_S:-300}
BINDINGS=${TUMULT_LAKE_PATH%.duckdb}.execution-bindings.json
CT='content-type: application/json'
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail() {
  echo "=== seed: gameday FAILED: $*" >&2
  exit 1
}

# One API call; prints the body on 2xx, fails loudly with status and body
# otherwise. $1 method, $2 path, $3 auth header, $4 optional body file.
call() {
  if [ -n "${4:-}" ]; then
    code=$(curl -s -o "$WORK/out" -w '%{http_code}' -X "$1" -H "$3" -H "$CT" \
      --data-binary "@$4" "$API$2") || fail "$1 $2: daemon unreachable"
  else
    code=$(curl -s -o "$WORK/out" -w '%{http_code}' -X "$1" -H "$3" "$API$2") \
      || fail "$1 $2: daemon unreachable"
  fi
  case $code in
    2*) cat "$WORK/out" ;;
    *) fail "$1 $2 -> HTTP $code: $(cat "$WORK/out")" ;;
  esac
}

# A file's content as a JSON string literal.
json_string() {
  printf '"%s\\n"' "$(sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g' -e 's/\r$//' "$1" \
    | sed -e ':a' -e 'N' -e '$!ba' -e 's/\n/\\n/g')"
}

# First value of a string field in a JSON body.
field() {
  grep -o "\"$1\":\"[^\"]*\"" | head -n1 | cut -d'"' -f4
}

# 1. Register the campaign and every experiment it references.
{
  printf '{"toon":%s,"experiments":{' "$(json_string "$GAMEDAY_FILE")"
  sep=''
  for path in $(sed -n 's/^ *- path: *//p' "$GAMEDAY_FILE"); do
    [ -f "$EXPERIMENTS_DIR/$path" ] || fail "experiment $path is missing from $EXPERIMENTS_DIR"
    printf '%s"%s":%s' "$sep" "$path" "$(json_string "$EXPERIMENTS_DIR/$path")"
    sep=','
  done
  printf '}}'
} > "$WORK/gameday.json"
call POST /gamedays/validate "$AUTH" "$WORK/gameday.json" > "$WORK/registered.json"
GID=$(field gameday_registry_id < "$WORK/registered.json")
[ -n "$GID" ] || fail "registration returned no gameday id: $(cat "$WORK/registered.json")"
echo "=== seed: registered gameday $GID"

# 2. Review and bind each step: the admin dry-run returns the private
#    execution fingerprint the daemon matches bindings against.
sep=''
printf '[' > "$WORK/bindings.json"
for rid in $(grep -o '"registry_id":"[^"]*"' "$WORK/registered.json" | cut -d'"' -f4); do
  printf '{"registry_id":"%s","env":"%s"}' "$rid" "$GAMEDAY_ENV" > "$WORK/dry.json"
  hash=$(call POST /runs/dry-run "$AUTH" "$WORK/dry.json" | field binding_hash)
  [ -n "$hash" ] || fail "no execution fingerprint for $rid (the seed token must be an unscoped admin)"
  printf '%s{"sha256":"%s","env":"%s","target":null}' "$sep" "$hash" "$GAMEDAY_ENV" >> "$WORK/bindings.json"
  sep=','
done
printf ']\n' >> "$WORK/bindings.json"
cp "$WORK/bindings.json" "$BINDINGS.tmp" && mv "$BINDINGS.tmp" "$BINDINGS" \
  || fail "cannot write the execution bindings file $BINDINGS"
echo "=== seed: bound the gameday experiments to env $GAMEDAY_ENV ($BINDINGS)"

# 3. Run one campaign: admin launches, the approver releases each step.
printf '{"env":"%s"}' "$GAMEDAY_ENV" > "$WORK/env.json"
CID=$(call POST "/gamedays/$GID/runs" "$AUTH" "$WORK/env.json" | field run_id)
[ -n "$CID" ] || fail "campaign start returned no run id"
echo "=== seed: campaign $CID started"
deadline=$(( $(date +%s) + GAMEDAY_WAIT_S ))
approved=0
while :; do
  state=$(call GET "/runs/$CID" "$AUTH" | field state)
  case $state in
    passed|deviated|failed|aborted|rejected|expired) break ;;
  esac
  [ "$(date +%s)" -lt "$deadline" ] || fail "campaign $CID still $state after ${GAMEDAY_WAIT_S}s"
  for rid in $(call GET /approvals "$APPROVER_AUTH" | grep -o '"run_id":"[^"]*"' | cut -d'"' -f4); do
    call GET "/runs/$rid" "$AUTH" | grep -q "\"gameday_id\":\"$CID\"" || continue
    printf '{"note":"demo seed: reviewed campaign step"}' > "$WORK/note.json"
    call POST "/runs/$rid/approve" "$APPROVER_AUTH" "$WORK/note.json" > /dev/null
    approved=$((approved + 1))
    echo "=== seed: approved campaign step $rid (second person)"
  done
  sleep 2
done
echo "=== seed: gameday campaign $CID -> $state ($approved steps approved)"
