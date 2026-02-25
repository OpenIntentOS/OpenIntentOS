---
name: "Email OAuth Setup"
description: "Automatically configure OAuth 2.0 authentication for email providers to enable secure, passwordless email access"
version: "1.0.0"
author: "OpenIntentOS"
category: "email"
tags: ["oauth", "email", "authentication", "gmail", "outlook"]
scripts:
  - name: "setup"
    description: "Set up OAuth authentication for an email provider"
    file: "setup.sh"
    parameters:
      - name: "email"
        description: "Email address to configure"
        required: true
        type: "string"
      - name: "provider"
        description: "Email provider (gmail, outlook, yahoo, or auto-detect)"
        required: false
        type: "string"
        default: "auto"
      - name: "scopes"
        description: "Custom OAuth scopes (comma-separated)"
        required: false
        type: "string"
---

# Email OAuth Setup

Automatically configure OAuth 2.0 authentication for email providers to enable secure, passwordless email access.

## Features

- **Auto-detect email provider** (Gmail, Outlook, Yahoo, etc.)
- **Generate OAuth configuration** with proper scopes and endpoints
- **Launch browser authorization flow** with PKCE security
- **Store encrypted tokens** in OpenIntentOS vault
- **Auto-refresh tokens** when expired
- **Support multiple accounts** per provider

## Supported Providers

| Provider | OAuth Endpoint | Scopes |
|----------|----------------|--------|
| **Gmail** | `accounts.google.com` | `https://mail.google.com/` |
| **Outlook/Office 365** | `login.microsoftonline.com` | `https://outlook.office.com/IMAP.AccessAsUser.All` |
| **Yahoo** | `api.login.yahoo.com` | `mail-r` |
| **Custom** | User-defined | User-defined |

## Usage

```bash
# Auto-setup for Gmail
openintent skill email-oauth-setup --provider gmail --email user@gmail.com

# Setup for Office 365 with custom scopes
openintent skill email-oauth-setup --provider outlook --email user@company.com --scopes "Mail.Send,Mail.Read"

# Interactive setup (detect provider)
openintent skill email-oauth-setup --email user@example.com
```

The skill will:
1. **Detect provider** from email domain
2. **Launch OAuth flow** in your default browser
3. **Store tokens securely** in the vault
4. **Test email connection** automatically
5. **Create email credential** for use with email tools