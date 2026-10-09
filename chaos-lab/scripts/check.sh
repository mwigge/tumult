#!/usr/bin/env sh
set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
: "${TUMULT_TEST_BIN:?Set TUMULT_TEST_BIN to the absolute path of a verified Tumult 2.22.0 Linux binary}"
.venv/bin/ruff check agent scripts tests targets
.venv/bin/black --check --workers 1 agent scripts tests targets
.venv/bin/mypy agent scripts
.venv/bin/bandit -r agent scripts targets -ll
.venv/bin/python -m pytest --cov=agent --cov-report=term-missing --cov-fail-under=95
.venv/bin/pip-audit -r agent/requirements.txt
npm run lint
npm run format:check
npm test
npm audit
docker compose config --quiet
