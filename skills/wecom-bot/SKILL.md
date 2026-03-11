---
name: wecom-bot
description: Send messages to a WeCom (企业微信) group via robot webhook
version: 1.0.0
author: OpenIntentOS
tags: [wecom, messaging, china]
requires:
  bins: [curl]
  env: [WECOM_WEBHOOK_URL]
tools:
  - name: wecom_send_text
    description: Send a plain text message to the WeCom group
    script: ./send.sh
    args:
      - name: content
        type: string
        required: true
        description: Text content to send
  - name: wecom_send_markdown
    description: Send a markdown-formatted message to the WeCom group
    script: ./send.sh
    args:
      - name: content
        type: string
        required: true
        description: Markdown content to send
  - name: wecom_send_news
    description: Send a news card link message to the WeCom group
    script: ./send.sh
    args:
      - name: title
        type: string
        required: true
        description: Title of the news card
      - name: description
        type: string
        required: false
        description: Short description shown below the title
      - name: url
        type: string
        required: true
        description: URL the card links to
      - name: picurl
        type: string
        required: false
        description: Optional thumbnail image URL for the card
---

# wecom-bot

Sends messages to a WeCom (企业微信) group chat using the robot webhook API.

## Setup

Create a group robot in WeCom and copy the webhook URL into the `WECOM_WEBHOOK_URL`
environment variable.

## Usage

```bash
# Send plain text
WECOM_WEBHOOK_URL="https://qyapi.weixin.qq.com/..." ./send.sh text "Hello team"

# Send markdown
WECOM_WEBHOOK_URL="https://qyapi.weixin.qq.com/..." ./send.sh markdown "**Alert**: deploy complete"

# Send news card
WECOM_WEBHOOK_URL="https://qyapi.weixin.qq.com/..." ./send.sh news "Title" "https://example.com" "Description" "https://example.com/img.jpg"
```

## Output

Returns JSON on success:

```json
{"success": true, "type": "text"}
```

On failure:

```json
{"success": false, "error": "invalid webhook url"}
```
