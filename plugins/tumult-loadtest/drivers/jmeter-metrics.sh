#!/bin/sh
# Parse JMeter JTL results for key metrics
#
# Environment variables:
#   TUMULT_JMETER_RESULTS - JTL results file (default: /tmp/tumult-jmeter.jtl)
set -eu

RESULTS="${TUMULT_JMETER_RESULTS:-/tmp/tumult-jmeter.jtl}"

if [ ! -f "${RESULTS}" ]; then
    echo '{"error": "JMeter results file not found"}'
    exit 1
fi

# JTL CSV format: timeStamp,elapsed,label,responseCode,responseMessage,threadName,dataType,success,failureMessage,bytes,sentBytes,grpThreads,allThreads,URL,Latency,IdleTime,Connect
if command -v python3 >/dev/null 2>&1; then
    python3 -c "
import csv, json, sys

times = []
errors = 0
total = 0

with open(sys.argv[1]) as f:
    reader = csv.DictReader(f)
    for row in reader:
        total += 1
        elapsed = int(row.get('elapsed', 0))
        times.append(elapsed)
        if row.get('success', 'true').lower() != 'true':
            errors += 1

if not times:
    print(json.dumps({'error': 'no data points'}))
    sys.exit(0)

times.sort()
n = len(times)
result = {
    'total_requests': total,
    'error_count': errors,
    'error_rate': round(errors / total, 4) if total > 0 else 0,
    'avg_ms': round(sum(times) / n, 1),
    'p50_ms': times[int(n * 0.5)],
    'p95_ms': times[int(n * 0.95)],
    'p99_ms': times[int(n * 0.99)],
    'min_ms': times[0],
    'max_ms': times[-1],
}
print(json.dumps(result))
" "${RESULTS}"
else
    # Fallback: line count
    LINES=$(wc -l < "${RESULTS}")
    echo "{\"total_requests\": $((LINES - 1))}"
fi
