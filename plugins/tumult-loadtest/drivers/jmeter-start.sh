#!/bin/sh
# Start a JMeter load test in non-GUI mode
#
# Environment variables:
#   TUMULT_JMETER_PLAN     - Path to .jmx test plan (required)
#   TUMULT_JMETER_HOME     - JMeter install directory (default: /opt/jmeter)
#   TUMULT_JMETER_THREADS  - Number of threads (overrides test plan, optional)
#   TUMULT_JMETER_DURATION - Duration in seconds (overrides test plan, optional)
#   TUMULT_JMETER_RESULTS  - JTL results file (default: /tmp/tumult-jmeter.jtl)
#   TUMULT_JMETER_PIDFILE  - PID file (default: /tmp/tumult-jmeter.pid)
set -eu

PLAN="${TUMULT_JMETER_PLAN:?TUMULT_JMETER_PLAN is required}"
JMETER_HOME="${TUMULT_JMETER_HOME:-/opt/jmeter}"
RESULTS="${TUMULT_JMETER_RESULTS:-/tmp/tumult-jmeter.jtl}"
PIDFILE="${TUMULT_JMETER_PIDFILE:-/tmp/tumult-jmeter.pid}"
JMETER="${JMETER_HOME}/bin/jmeter"

if [ ! -x "${JMETER}" ]; then
    # Try PATH
    if command -v jmeter >/dev/null 2>&1; then
        JMETER="jmeter"
    else
        echo "error: jmeter not found at ${JMETER} or in PATH" >&2
        exit 1
    fi
fi

if [ ! -f "${PLAN}" ]; then
    echo "error: JMeter test plan not found: ${PLAN}" >&2
    exit 1
fi

ARGS="-n -t ${PLAN} -l ${RESULTS}"

if [ -n "${TUMULT_JMETER_THREADS:-}" ]; then
    ARGS="${ARGS} -Jthreads=${TUMULT_JMETER_THREADS}"
fi
if [ -n "${TUMULT_JMETER_DURATION:-}" ]; then
    ARGS="${ARGS} -Jduration=${TUMULT_JMETER_DURATION}"
fi

echo "starting JMeter: plan=${PLAN} results=${RESULTS}"
${JMETER} ${ARGS} &
echo $! > "${PIDFILE}"
echo "JMeter started (PID: $(cat "${PIDFILE}"))"
