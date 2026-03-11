#!/usr/bin/env python3
"""
Alibaba Cloud SMS sender using HMAC-SHA1 signature.
Usage: python3 send.py <phone> <template_code> [template_params_json]
"""
import sys, os, json, hmac, hashlib, time, urllib.parse, requests, base64, uuid


def percent_encode(s):
    return urllib.parse.quote(str(s), safe='')


def sign_aliyun(key, params):
    sorted_params = sorted(params.items())
    query = '&'.join(f"{percent_encode(k)}={percent_encode(v)}" for k, v in sorted_params)
    string_to_sign = f"POST&{percent_encode('/')}&{percent_encode(query)}"
    mac = hmac.new((key + '&').encode(), string_to_sign.encode(), hashlib.sha1)
    return base64.b64encode(mac.digest()).decode()


def main():
    phone = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('PHONE', '')
    template_code = sys.argv[2] if len(sys.argv) > 2 else os.environ.get('TEMPLATE_CODE', '')
    template_params = sys.argv[3] if len(sys.argv) > 3 else os.environ.get('TEMPLATE_PARAMS', '{}')

    access_key_id = os.environ['ALIYUN_ACCESS_KEY_ID']
    access_key_secret = os.environ['ALIYUN_ACCESS_KEY_SECRET']
    sign_name = os.environ['ALIYUN_SMS_SIGN_NAME']

    params = {
        'Action': 'SendSms',
        'Version': '2017-05-25',
        'Format': 'JSON',
        'AccessKeyId': access_key_id,
        'SignatureMethod': 'HMAC-SHA1',
        'SignatureVersion': '1.0',
        'SignatureNonce': str(uuid.uuid4()),
        'Timestamp': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
        'RegionId': 'cn-hangzhou',
        'PhoneNumbers': phone,
        'SignName': sign_name,
        'TemplateCode': template_code,
        'TemplateParam': template_params,
    }

    signature = sign_aliyun(access_key_secret, params)
    params['Signature'] = signature

    resp = requests.post('https://dysmsapi.aliyuncs.com/', data=params)
    result = resp.json()

    if result.get('Code') == 'OK':
        print(json.dumps({
            "success": True,
            "phone": phone,
            "biz_id": result.get('BizId', ''),
        }))
    else:
        print(json.dumps({
            "success": False,
            "error": result.get('Message', result.get('Code', 'Unknown')),
        }))
        sys.exit(1)


if __name__ == "__main__":
    main()
