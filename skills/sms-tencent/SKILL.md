---
name: sms-tencent
description: Send SMS via Tencent Cloud (腾讯云短信)
version: 1.0.0
author: OpenIntentOS
tags: [sms, notification, china]
requires:
  bins: [python3]
  env: [TENCENT_SECRET_ID, TENCENT_SECRET_KEY, TENCENT_SMS_APP_ID, TENCENT_SMS_SIGN_NAME]
tools:
  - name: sms_send_tencent
    description: Send an SMS message via Tencent Cloud SMS
    script: ./send.py
    args:
      - name: phone
        type: string
        required: true
        description: "Phone number with country code, e.g. +8613800138000"
      - name: template_id
        type: string
        required: true
        description: Tencent Cloud SMS template ID
      - name: params
        type: string
        required: false
        description: "Comma-separated template params, e.g. 123456,5"
---

# sms-tencent

Sends SMS messages using the Tencent Cloud (腾讯云) SMS API with TC3-HMAC-SHA256 request signing.

## Setup

Set the following environment variables:

| Variable | Description |
|---|---|
| `TENCENT_SECRET_ID` | Tencent Cloud API secret ID |
| `TENCENT_SECRET_KEY` | Tencent Cloud API secret key |
| `TENCENT_SMS_APP_ID` | SMS application ID (SDKAppId) |
| `TENCENT_SMS_SIGN_NAME` | Approved SMS signature name |

## Usage

```bash
python3 send.py "+8613800138000" "123456" "987654,5"
```

## Output

Success:

```json
{"success": true, "phone": "+8613800138000", "message_id": "2021-..."}
```

Failure:

```json
{"success": false, "error": "InvalidParameterValue.TemplateNotExist"}
```
