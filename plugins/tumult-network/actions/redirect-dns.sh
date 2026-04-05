#!/bin/sh
# Redirect DNS for a specific domain to a wrong IP
# Uses /etc/hosts manipulation — works on Linux, macOS, and containers.
# No iptables or CAP_NET_ADMIN needed.
#
# This is safer and more portable than iptables --string matching
# (which doesn't work because DNS wire format uses length-prefixed
# labels, not dot-separated strings).
#
# Environment variables:
#   TUMULT_DNS_DOMAIN   - Domain to redirect (required)
#   TUMULT_DNS_REDIRECT - IP to redirect to (default: 127.0.0.1)
set -eu

DOMAIN="${TUMULT_DNS_DOMAIN:?TUMULT_DNS_DOMAIN required}"
REDIRECT_IP="${TUMULT_DNS_REDIRECT:-127.0.0.1}"
MARKER="# tumult-dns-redirect"

# Validate domain format (basic check — no path traversal, no whitespace)
case "$DOMAIN" in
    *[!a-zA-Z0-9.-]*) echo "error: invalid domain: ${DOMAIN}" >&2; exit 1 ;;
esac

echo "redirecting ${DOMAIN} → ${REDIRECT_IP} via /etc/hosts"

# Append redirect entry with marker for clean rollback
printf '%s\t%s %s\n' "${REDIRECT_IP}" "${DOMAIN}" "${MARKER}" >> /etc/hosts

echo "DNS redirect active"
