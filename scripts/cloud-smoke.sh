#!/usr/bin/env bash
#
# cloud-smoke.sh — exercise ONE safe, read-only-ish cloud connector call end to
# end against a REAL provider. This is NOT run in CI: it requires real cloud
# credentials and an existing FIS experiment (or falls back to a status read).
#
# The hermetic mocked-HTTP tests (tumult-cloud/tests/hermetic.rs) are the
# primary validation. This script exists so a human with credentials can prove
# the wire actually reaches AWS.
#
# Usage:
#   AWS_ACCESS_KEY_ID=...  AWS_SECRET_ACCESS_KEY=...  AWS_REGION=us-east-1 \
#     FIS_EXPERIMENT_ID=EXP0123456789abcdef  ./scripts/cloud-smoke.sh
#
# What it does (read-only, no fault is injected):
#   * If FIS_EXPERIMENT_ID is set: runs a single-step experiment that calls
#     aws_fis_experiment_status (GetExperiment) — a pure read.
#   * Otherwise: prints how to list templates with the AWS CLI and exits 0.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUMULT="${TUMULT_BIN:-cargo run -q -p tumult-cli --}"

require_env() {
  local name="$1"
  if [ -z "${!name:-}" ]; then
    echo "error: environment variable ${name} is required" >&2
    exit 1
  fi
}

echo "== tumult-cloud smoke test (real AWS) =="

require_env AWS_ACCESS_KEY_ID
require_env AWS_SECRET_ACCESS_KEY
: "${AWS_REGION:=${AWS_DEFAULT_REGION:-us-east-1}}"
export AWS_REGION
echo "region: ${AWS_REGION}"

if [ -z "${FIS_EXPERIMENT_ID:-}" ]; then
  cat <<EOF
No FIS_EXPERIMENT_ID set — skipping the read call.

To find an experiment id (read-only) with the AWS CLI:
  aws fis list-experiments --region "${AWS_REGION}"
  aws fis list-experiment-templates --region "${AWS_REGION}"

Then re-run:
  FIS_EXPERIMENT_ID=<id> ${BASH_SOURCE[0]}
EOF
  exit 0
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT
EXPERIMENT="${WORKDIR}/fis-status-smoke.toon"

cat >"${EXPERIMENT}" <<EOF
title: FIS status smoke (read-only)
description: Read one FIS experiment's status via tumult-cloud — no fault injected

tags[2]: aws, smoke

method[1]:
  - name: read-fis-status
    activity_type: probe
    provider:
      type: native
      plugin: tumult-cloud
      function: aws_fis_experiment_status
      arguments:
        experiment_id: ${FIS_EXPERIMENT_ID}
        region: ${AWS_REGION}

rollbacks[0]:
EOF

echo "validating experiment..."
# shellcheck disable=SC2086
${TUMULT} validate "${EXPERIMENT}"

echo "running read-only FIS status call..."
# shellcheck disable=SC2086
${TUMULT} run "${EXPERIMENT}"

echo "== smoke test complete =="
