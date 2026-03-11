#!/bin/bash
# Daily briefing aggregator
# Fetches weather, a motivational quote, and top HackerNews stories,
# then outputs a structured JSON morning briefing.
# Usage: ./briefing.sh
# Env: BRIEFING_LOCATION (default: "auto")

set -euo pipefail

LOCATION="${BRIEFING_LOCATION:-auto}"
NOW=$(date '+%Y-%m-%dT%H:%M:%S')
DATE_HUMAN=$(date '+%A, %B %-d %Y')

# --- Weather ---
get_weather() {
    local loc="$1"
    local encoded="${loc// /+}"
    local resp

    resp=$(curl -s --max-time 10 "https://wttr.in/${encoded}?format=j1" 2>/dev/null) || {
        echo '{"error": "unavailable"}'
        return
    }

    if command -v jq >/dev/null 2>&1 && echo "$resp" | jq -e '.current_condition' >/dev/null 2>&1; then
        echo "$resp" | jq -c '{
            location: (.nearest_area[0].areaName[0].value + ", " + .nearest_area[0].country[0].value),
            temp: (.current_condition[0].temp_C + "°C / " + .current_condition[0].temp_F + "°F"),
            description: .current_condition[0].weatherDesc[0].value,
            humidity: (.current_condition[0].humidity + "%"),
            wind: (.current_condition[0].windspeedKmph + " km/h " + .current_condition[0].winddir16Point)
        }'
    else
        # Plain text fallback
        local plain
        plain=$(curl -s --max-time 10 "https://wttr.in/${encoded}?format=3" 2>/dev/null) || plain="unavailable"
        printf '{"location": "%s", "temp": "unknown", "description": "%s", "humidity": "unknown", "wind": "unknown"}' \
            "$loc" "$plain"
    fi
}

# --- Motivational quote ---
get_quote() {
    local resp
    resp=$(curl -s --max-time 10 "https://api.quotable.io/random" 2>/dev/null) || {
        echo '"Carpe diem." — Unknown'
        return
    }

    if command -v jq >/dev/null 2>&1; then
        echo "$resp" | jq -r '"\(.content) — \(.author)"' 2>/dev/null || echo '"Carpe diem." — Unknown'
    else
        echo '"Carpe diem." — Unknown'
    fi
}

# --- HackerNews top 3 stories ---
get_news() {
    local ids
    ids=$(curl -s --max-time 10 "https://hacker-news.firebaseio.com/v0/topstories.json" 2>/dev/null | \
          python3 -c "import sys,json; ids=json.load(sys.stdin); print(' '.join(str(i) for i in ids[:3]))" 2>/dev/null) || {
        echo '[]'
        return
    }

    local stories='['
    local first=true
    for id in $ids; do
        local item
        item=$(curl -s --max-time 10 "https://hacker-news.firebaseio.com/v0/item/${id}.json" 2>/dev/null) || continue
        if command -v jq >/dev/null 2>&1; then
            local entry
            entry=$(echo "$item" | jq -c '{title: .title, url: (.url // ("https://news.ycombinator.com/item?id=" + (.id | tostring))), score: .score}' 2>/dev/null) || continue
            if [ "$first" = true ]; then
                stories+="$entry"
                first=false
            else
                stories+=",$entry"
            fi
        fi
    done
    stories+=']'
    echo "$stories"
}

# Gather all data
WEATHER=$(get_weather "$LOCATION")
QUOTE=$(get_quote)
NEWS=$(get_news)

# Output final JSON briefing
if command -v jq >/dev/null 2>&1; then
    jq -n \
        --arg date "$DATE_HUMAN" \
        --arg datetime "$NOW" \
        --argjson weather "$WEATHER" \
        --arg quote "$QUOTE" \
        --argjson news "$NEWS" \
        '{
            date: $date,
            datetime: $datetime,
            weather: $weather,
            quote: $quote,
            news: $news
        }'
else
    printf '{"date": "%s", "datetime": "%s", "weather": %s, "quote": "%s", "news": %s}\n' \
        "$DATE_HUMAN" "$NOW" "$WEATHER" "$QUOTE" "$NEWS"
fi
