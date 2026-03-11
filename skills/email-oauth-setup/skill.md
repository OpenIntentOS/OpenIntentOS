---
name: email-oauth-setup
description: Configure OAuth 2.0 authentication for Gmail, Outlook, or Yahoo email accounts
version: 1.0.0
author: OpenIntentOS
tags: [email, oauth, authentication, gmail, outlook, yahoo]
requires:
  bins: [bash, curl, python3]
  env:
    - GMAIL_CLIENT_ID
    - GMAIL_CLIENT_SECRET
tools:
  - name: setup
    description: Run interactive OAuth setup for an email provider, launching a browser auth flow and storing tokens in the vault
    script: ./setup.sh
    args:
      - name: email
        type: string
        required: true
        description: Email address to configure (provider is auto-detected from domain)
      - name: provider
        type: string
        required: false
        description: "Override provider detection: gmail, outlook, or yahoo (default: auto)"
      - name: scopes
        type: string
        required: false
        description: Comma-separated custom OAuth scopes to request (optional override)
---

# Email OAuth Setup

Automatically configures OAuth 2.0 authentication for email providers (Gmail, Outlook/Office 365, Yahoo). After running setup, tokens are stored in the OpenIntentOS vault and email tools can authenticate without passwords.

## Supported Providers

| Provider | Domains | OAuth Endpoint |
|----------|---------|----------------|
| Gmail | `@gmail.com` | `accounts.google.com` |
| Outlook | `@outlook.com`, `@*.onmicrosoft.com` | `login.microsoftonline.com` |
| Yahoo | `@yahoo.com`, `@yahoo.*` | `api.login.yahoo.com` |

## Required Environment Variables

Set the credentials for the provider you want to configure:

```bash
# Gmail
export GMAIL_CLIENT_ID="your-client-id.googleusercontent.com"
export GMAIL_CLIENT_SECRET="your-client-secret"

# Outlook
export OUTLOOK_CLIENT_ID="your-outlook-client-id"
export OUTLOOK_CLIENT_SECRET="your-outlook-client-secret"

# Yahoo
export YAHOO_CLIENT_ID="your-yahoo-client-id"
export YAHOO_CLIENT_SECRET="your-yahoo-client-secret"
```

Obtain credentials from each provider's developer console.

## Usage

```bash
# Auto-detect provider from email domain
./setup.sh --email user@gmail.com

# Specify provider explicitly
./setup.sh --provider outlook --email user@company.com

# Custom scopes
./setup.sh --email user@gmail.com --scopes "https://mail.google.com/,https://www.googleapis.com/auth/calendar"
```

## What Happens

1. Provider is detected from the email domain (or overridden by --provider).
2. The script launches a local OAuth callback server and opens the authorization URL in your browser.
3. After you grant permission, the authorization code is exchanged for access and refresh tokens.
4. Tokens are stored in the OpenIntentOS vault, encrypted at rest.
5. The email connection is tested automatically.

## Security

- Uses PKCE (Proof Key for Code Exchange) to prevent authorization code interception.
- CSRF protection via random state parameter.
- Only the minimum required scopes are requested.
- All tokens are encrypted before storage.
