#!/bin/sh
# Probe: current IO wait percentage
# Outputs a single float value (0-100)
set -e

OS="$(uname -s)"
case "${OS}" in
    Linux)
        # Use /proc/stat — sample iowait over 1 second
        read -r _ user1 nice1 system1 idle1 iowait1 _ < /proc/stat
        sleep 1
        read -r _ user2 nice2 system2 idle2 iowait2 _ < /proc/stat
        iowait_delta=$((iowait2 - iowait1))
        total=$((user2 + nice2 + system2 + idle2 + iowait2 - user1 - nice1 - system1 - idle1 - iowait1))
        if [ "${total}" -gt 0 ]; then
            awk "BEGIN { printf \"%.1f\", ${iowait_delta} / ${total} * 100 }"
        else
            echo "0.0"
        fi
        ;;
    Darwin)
        # macOS — iostat doesn't expose iowait directly, approximate with disk busy
        iostat -c 2 -w 1 2>/dev/null | tail -1 | awk '{ printf "%.1f", 100 - $NF }'
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
