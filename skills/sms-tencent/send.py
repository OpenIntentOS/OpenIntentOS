#!/usr/bin/env python3
"""
Tencent Cloud SMS sender.
Usage: python3 send.py <phone> <template_id> [param1,param2,...]
"""
import sys, os, json, hmac, hashlib, time, datetime, requests


def sign_v3(secret_id, secret_key, host, service, action, version, region, payload):
    timestamp = int(time.time())
    date = datetime.datetime.utcfromtimestamp(timestamp).strftime('%Y-%m-%d')

    canonical_headers = f"content-type:application/json\nhost:{host}\n"
    signed_headers = "content-type;host"
    hashed_payload = hashlib.sha256(payload.encode()).hexdigest()
    canonical_request = f"POST\n/\n\n{canonical_headers}\n{signed_headers}\n{hashed_payload}"

    credential_scope = f"{date}/{service}/tc3_request"
    string_to_sign = (
        f"TC3-HMAC-SHA256\n{timestamp}\n{credential_scope}\n"
        f"{hashlib.sha256(canonical_request.encode()).hexdigest()}"
    )

    def hmac_sha256(key, msg):
        return hmac.new(
            key if isinstance(key, bytes) else key.encode(),
            msg.encode(),
            hashlib.sha256,
        ).digest()

    secret_date = hmac_sha256(f"TC3{secret_key}", date)
    secret_service = hmac_sha256(secret_date, service)
    secret_signing = hmac_sha256(secret_service, "tc3_request")
    signature = hmac.new(secret_signing, string_to_sign.encode(), hashlib.sha256).hexdigest()

    auth = (
        f"TC3-HMAC-SHA256 Credential={secret_id}/{credential_scope}, "
        f"SignedHeaders={signed_headers}, Signature={signature}"
    )
    return auth, timestamp


def main():
    phone = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('PHONE', '')
    template_id = sys.argv[2] if len(sys.argv) > 2 else os.environ.get('TEMPLATE_ID', '')
    params_str = sys.argv[3] if len(sys.argv) > 3 else os.environ.get('PARAMS', '')

    secret_id = os.environ['TENCENT_SECRET_ID']
    secret_key = os.environ['TENCENT_SECRET_KEY']
    app_id = os.environ['TENCENT_SMS_APP_ID']
    sign_name = os.environ['TENCENT_SMS_SIGN_NAME']

    template_params = [p.strip() for p in params_str.split(',')] if params_str else []

    payload = json.dumps({
        "PhoneNumberSet": [phone],
        "SmsSdkAppId": app_id,
        "SignName": sign_name,
        "TemplateId": template_id,
        "TemplateParamSet": template_params,
    })

    host = "sms.tencentcloudapi.com"
    auth, timestamp = sign_v3(
        secret_id, secret_key, host, "sms", "SendSms", "2021-01-11", "ap-guangzhou", payload
    )

    resp = requests.post(
        f"https://{host}/",
        headers={
            "Authorization": auth,
            "Content-Type": "application/json",
            "Host": host,
            "X-TC-Action": "SendSms",
            "X-TC-Version": "2021-01-11",
            "X-TC-Timestamp": str(timestamp),
            "X-TC-Region": "ap-guangzhou",
        },
        data=payload,
    )

    result = resp.json()
    send_status = result.get("Response", {}).get("SendStatusSet", [{}])[0]

    if send_status.get("Code") == "Ok":
        print(json.dumps({
            "success": True,
            "phone": phone,
            "message_id": send_status.get("SerialNo", ""),
        }))
    else:
        print(json.dumps({
            "success": False,
            "error": send_status.get("Message", "Unknown error"),
        }))
        sys.exit(1)


if __name__ == "__main__":
    main()
