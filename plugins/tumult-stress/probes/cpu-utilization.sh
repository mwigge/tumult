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
        # macOS — use top for CPU idle. top runs on its own first, so a failed
        # top ends the probe with its status and its message.
        TOP=$(top -l 1 -n 0)
        BUSY=$(printf '%s\n' "${TOP}" | awk '/CPU usage/ { gsub(/%/,""); print 100 - $7 }')
        if [ -z "${BUSY}" ]; then
            echo "error: top printed no CPU usage line" >&2
            printf '%s\n' "${TOP}" >&2
            exit 1
        fi
        echo "${BUSY}"
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
