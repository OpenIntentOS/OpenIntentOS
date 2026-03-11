#!/usr/bin/env python3
"""
OAuth 1.0a helper for posting tweets via Twitter/X API v2.
Called by tweet.sh — reads credentials from environment variables.

Dependencies: pip3 install requests requests-oauthlib
"""

import json
import os
import sys


def main() -> None:
    if len(sys.argv) < 2:
        print(json.dumps({"error": "no tweet text provided"}))
        sys.exit(1)

    tweet_text = sys.argv[1][:280]

    api_key = os.environ.get("TWITTER_API_KEY", "")
    api_secret = os.environ.get("TWITTER_API_SECRET", "")
    access_token = os.environ.get("TWITTER_ACCESS_TOKEN", "")
    access_secret = os.environ.get("TWITTER_ACCESS_SECRET", "")

    for name, val in [
        ("TWITTER_API_KEY", api_key),
        ("TWITTER_API_SECRET", api_secret),
        ("TWITTER_ACCESS_TOKEN", access_token),
        ("TWITTER_ACCESS_SECRET", access_secret),
    ]:
        if not val:
            print(json.dumps({"error": f"missing required environment variable: {name}"}))
            sys.exit(1)

    try:
        from requests_oauthlib import OAuth1Session  # type: ignore
    except ImportError:
        print(
            json.dumps({
                "error": "requests-oauthlib not installed",
                "detail": "run: pip3 install requests requests-oauthlib",
            })
        )
        sys.exit(1)

    session = OAuth1Session(
        client_key=api_key,
        client_secret=api_secret,
        resource_owner_key=access_token,
        resource_owner_secret=access_secret,
    )

    url = "https://api.twitter.com/2/tweets"
    payload = {"text": tweet_text}

    try:
        response = session.post(url, json=payload, timeout=15)
    except Exception as exc:
        print(json.dumps({"error": "request failed", "detail": str(exc)}))
        sys.exit(1)

    if response.status_code in (200, 201):
        data = response.json().get("data", {})
        tweet_id = data.get("id", "")
        result = {
            "id": tweet_id,
            "text": data.get("text", tweet_text),
            "url": f"https://twitter.com/i/web/status/{tweet_id}" if tweet_id else "",
        }
        print(json.dumps(result, indent=2))
    else:
        try:
            err_body = response.json()
        except Exception:
            err_body = response.text
        print(json.dumps({
            "error": "API request failed",
            "status": response.status_code,
            "detail": err_body,
        }, indent=2))
        sys.exit(1)


if __name__ == "__main__":
    main()
