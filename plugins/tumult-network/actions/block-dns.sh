#!/bin/sh
# Block DNS resolution via iptables
# Requires root/sudo. Linux only.
#
# When TUMULT_DNS_DOMAIN is set, uses /etc/hosts to redirect the specific
# domain to 127.0.0.1 (portable, no CAP_NET_ADMIN needed).
# When not set, blocks all DNS traffic on port 53 via iptables.
#
# Environment variables:
#   TUMULT_DNS_PORT   - DNS port to block (default: 53)
#   TUMULT_DNS_DOMAIN - Optional: block only this domain (via /etc/hosts)
set -eu

DNS_PORT="${TUMULT_DNS_PORT:-53}"
DOMAIN="${TUMULT_DNS_DOMAIN:-}"
MARKER="# tumult-dns-block"

if [ -n "${DOMAIN}" ]; then
    # Targeted blocking via /etc/hosts — portable, no iptables needed
    case "$DOMAIN" in
        *[!a-zA-Z0-9.-]*) echo "error: invalid domain: ${DOMAIN}" >&2; exit 1 ;;
    esac
    echo "blocking DNS for ${DOMAIN} via /etc/hosts"
    printf '127.0.0.1\t%s %s\n' "${DOMAIN}" "${MARKER}" >> /etc/hosts
    echo "DNS blocked for ${DOMAIN}"
    exit 0
fi

# Full DNS block via iptables
if [ "$(uname -s)" != "Linux" ]; then
    echo "error: iptables requires Linux" >&2
    exit 1
fi

if ! command -v iptables >/dev/null 2>&1; then
    echo "error: iptables not found" >&2
    exit 1
fi

echo "blocking DNS (port ${DNS_PORT}) via iptables"
iptables -A OUTPUT -p udp --dport "${DNS_PORT}" -j DROP -m comment --comment "tumult-dns-block"
iptables -A OUTPUT -p tcp --dport "${DNS_PORT}" -j DROP -m comment --comment "tumult-dns-block"
echo "DNS blocked"
