#!/usr/bin/env sh
set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
docker compose up -d --build --wait --wait-timeout 120
printf '\nTumult Chaos Lab is ready.\n'
printf 'Open http://localhost:%s (or LAB_PORT from .env).\n' "${LAB_PORT:-8089}"
printf 'Choose a lesson; add an OpenAI or Claude key in AI settings when you want tutoring.\n'
