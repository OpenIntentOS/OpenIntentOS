---
name: sms-aliyun
description: Send SMS via Alibaba Cloud (阿里云短信)
version: 1.0.0
author: OpenIntentOS
tags: [sms, notification, china]
requires:
  bins: [python3]
  env: [ALIYUN_ACCESS_KEY_ID, ALIYUN_ACCESS_KEY_SECRET, ALIYUN_SMS_SIGN_NAME]
tools:
  - name: sms_send_aliyun
    description: Send an SMS message via Alibaba Cloud SMS
    script: ./send.py
    args:
      - name: phone
        type: string
        required: true
        description: "Phone number to send to, e.g. +8613800138000"
      - name: template_code
        type: string
        required: true
        description: "Template code, e.g. SMS_123456"
      - name: template_params
        type: string
        required: false
        description: 'JSON string of template params, e.g. {"code":"1234"}'
---

# sms-aliyun

Sends SMS messages using the Alibaba Cloud (阿里云) Dysmsapi with HMAC-SHA1 request signing.

## Setup

Set the following environment variables:

| Variable | Description |
|---|---|
| `ALIYUN_ACCESS_KEY_ID` | Alibaba Cloud access key ID |
| `ALIYUN_ACCESS_KEY_SECRET` | Alibaba Cloud access key secret |
| `ALIYUN_SMS_SIGN_NAME` | Approved SMS signature name |

## Usage

```bash
python3 send.py "+8613800138000" "SMS_123456" '{"code":"9527"}'
```

## Output

Success:

```json
{"success": true, "phone": "+8613800138000", "biz_id": "9014..."}
```

Failure:

```json
{"success": false, "error": "isv.MOBILE_NUMBER_ILLEGAL"}
```
