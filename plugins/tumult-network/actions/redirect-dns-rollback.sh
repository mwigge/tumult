#!/bin/sh
# Rollback: remove tumult DNS redirect entries from /etc/hosts
set -eu

MARKER="tumult-dns-redirect"

if grep -q "${MARKER}" /etc/hosts 2>/dev/null; then
    # Remove lines containing our marker
    sed -i.bak "/${MARKER}/d" /etc/hosts
    rm -f /etc/hosts.bak
    echo "DNS redirect entries removed"
else
    echo "no redirect entries found"
fi
