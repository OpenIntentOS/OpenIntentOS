#!/bin/bash
# Web search using DuckDuckGo instant answer API
# Usage: ./web_search.sh "search query"
# Returns top results as JSON array

set -euo pipefail

QUERY="${1:-}"

if [ -z "$QUERY" ]; then
    echo '{"error": "no query provided", "usage": "./web_search.sh \"search terms\""}' >&2
    exit 1
fi

# URL-encode the query
ENCODED_QUERY=$(python3 -c "import urllib.parse, sys; print(urllib.parse.quote(sys.argv[1]))" "$QUERY" 2>/dev/null) || \
    ENCODED_QUERY="${QUERY// /+}"

# Query DuckDuckGo instant answer API
DDG_RESPONSE=$(curl -s --max-time 15 \
    -H "User-Agent: OpenIntentOS/1.0" \
    "https://api.duckduckgo.com/?q=${ENCODED_QUERY}&format=json&no_redirect=1&no_html=1" 2>/dev/null) || {
    echo '{"error": "failed to reach DuckDuckGo API"}' >&2
    exit 1
}

if command -v jq >/dev/null 2>&1; then
    # Build results array from RelatedTopics (up to 5)
    RESULTS=$(echo "$DDG_RESPONSE" | jq -c '
        [
            (.RelatedTopics // [])[]
            | select(.FirstURL and .Text)
            | {
                title: (.Text | split(" - ")[0] | .[0:120]),
                url: .FirstURL,
                snippet: (.Text | .[0:300])
              }
        ][0:5]
    ')

    # If no related topics, try Abstract
    if [ "$RESULTS" = "[]" ]; then
        ABSTRACT_TEXT=$(echo "$DDG_RESPONSE" | jq -r '.AbstractText // ""')
        ABSTRACT_URL=$(echo "$DDG_RESPONSE" | jq -r '.AbstractURL // ""')
        ABSTRACT_TITLE=$(echo "$DDG_RESPONSE" | jq -r '.Heading // ""')

        if [ -n "$ABSTRACT_TEXT" ]; then
            RESULTS=$(echo "$DDG_RESPONSE" | jq -c '[{
                title: .Heading,
                url: .AbstractURL,
                snippet: .AbstractText
            }]')
        fi
    fi

    # Output combined results
    echo "$DDG_RESPONSE" | jq -c --argjson results "$RESULTS" '{
        query: $ENV.QUERY,
        abstract: {
            title: (.Heading // ""),
            text: (.AbstractText // ""),
            url: (.AbstractURL // ""),
            source: (.AbstractSource // "")
        },
        answer: (.Answer // ""),
        results: $results
    }' --arg query "$QUERY" QUERY="$QUERY"

else
    # Fallback without jq: return raw snippet
    ABSTRACT=$(echo "$DDG_RESPONSE" | grep -o '"AbstractText":"[^"]*"' | head -1 | cut -d'"' -f4 | head -c 500)
    HEADING=$(echo "$DDG_RESPONSE" | grep -o '"Heading":"[^"]*"' | head -1 | cut -d'"' -f4)
    ABSTRACT_URL=$(echo "$DDG_RESPONSE" | grep -o '"AbstractURL":"[^"]*"' | head -1 | cut -d'"' -f4)

    printf '{"query": "%s", "abstract": {"title": "%s", "text": "%s", "url": "%s"}, "answer": "", "results": []}\n' \
        "$QUERY" "$HEADING" "$ABSTRACT" "$ABSTRACT_URL"
fi
