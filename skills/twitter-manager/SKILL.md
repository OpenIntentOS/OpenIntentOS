---
name: twitter-manager
description: Post tweets and manage Twitter/X account via the API v2
version: 1.0.0
author: OpenIntentOS
tags: [twitter, social-media, automation]
requires:
  bins: [curl, python3]
  env:
    - TWITTER_API_KEY
    - TWITTER_API_SECRET
    - TWITTER_ACCESS_TOKEN
    - TWITTER_ACCESS_SECRET
    - TWITTER_BEARER_TOKEN
tools:
  - name: tweet
    description: Post a tweet to Twitter/X (max 280 characters)
    script: ./tweet.sh
    args:
      - name: text
        type: string
        required: true
        description: The tweet text to post (max 280 characters)
---

# Twitter Manager

Posts tweets and manages the Twitter/X account using the API v2. Requires OAuth 1.0a credentials.

## Setup

Set the following environment variables before using this skill:

```bash
export TWITTER_API_KEY="your_api_key"
export TWITTER_API_SECRET="your_api_secret"
export TWITTER_ACCESS_TOKEN="your_access_token"
export TWITTER_ACCESS_SECRET="your_access_secret"
export TWITTER_BEARER_TOKEN="your_bearer_token"
```

Credentials are obtained from the [Twitter Developer Portal](https://developer.twitter.com/en/portal/dashboard).

## Usage

```bash
# Post a tweet
./tweet.sh "Hello from OpenIntentOS!"

# Post with newlines (quote the text)
./tweet.sh "Line one
Line two #OpenIntentOS"
```

## Output

Returns JSON on success:

```json
{
  "id": "1234567890123456789",
  "text": "Hello from OpenIntentOS!",
  "url": "https://twitter.com/i/web/status/1234567890123456789"
}
```

Returns an error JSON on failure:

```json
{
  "error": "authentication failed",
  "detail": "..."
}
```

## Notes

- Tweet text is truncated to 280 characters if longer.
- OAuth 1.0a signing is handled by `tweet_oauth.py` (requires `requests` and `requests-oauthlib` Python packages).
- Install Python deps: `pip3 install requests requests-oauthlib`
