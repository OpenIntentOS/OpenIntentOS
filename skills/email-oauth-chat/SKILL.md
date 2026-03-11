---
name: email-oauth-chat
description: Complete email OAuth authentication flows interactively via Telegram chat
version: 1.0.0
author: OpenIntentOS
tags: [email, oauth, telegram, authentication, gmail, outlook, yahoo]
requires:
  bins: [bash, curl]
  env:
    - TELEGRAM_BOT_TOKEN
tools:
  - name: setup
    description: Start an in-chat OAuth flow for an email provider, sending progress updates to a Telegram chat
    script: ./setup.sh
    args:
      - name: email
        type: string
        required: false
        description: Email address to authenticate (provider is auto-detected from domain)
      - name: provider
        type: string
        required: false
        description: "Email provider: gmail, outlook, yahoo, or custom (default: auto-detect)"
      - name: chat_id
        type: string
        required: true
        description: Telegram chat ID to send progress notifications and auth URLs to
      - name: interactive
        type: boolean
        required: false
        description: "Enable interactive mode: guides user through provider selection step by step"
---

# Email OAuth Chat

Handles complete OAuth 2.0 authentication flows for email providers (Gmail, Outlook, Yahoo) entirely within a Telegram conversation. Users receive authorization links and real-time status updates without leaving their chat client.

## Supported Providers

| Provider | Domains | Flow |
|----------|---------|------|
| Gmail | `@gmail.com` | Authorization Code + PKCE |
| Outlook | `@outlook.com`, `@*.onmicrosoft.com` | Device Code (RFC 8628) |
| Yahoo | `@yahoo.com`, `@yahoo.*` | Authorization Code + PKCE |

## Required Environment Variables

```bash
# Telegram (required for chat notifications)
export TELEGRAM_BOT_TOKEN="your-bot-token"

# Provider credentials (set the ones you need)
export GMAIL_CLIENT_ID="your-gmail-client-id"
export GMAIL_CLIENT_SECRET="your-gmail-client-secret"

export OUTLOOK_CLIENT_ID="your-outlook-client-id"
export OUTLOOK_CLIENT_SECRET="your-outlook-client-secret"

export YAHOO_CLIENT_ID="your-yahoo-client-id"
export YAHOO_CLIENT_SECRET="your-yahoo-client-secret"
```

## Usage

```bash
# Auto-detect Gmail and start OAuth in chat
./setup.sh --email user@gmail.com --chat-id 123456789

# Outlook with device code flow (best for non-browser environments)
./setup.sh --provider outlook --email user@company.com --chat-id 123456789

# Interactive guided mode (prompts user step by step via Telegram)
./setup.sh --interactive --chat-id 123456789
```

## OAuth Flows

### Authorization Code Flow (Gmail, Yahoo)

1. The script generates a PKCE challenge and constructs an authorization URL.
2. The URL is sent to the Telegram chat.
3. The user opens the link and grants access in their browser.
4. The provider redirects to a local callback server (port 8400) with an auth code.
5. The code is exchanged for access and refresh tokens.
6. Success confirmation is sent to Telegram.

### Device Code Flow (Outlook)

1. The script requests a device code from Microsoft.
2. The user code and verification URL are sent to Telegram.
3. The user visits `https://microsoft.com/devicelogin` and enters the code.
4. The script polls until authorization is complete.
5. Tokens are retrieved and stored in the vault.
6. Success confirmation is sent to Telegram.

## Timeouts

| Flow | Timeout |
|------|---------|
| Authorization Code | 300 seconds (5 minutes) |
| Device Code | 900 seconds (15 minutes) |
| Callback Server | 300 seconds (5 minutes) |

## Security

- PKCE prevents authorization code interception attacks.
- Random state parameter provides CSRF protection.
- All tokens are encrypted before storage in the OpenIntentOS vault.
- Minimal OAuth scopes are requested (only what is needed for email access).

## Troubleshooting

- **"Missing environment variables"**: Set the required `*_CLIENT_ID` and `*_CLIENT_SECRET` for your provider.
- **"Authentication timed out"**: Increase timeout configuration or try the device code flow for Outlook.
- **"State mismatch"**: Clear browser cookies and retry; avoid browser extensions on the auth page.
- **"Invalid redirect URI"**: Ensure port 8400 is available and matches the URI registered in your OAuth app.
