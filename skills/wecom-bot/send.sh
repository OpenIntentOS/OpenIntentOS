#!/usr/bin/env bash
# Usage: ./send.sh <type> <content> [extra_args...]
# type: text | markdown | news
#   text/markdown: ./send.sh text "Hello"
#   news:          ./send.sh news <title> <url> [description] [picurl]
set -euo pipefail

WEBHOOK_URL="${WECOM_WEBHOOK_URL:?WECOM_WEBHOOK_URL not set}"
TYPE="${1:-text}"
CONTENT="${2:-}"

case "$TYPE" in
  text)
    PAYLOAD=$(printf '{"msgtype":"text","text":{"content":"%s"}}' "$CONTENT")
    ;;
  markdown)
    PAYLOAD=$(printf '{"msgtype":"markdown","markdown":{"content":"%s"}}' "$CONTENT")
    ;;
  news)
    TITLE="${3:-}"
    URL="${4:-}"
    DESC="${5:-}"
    PICURL="${6:-}"
    PAYLOAD=$(printf '{"msgtype":"news","news":{"articles":[{"title":"%s","description":"%s","url":"%s","picurl":"%s"}]}}' "$TITLE" "$DESC" "$URL" "$PICURL")
    ;;
  *)
    echo "Unknown type: $TYPE" >&2
    exit 1
    ;;
esac

curl -s -X POST "$WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" | python3 -c "
import json,sys
r=json.load(sys.stdin)
if r.get('errcode',0)==0:
    print(json.dumps({'success':True,'type':'$TYPE'}))
else:
    print(json.dumps({'success':False,'error':r.get('errmsg','unknown')}))
    sys.exit(1)
"
