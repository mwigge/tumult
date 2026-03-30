#!/bin/sh
# Capture k6 end-of-test summary metrics as JSON
#
# Environment variables:
#   TUMULT_K6_RESULTS - Path to k6 JSON output (default: /tmp/tumult-k6-results.json)
set -eu

RESULTS="${TUMULT_K6_RESULTS:-/tmp/tumult-k6-results.json}"

if [ ! -f "${RESULTS}" ]; then
    echo '{"error": "k6 results file not found"}'
    exit 1
fi

# Extract key metrics from k6 JSON output
# k6 JSON output has one JSON object per line
if command -v python3 >/dev/null 2>&1; then
    python3 -c "
import json, sys
metrics = {}
with open(sys.argv[1]) as f:
    for line in f:
        try:
            d = json.loads(line)
            if d.get('type') == 'Point':
                name = d['metric']
                val = d['data']['value']
                if name not in metrics:
                    metrics[name] = {'count': 0, 'sum': 0, 'min': val, 'max': val}
                m = metrics[name]
                m['count'] += 1
                m['sum'] += val
                m['min'] = min(m['min'], val)
                m['max'] = max(m['max'], val)
        except: pass

result = {}
for name, m in metrics.items():
    if m['count'] > 0:
        result[name] = {
            'avg': round(m['sum'] / m['count'], 2),
            'min': round(m['min'], 2),
            'max': round(m['max'], 2),
            'count': m['count']
        }
print(json.dumps(result, indent=2))
" "${RESULTS}"
else
    # Fallback: just count lines
    LINES=$(wc -l < "${RESULTS}")
    echo "{\"data_points\": ${LINES}}"
fi
