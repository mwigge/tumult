#!/bin/sh
# The proof's fault: raise the flag probe.sh reads, hold it, then hand over
# to the recovery phase.
#
#   fault.sh <state-dir> <hold-seconds>
set -eu
state=$1
touch "$state/fault"
sleep "$2"
echo 0 >"$state/recovering"
rm -f "$state/fault"
echo done
