#!/usr/bin/env bash
# import-dashboards.sh
#
# Imports all Tumult SigNoz dashboards via the SigNoz HTTP API.
#
# Usage:
#   ./import-dashboards.sh [SIGNOZ_URL]
#
# Defaults to http://localhost:13301 (the Tumult observability stack port).
# SigNoz does not support file-based provisioning, so this script must be
# run once after `docker compose up` to load all dashboards.
#
# Requirements: curl, jq (optional — used for pretty-printing responses)

set -euo pipefail

SIGNOZ_URL="${1:-http://localhost:13301}"
API_URL="${SIGNOZ_URL}/api/v1/dashboards"
DASHBOARD_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Importing Tumult dashboards to SigNoz at: ${SIGNOZ_URL}"
echo ""

success=0
failed=0

for f in "${DASHBOARD_DIR}"/*.json; do
  name="$(basename "$f")"
  printf "  Importing %-55s ... " "$name"

  http_code=$(curl \
    --silent \
    --output /tmp/signoz-import-response.json \
    --write-out "%{http_code}" \
    --request POST \
    --header "Content-Type: application/json" \
    --data "@${f}" \
    "${API_URL}")

  if [[ "$http_code" == "200" || "$http_code" == "201" ]]; then
    echo "OK (HTTP ${http_code})"
    success=$((success + 1))
  else
    echo "FAILED (HTTP ${http_code})"
    if command -v jq &>/dev/null; then
      jq -r '.error // .message // "unknown error"' /tmp/signoz-import-response.json 2>/dev/null || true
    fi
    failed=$((failed + 1))
  fi
done

echo ""
echo "Done: ${success} imported, ${failed} failed."

if [[ $failed -gt 0 ]]; then
  echo ""
  echo "Tip: If SigNoz is not yet ready, wait 30s and retry."
  echo "     If authentication is required, add -H 'Authorization: Bearer <token>' to the curl command."
  exit 1
fi
