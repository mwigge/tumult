#!/usr/bin/env sh
# Generate real sample evidence without an AI subscription or model-generated commands.
set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
docker compose exec -T lab python - <<'PY'
import http.cookiejar
import json
import time
import urllib.request

client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
base = 'http://127.0.0.1:8000/api'

def request(path, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    message = urllib.request.Request(base + path, data=data, headers={'Content-Type': 'application/json'})
    with client.open(message, timeout=15) as response:
        return json.load(response)

request('/status')
for scenario in request('/scenarios'):
    run = request('/runs', {'scenario_id': scenario['id'], 'armed': True})
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        result = request('/runs/' + run['id'])
        if result['finished_at']:
            break
        time.sleep(0.5)
    else:
        request('/runs/' + run['id'] + '/stop', {})
        raise SystemExit('Experiment timed out and was stopped.')
    print(scenario['title'], result['status'], result['verdict'])
    if result['status'] != 'completed' or not result['recovery']['healthy']:
        raise SystemExit('Experiment did not finish with measured healthy recovery.')
PY
