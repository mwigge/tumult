#!/bin/sh
# The old name of simulate-hang. It makes Redis hang with DEBUG SLEEP, it
# fails nothing over. Kept so that older experiments still run; use
# simulate-hang.
set -eu

echo "warning: simulate-failover is deprecated, use simulate-hang: it makes Redis hang, it fails nothing over" >&2
exec sh "$(dirname "$0")/simulate-hang.sh" "$@"
