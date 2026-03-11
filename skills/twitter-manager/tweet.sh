#!/bin/bash
# Post a tweet using the Twitter/X API v2
# Usage: ./tweet.sh "tweet text"
# Requires: TWITTER_API_KEY, TWITTER_API_SECRET, TWITTER_ACCESS_TOKEN,
#           TWITTER_ACCESS_SECRET environment variables

set -euo pipefail

TWEET_TEXT="${1:-}"

if [ -z "$TWEET_TEXT" ]; then
    echo '{"error": "no tweet text provided", "usage": "./tweet.sh \"your tweet text\""}' >&2
    exit 1
fi

# Validate required env vars
for var in TWITTER_API_KEY TWITTER_API_SECRET TWITTER_ACCESS_TOKEN TWITTER_ACCESS_SECRET; do
    if [ -z "${!var:-}" ]; then
        printf '{"error": "missing required environment variable: %s"}\n' "$var" >&2
        exit 1
    fi
done

# Truncate to 280 characters
TWEET_TEXT="${TWEET_TEXT:0:280}"

# Delegate OAuth 1.0a signing and posting to the Python helper
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "${SCRIPT_DIR}/tweet_oauth.py" "$TWEET_TEXT"
