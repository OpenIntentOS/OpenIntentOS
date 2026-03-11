#!/usr/bin/env bash
# Weibo operations using pre-obtained OAuth2 access token.
# Usage: ./post.sh <action> [args...]
# Actions:
#   post     <content> [pic_url]
#   mentions [count]
#   reply    <comment> <weibo_id>
set -euo pipefail

TOKEN="${WEIBO_ACCESS_TOKEN:?WEIBO_ACCESS_TOKEN not set}"
ACTION="${1:-post}"

case "$ACTION" in
  post)
    CONTENT="${2:?content required}"
    curl -s -X POST "https://api.weibo.com/2/statuses/share.json" \
      -F "access_token=$TOKEN" \
      -F "status=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$CONTENT")" \
      | python3 -c "
import json,sys
r=json.load(sys.stdin)
if 'id' in r:
    print(json.dumps({'success':True,'id':str(r['id']),'text':r.get('text','')}))
else:
    print(json.dumps({'success':False,'error':r.get('error','unknown')}))
    sys.exit(1)
"
    ;;
  mentions)
    COUNT="${2:-10}"
    curl -s "https://api.weibo.com/2/statuses/mentions.json?access_token=$TOKEN&count=$COUNT" \
      | python3 -c "
import json,sys
r=json.load(sys.stdin)
statuses=r.get('statuses',[])
results=[{'id':str(s['id']),'text':s.get('text',''),'user':s.get('user',{}).get('screen_name','')} for s in statuses]
print(json.dumps({'success':True,'mentions':results,'count':len(results)}))
"
    ;;
  reply)
    COMMENT="${2:?comment required}"
    WEIBO_ID="${3:?weibo_id required}"
    curl -s -X POST "https://api.weibo.com/2/comments/create.json" \
      -F "access_token=$TOKEN" \
      -F "id=$WEIBO_ID" \
      -F "comment=$COMMENT" \
      | python3 -c "
import json,sys
r=json.load(sys.stdin)
if 'id' in r:
    print(json.dumps({'success':True,'comment_id':str(r['id'])}))
else:
    print(json.dumps({'success':False,'error':r.get('error','unknown')}))
    sys.exit(1)
"
    ;;
  *)
    echo "Unknown action: $ACTION" >&2
    exit 1
    ;;
esac
