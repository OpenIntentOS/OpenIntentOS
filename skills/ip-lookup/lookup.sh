#!/bin/bash
# IP lookup script - returns IP geolocation data as JSON
# Usage: ./lookup.sh [ip_address]
# If no argument is given, looks up the machine's own public IP.

set -euo pipefail

IP="${1:-}"

if [ -z "$IP" ]; then
    # No argument: look up own public IP
    TARGET=""
else
    TARGET="$IP"
fi

RESPONSE=$(curl -s --max-time 10 "https://ipinfo.io/${TARGET}/json" 2>/dev/null) || {
    echo '{"error": "failed to reach ipinfo.io"}' >&2
    exit 1
}

# Check for error in response
if echo "$RESPONSE" | grep -q '"error"'; then
    echo "$RESPONSE" >&2
    exit 1
fi

if command -v jq >/dev/null 2>&1; then
    # Pretty-print and augment with human-readable lat/lon
    echo "$RESPONSE" | jq '{
        ip: .ip,
        hostname: (.hostname // ""),
        city: (.city // ""),
        region: (.region // ""),
        country: (.country // ""),
        org: (.org // ""),
        timezone: (.timezone // ""),
        loc: (.loc // ""),
        postal: (.postal // "")
    }'
else
    # Return raw response when jq is not available
    echo "$RESPONSE"
fi
