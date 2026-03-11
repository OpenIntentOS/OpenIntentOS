#!/bin/bash
# Weather lookup script using wttr.in
# Usage: ./weather.sh [location]
# Returns JSON with weather data

set -euo pipefail

LOCATION="${1:-}"

if [ -z "$LOCATION" ]; then
    echo '{"error": "no location provided", "usage": "./weather.sh <city or location>"}' >&2
    exit 1
fi

# URL-encode location (replace spaces with +)
ENCODED_LOCATION="${LOCATION// /+}"

# Fetch JSON data from wttr.in
RESPONSE=$(curl -s --max-time 10 "https://wttr.in/${ENCODED_LOCATION}?format=j1" 2>/dev/null) || {
    echo '{"error": "failed to reach wttr.in"}' >&2
    exit 1
}

# Check if we got a valid JSON response
if ! echo "$RESPONSE" | grep -q '"current_condition"' 2>/dev/null; then
    # Fallback: use plain text format
    PLAIN=$(curl -s --max-time 10 "https://wttr.in/${ENCODED_LOCATION}?format=3" 2>/dev/null) || PLAIN="unavailable"
    printf '{"location": "%s", "temp": "unknown", "description": "%s", "humidity": "unknown", "wind": "unknown"}' \
        "$LOCATION" "$PLAIN"
    exit 0
fi

# Parse with jq if available
if command -v jq >/dev/null 2>&1; then
    echo "$RESPONSE" | jq -c '{
        location: (.nearest_area[0].areaName[0].value + ", " + .nearest_area[0].country[0].value),
        temp: (.current_condition[0].temp_C + "°C / " + .current_condition[0].temp_F + "°F"),
        description: .current_condition[0].weatherDesc[0].value,
        humidity: (.current_condition[0].humidity + "%"),
        wind: (.current_condition[0].windspeedKmph + " km/h " + .current_condition[0].winddir16Point)
    }'
else
    # Fallback without jq: extract fields with grep/sed
    TEMP_C=$(echo "$RESPONSE" | grep -o '"temp_C":"[^"]*"' | head -1 | cut -d'"' -f4)
    TEMP_F=$(echo "$RESPONSE" | grep -o '"temp_F":"[^"]*"' | head -1 | cut -d'"' -f4)
    DESC=$(echo "$RESPONSE" | grep -o '"weatherDesc":\[{"value":"[^"]*"' | head -1 | sed 's/.*"value":"\([^"]*\)".*/\1/')
    HUMIDITY=$(echo "$RESPONSE" | grep -o '"humidity":"[^"]*"' | head -1 | cut -d'"' -f4)
    WIND=$(echo "$RESPONSE" | grep -o '"windspeedKmph":"[^"]*"' | head -1 | cut -d'"' -f4)
    AREA=$(echo "$RESPONSE" | grep -o '"areaName":\[{"value":"[^"]*"' | head -1 | sed 's/.*"value":"\([^"]*\)".*/\1/')

    printf '{"location": "%s", "temp": "%s", "description": "%s", "humidity": "%s", "wind": "%s"}\n' \
        "${AREA:-$LOCATION}" \
        "${TEMP_C:-?}°C / ${TEMP_F:-?}°F" \
        "${DESC:-unknown}" \
        "${HUMIDITY:-?}%" \
        "${WIND:-?} km/h"
fi
