#!/bin/sh
# The proof's health probe: answers slowly while the fault flag is up, then
# reports "recovering" for a fixed number of calls (the first few of them
# still slow) before it says "ok" again. The runner's post phase samples
# until the probe passes, so this gives the run a recovery tail to measure.
#
#   probe.sh <state-dir> <slow-recovery-calls> <recovery-calls>
set -eu
state=$1
slow=$2
total=$3
if [ -f "$state/fault" ]; then
    sleep 0.30
    echo degraded
    exit 0
fi
if [ -f "$state/recovering" ]; then
    calls=$(($(cat "$state/recovering") + 1))
    echo "$calls" >"$state/recovering"
    if [ "$calls" -le "$slow" ]; then
        sleep 0.15
        echo recovering
        exit 0
    fi
    if [ "$calls" -le "$total" ]; then
        sleep 0.05
        echo recovering
        exit 0
    fi
fi
sleep 0.05
echo ok
