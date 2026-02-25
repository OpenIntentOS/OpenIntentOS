# Email OAuth Chat Setup

Seamless OAuth authentication for email providers directly within Telegram conversations.

## Features

- **🔐 In-Chat OAuth**: Complete OAuth flows without leaving Telegram
- **📧 Multi-Provider**: Gmail, Outlook/Office 365, Yahoo Mail support
- **🤖 Auto-Detection**: Automatically detects provider from email domain
- **🔄 Device Code Flow**: Supports RFC 8628 device authorization grant
- **🔒 Secure Storage**: Encrypted token storage in OpenIntentOS vault
- **⏱️ Smart Timeouts**: Configurable timeouts for different flow types
- **📱 Real-time Updates**: Live progress notifications in Telegram

## Quick Start

### Auto-Detection Mode
```bash
# Automatically detect Gmail and setup OAuth
./setup.sh --email user@gmail.com --chat-id 1554610126
```

### Specific Provider
```bash
# Office 365 with device code flow
./setup.sh --provider outlook --email user@company.com --chat-id 1554610126
```

### Interactive Mode
```bash
# Guided setup with prompts
./setup.sh --interactive --chat-id 1554610126
```

## Supported Providers

| Provider | Domains | Flow Type | Setup Time |
|----------|---------|-----------|------------|
| **Gmail** | `@gmail.com` | Authorization Code + PKCE | ~2 minutes |
| **Outlook** | `@outlook.com`, `@*.onmicrosoft.com` | Device Code (preferred) | ~3 minutes |
| **Yahoo** | `@yahoo.com`, `@yahoo.*` | Authorization Code + PKCE | ~2 minutes |
| **Custom** | Any domain | User-configured | Variable |

## Environment Variables

Set these in your environment or `.env` file:

### Gmail
```bash
GMAIL_CLIENT_ID="your-gmail-client-id"
GMAIL_CLIENT_SECRET="your-gmail-client-secret"
```

### Outlook/Office 365
```bash
OUTLOOK_CLIENT_ID="your-outlook-client-id"
OUTLOOK_CLIENT_SECRET="your-outlook-client-secret"
```

### Yahoo
```bash
YAHOO_CLIENT_ID="your-yahoo-client-id"
YAHOO_CLIENT_SECRET="your-yahoo-client-secret"
```

## How It Works

### 1. **Initiation**
```
User: "自动整理我的邮件并创建待办事项"
Bot: "🔐 Starting Gmail OAuth Authentication..."
```

### 2. **Provider Detection**
- Auto-detects provider from email domain
- Selects optimal OAuth flow (authorization code vs device code)
- Configures appropriate scopes and endpoints

### 3. **OAuth Flow**
**Authorization Code Flow:**
- Generates PKCE verifier and challenge
- Opens local callback server
- Sends authorization URL to Telegram
- Waits for user authorization
- Exchanges code for tokens

**Device Code Flow:**
- Requests device code from provider
- Displays user code and verification URL
- Polls for authorization completion
- Retrieves tokens when authorized

### 4. **Secure Storage**
- Encrypts tokens using OpenIntentOS vault
- Stores with expiration tracking
- Enables automatic refresh

### 5. **Email Automation**
- Uses stored tokens for email access
- Performs intelligent email analysis
- Creates structured todo lists
- Reports results back to Telegram

## OAuth Flow Examples

### Gmail Authorization Code Flow
```
🔐 Starting Gmail OAuth Authentication

I'll guide you through the authentication process.
This will take just a few moments.

🔄 Preparing authorization...

🌐 Please open this URL in your browser:
https://accounts.google.com/o/oauth2/v2/auth?client_id=...

✅ Authentication Successful!

Gmail has been successfully connected to your account.
You can now use email automation features.
```

### Outlook Device Code Flow
```
🔐 Starting Outlook OAuth Authentication

🔄 Requesting device authorization...

📱 Enter this code at the URL shown:
Code: ABCD-EFGH
URL: https://microsoft.com/devicelogin

Or open this URL directly:
https://microsoft.com/devicelogin?user_code=ABCD-EFGH

⏳ Waiting for authorization...

✅ Authentication Successful!

Outlook has been successfully connected to your account.
You can now use email automation features.
```

## Security Features

- **🔒 PKCE Protection**: Prevents authorization code interception
- **🛡️ State Validation**: CSRF protection with random state parameters
- **🔐 Encrypted Storage**: All tokens encrypted at rest
- **⏱️ Automatic Expiry**: Tokens automatically refreshed when needed
- **🚫 Minimal Scopes**: Only requests necessary email permissions

## Configuration

### Custom Provider Setup
For custom email providers, create `oauth_config.json`:

```json
{
  "provider": "custom",
  "oauth_config": {
    "client_id": "your-client-id",
    "client_secret": "your-client-secret",
    "auth_url": "https://your-provider.com/oauth2/authorize",
    "token_url": "https://your-provider.com/oauth2/token",
    "redirect_uri": "http://127.0.0.1:8400/callback",
    "scopes": ["email.read", "email.send"]
  },
  "timeout_secs": 300,
  "prefer_device_code": false
}
```

### Timeout Configuration
- **Authorization Code Flow**: 300 seconds (5 minutes)
- **Device Code Flow**: 900 seconds (15 minutes)
- **Callback Server**: 300 seconds (5 minutes)

## Integration

This skill integrates with:
- **OpenIntentOS Auth Engine**: Core OAuth 2.0 implementation
- **OpenIntentOS Vault**: Encrypted credential storage
- **Telegram Adapter**: Real-time chat notifications
- **Email Adapter**: Automated email processing

## Troubleshooting

### Common Issues

**"Missing environment variables"**
- Set the required `*_CLIENT_ID` and `*_CLIENT_SECRET` variables
- Check your `.env` file or environment configuration

**"Authentication timed out"**
- Increase timeout values in the configuration
- Ensure stable internet connection
- Try the device code flow for better reliability

**"State mismatch error"**
- Clear browser cookies and try again
- Ensure no browser extensions are interfering
- Use incognito/private browsing mode

**"Invalid redirect URI"**
- Ensure port 8400 is available
- Check firewall settings
- Verify OAuth app configuration matches redirect URI

### Debug Mode
```bash
# Enable debug logging
RUST_LOG=debug ./setup.sh --email user@gmail.com --chat-id 123456789
```

## Future Enhancements

- **📊 Multi-Account**: Support multiple email accounts per provider
- **🔄 Background Sync**: Automatic email monitoring and processing
- **🎯 Smart Filters**: AI-powered email categorization
- **📈 Analytics**: Email processing statistics and insights
- **🔗 Calendar Integration**: Automatic event creation from emails
- **📝 Template System**: Customizable email response templates

---

**Ready to automate your email workflow? Start with a simple command and let OpenIntentOS handle the rest!** 🚀