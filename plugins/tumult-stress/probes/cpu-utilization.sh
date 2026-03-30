#!/bin/sh
# Probe: current CPU utilization percentage
# Outputs a single float value (0-100)
set -eu

OS="$(uname -s)"
case "${OS}" in
    Linux)
        # Use /proc/stat — sample over 1 second
        read -r _ user1 nice1 system1 idle1 _ < /proc/stat
        sleep 1
        read -r _ user2 nice2 system2 idle2 _ < /proc/stat
        active=$((user2 + nice2 + system2 - user1 - nice1 - system1))
        total=$((active + idle2 - idle1))
        if [ "${total}" -gt 0 ]; then
            awk "BEGIN { printf \"%.1f\", ${active} / ${total} * 100 }"
        else
            echo "0.0"
        fi
        ;;
    Darwin)
        # macOS — use top for CPU idle
        top -l 1 -n 0 2>/dev/null | awk '/CPU usage/ { gsub(/%/,""); print 100 - $7 }'
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
