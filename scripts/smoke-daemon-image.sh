#!/usr/bin/env bash
# Validate an already-built daemon image without host ports or external networking.
# Usage: scripts/smoke-daemon-image.sh IMAGE
# Requires Docker, Bash, and GNU coreutils (the release CI runs on Ubuntu).
set -euo pipefail
# Never inherit xtrace: credentials must not appear in CI logs.
set +x
umask 077

if [[ $# != 1 || -z "$1" || "$1" == -* ]]; then
  echo "usage: $0 IMAGE" >&2
  exit 2
fi
image=$1
wait_seconds=${TUMULT_SMOKE_TIMEOUT_SECONDS:-90}
if [[ ! "$wait_seconds" =~ ^[1-9][0-9]*$ ]] || (( wait_seconds > 300 )); then
  echo 'TUMULT_SMOKE_TIMEOUT_SECONDS must be between 1 and 300' >&2
  exit 2
fi
for dependency in docker timeout od mktemp; do
  command -v "$dependency" >/dev/null || { echo "missing dependency: $dependency" >&2; exit 2; }
done

scratch=$(mktemp -d)
resource="tumult-smoke-${scratch##*/}"
first="${resource}-first"
second="${resource}-second"
volume="${resource}-data"
volume_requested=false
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  timeout 20 docker rm --force "$first" "$second" >/dev/null 2>&1 || true
  if [[ "$volume_requested" == true ]]; then
    if ! timeout 20 docker volume rm "$volume" >/dev/null 2>&1; then
      echo "failed to clean up owned smoke volume: $volume" >&2
      result=1
    fi
  fi
  rm -rf -- "$scratch"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

random_secret() { od -An -N32 -tx1 /dev/urandom | tr -d ' \n'; }
api_token="kro_$(random_secret)"
ingest_token="kro_$(random_secret)"
password=$(random_secret)
# Env files keep secrets out of Docker argv and survive neither success nor failure.
cat > "$scratch/runtime.env" <<ENV
TUMULT_LAKE_PATH=/data/lake.duckdb
KRONIKA_OTLP_HTTP_ADDR=0.0.0.0:4318
KRONIKA_OTLP_GRPC_ADDR=0.0.0.0:4317
KRONIKA_RETENTION_DAYS=0
TUMULTD_RUN_RETENTION_DAYS=0
KRONIKA_INGEST_TOKEN=$ingest_token
TUMULT_SMOKE_API_TOKEN=$api_token
ENV
cp "$scratch/runtime.env" "$scratch/bootstrap.env"
printf 'KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=%s\nKRONIKA_BOOTSTRAP_TOKEN=%s\n' \
  "$password" "$api_token" >> "$scratch/bootstrap.env"
unset api_token ingest_token password

# The daemon may create a volume even if the Docker client later times out.
volume_requested=true
timeout 20 docker volume create "$volume" >/dev/null
start_container() {
  timeout 120 docker run --detach --name "$1" \
    --user 10001:10001 --read-only --network none \
    --cap-drop ALL --security-opt no-new-privileges \
    --tmpfs /tmp:rw,nosuid,nodev,size=128m,mode=1777 \
    --mount "type=volume,source=$volume,target=/data" \
    --env-file "$2" "$image" >/dev/null
}

probe() {
  timeout 25 docker exec --user 10001:10001 "$1" sh -ec '
    test "$(id -u)" = 10001
    test -s /data/lake.duckdb
    test -s "$KRONIKA_METRICS_DIR/hypothesis_pass_rate.yaml"
    test -x "$(command -v tumult-net-proxyd)"
    request() {
      printf "header = \"Authorization: Bearer %s\"\n" "$TUMULT_SMOKE_API_TOKEN" |
        curl --config - --fail --silent --max-time 5 "http://127.0.0.1:4318$1"
    }
    request /readyz >/dev/null
    request /healthz >/dev/null
    identity=$(request /api/me)
    printf "%s" "$identity" | grep -Eq "\"auth_required\"[[:space:]]*:[[:space:]]*true"
    printf "%s" "$identity" | grep -Eq "\"authenticated\"[[:space:]]*:[[:space:]]*true"
    printf "%s" "$identity" | grep -Eq "\"username\"[[:space:]]*:[[:space:]]*\"admin\""
    request /api/metrics | grep -q hypothesis_pass_rate
  ' >/dev/null 2>&1
}
wait_ready() {
  local deadline=$((SECONDS + wait_seconds))
  while (( SECONDS < deadline )); do
    if probe "$1"; then return 0; fi
    if [[ $(timeout 10 docker inspect --format '{{.State.Running}}' "$1" 2>/dev/null) != true ]]; then
      echo 'daemon exited before authenticated readiness' >&2
      return 1
    fi
    sleep 1
  done
  echo 'timed out waiting for authenticated daemon readiness' >&2
  return 1
}

start_container "$first" "$scratch/bootstrap.env"
wait_ready "$first"
timeout 20 docker stop --time 10 "$first" >/dev/null
timeout 20 docker rm "$first" >/dev/null
# Fresh container, same volume, and no identity-provisioning inputs.
start_container "$second" "$scratch/runtime.env"
timeout 10 docker exec "$second" sh -ec \
  'test -z "${KRONIKA_BOOTSTRAP_ADMIN_PASSWORD:-}${KRONIKA_BOOTSTRAP_TOKEN:-}"' >/dev/null
wait_ready "$second"
echo 'daemon image smoke passed: authenticated health, bundled tools and persisted identity'
