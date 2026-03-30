#!/bin/sh
# Probe: current memory utilization percentage
# Outputs a single float value (0-100)
set -e

OS="$(uname -s)"
case "${OS}" in
    Linux)
        awk '/MemTotal/ {total=$2} /MemAvailable/ {avail=$2} END { printf "%.1f", (1 - avail/total) * 100 }' /proc/meminfo
        ;;
    Darwin)
        # macOS — use vm_stat
        page_size=$(sysctl -n hw.pagesize)
        total_mem=$(sysctl -n hw.memsize)
        free_pages=$(vm_stat | awk '/Pages free/ { gsub(/\./,""); print $3 }')
        inactive_pages=$(vm_stat | awk '/Pages inactive/ { gsub(/\./,""); print $3 }')
        free_mem=$(( (free_pages + inactive_pages) * page_size ))
        awk "BEGIN { printf \"%.1f\", (1 - ${free_mem} / ${total_mem}) * 100 }"
        ;;
    *)
        echo "error: unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac
