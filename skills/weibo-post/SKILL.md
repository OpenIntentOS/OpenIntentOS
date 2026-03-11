---
name: weibo-post
description: Post to Weibo (微博) using a pre-obtained access token
version: 1.0.0
author: OpenIntentOS
tags: [weibo, social, china]
requires:
  bins: [curl]
  env: [WEIBO_ACCESS_TOKEN, WEIBO_UID]
tools:
  - name: weibo_post
    description: Post a status update to Weibo
    script: ./post.sh
    args:
      - name: content
        type: string
        required: true
        description: Post text, max 140 chars
      - name: pic_url
        type: string
        required: false
        description: Optional image URL to attach
  - name: weibo_get_mentions
    description: Fetch recent @mentions for the authenticated user
    script: ./post.sh
    args:
      - name: count
        type: string
        required: false
        description: Number of mentions to fetch, default 10
  - name: weibo_reply
    description: Post a comment reply to an existing Weibo
    script: ./post.sh
    args:
      - name: comment
        type: string
        required: true
        description: Reply text
      - name: weibo_id
        type: string
        required: true
        description: ID of the Weibo post to reply to
---

# weibo-post

Interacts with Weibo (微博) using the official API v2 and a pre-obtained OAuth2 access token.

## Setup

Obtain an OAuth2 access token via the Weibo developer portal and set:

| Variable | Description |
|---|---|
| `WEIBO_ACCESS_TOKEN` | OAuth2 access token |
| `WEIBO_UID` | Your Weibo user ID (for context) |

## Usage

```bash
# Post a status
WEIBO_ACCESS_TOKEN="..." ./post.sh post "Hello Weibo!"

# Fetch 5 mentions
WEIBO_ACCESS_TOKEN="..." ./post.sh mentions 5

# Reply to a post
WEIBO_ACCESS_TOKEN="..." ./post.sh reply "Great post!" "4987654321098765"
```

## Output

Post success:

```json
{"success": true, "id": "4987654321098765", "text": "Hello Weibo!"}
```

Mentions success:

```json
{"success": true, "mentions": [{"id": "...", "text": "...", "user": "..."}], "count": 5}
```

Failure:

```json
{"success": false, "error": "expired token"}
```
