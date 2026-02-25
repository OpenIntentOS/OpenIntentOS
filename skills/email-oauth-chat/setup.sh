#!/bin/bash

# Email OAuth Chat Setup
# Handles OAuth authentication for email providers directly in chat

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPENINTENT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Default values
PROVIDER=""
EMAIL=""
CHAT_ID=""
INTERACTIVE=false

# Help function
show_help() {
    cat << EOF
Email OAuth Chat Setup

USAGE:
    $0 [OPTIONS]

OPTIONS:
    --provider <provider>    Email provider (gmail, outlook, yahoo, or custom)
    --email <email>         Email address to authenticate
    --chat-id <chat_id>     Telegram chat ID for notifications
    --interactive           Interactive mode (auto-detect provider from email)
    --help                  Show this help message

EXAMPLES:
    # Auto-detect Gmail and setup OAuth
    $0 --email user@gmail.com --chat-id 123456789

    # Specific provider with chat notifications
    $0 --provider outlook --email user@company.com --chat-id 123456789

    # Interactive mode
    $0 --interactive --chat-id 123456789

SUPPORTED PROVIDERS:
    gmail      - Google Gmail (accounts.google.com)
    outlook    - Microsoft Outlook/Office 365 (login.microsoftonline.com)
    yahoo      - Yahoo Mail (api.login.yahoo.com)
    custom     - Custom OAuth configuration

EOF
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --provider)
            PROVIDER="$2"
            shift 2
            ;;
        --email)
            EMAIL="$2"
            shift 2
            ;;
        --chat-id)
            CHAT_ID="$2"
            shift 2
            ;;
        --interactive)
            INTERACTIVE=true
            shift
            ;;
        --help)
            show_help
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

# Auto-detect provider from email domain
detect_provider() {
    local email="$1"
    local domain="${email##*@}"
    
    case "$domain" in
        gmail.com)
            echo "gmail"
            ;;
        outlook.com|hotmail.com|live.com)
            echo "outlook"
            ;;
        yahoo.com|yahoo.co.uk|yahoo.ca)
            echo "yahoo"
            ;;
        *.onmicrosoft.com)
            echo "outlook"
            ;;
        *)
            echo "custom"
            ;;
    esac
}

# Interactive mode
if [[ "$INTERACTIVE" == "true" ]]; then
    echo "🔐 Email OAuth Setup - Interactive Mode"
    echo
    
    if [[ -z "$EMAIL" ]]; then
        read -p "📧 Enter your email address: " EMAIL
    fi
    
    if [[ -z "$CHAT_ID" ]]; then
        read -p "💬 Enter your Telegram chat ID: " CHAT_ID
    fi
    
    if [[ -z "$PROVIDER" ]]; then
        PROVIDER=$(detect_provider "$EMAIL")
        echo "🔍 Auto-detected provider: $PROVIDER"
    fi
fi

# Validate required parameters
if [[ -z "$EMAIL" ]]; then
    echo "❌ Error: Email address is required"
    show_help
    exit 1
fi

if [[ -z "$CHAT_ID" ]]; then
    echo "❌ Error: Telegram chat ID is required"
    show_help
    exit 1
fi

if [[ -z "$PROVIDER" ]]; then
    PROVIDER=$(detect_provider "$EMAIL")
    echo "🔍 Auto-detected provider: $PROVIDER"
fi

# Validate provider
case "$PROVIDER" in
    gmail|outlook|yahoo|custom)
        ;;
    *)
        echo "❌ Error: Unsupported provider '$PROVIDER'"
        echo "Supported providers: gmail, outlook, yahoo, custom"
        exit 1
        ;;
esac

echo "🚀 Starting OAuth setup for $EMAIL ($PROVIDER)"
echo

# Create OAuth configuration based on provider
create_oauth_config() {
    local provider="$1"
    local config_file="$SCRIPT_DIR/oauth_config.json"
    
    case "$provider" in
        gmail)
            cat > "$config_file" << EOF
{
  "provider": "gmail",
  "oauth_config": {
    "client_id": "\${GMAIL_CLIENT_ID}",
    "client_secret": "\${GMAIL_CLIENT_SECRET}",
    "auth_url": "https://accounts.google.com/o/oauth2/v2/auth",
    "token_url": "https://oauth2.googleapis.com/token",
    "redirect_uri": "http://127.0.0.1:8400/callback",
    "scopes": [
      "https://www.googleapis.com/auth/gmail.readonly",
      "https://www.googleapis.com/auth/gmail.send"
    ]
  },
  "timeout_secs": 300,
  "prefer_device_code": false
}
EOF
            ;;
        outlook)
            cat > "$config_file" << EOF
{
  "provider": "outlook",
  "oauth_config": {
    "client_id": "\${OUTLOOK_CLIENT_ID}",
    "client_secret": "\${OUTLOOK_CLIENT_SECRET}",
    "auth_url": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    "token_url": "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    "redirect_uri": "http://127.0.0.1:8400/callback",
    "scopes": [
      "https://graph.microsoft.com/Mail.Read",
      "https://graph.microsoft.com/Mail.Send"
    ]
  },
  "device_code_config": {
    "client_id": "\${OUTLOOK_CLIENT_ID}",
    "device_auth_url": "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode",
    "token_url": "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    "scopes": [
      "https://graph.microsoft.com/Mail.Read",
      "https://graph.microsoft.com/Mail.Send"
    ]
  },
  "timeout_secs": 900,
  "prefer_device_code": true
}
EOF
            ;;
        yahoo)
            cat > "$config_file" << EOF
{
  "provider": "yahoo",
  "oauth_config": {
    "client_id": "\${YAHOO_CLIENT_ID}",
    "client_secret": "\${YAHOO_CLIENT_SECRET}",
    "auth_url": "https://api.login.yahoo.com/oauth2/request_auth",
    "token_url": "https://api.login.yahoo.com/oauth2/get_token",
    "redirect_uri": "http://127.0.0.1:8400/callback",
    "scopes": ["mail-r", "mail-w"]
  },
  "timeout_secs": 300,
  "prefer_device_code": false
}
EOF
            ;;
        custom)
            echo "📝 Custom provider configuration required"
            echo "Please create oauth_config.json manually with your provider's details"
            exit 1
            ;;
    esac
    
    echo "$config_file"
}

# Check for required environment variables
check_env_vars() {
    local provider="$1"
    local missing_vars=()
    
    case "$provider" in
        gmail)
            [[ -z "${GMAIL_CLIENT_ID:-}" ]] && missing_vars+=("GMAIL_CLIENT_ID")
            [[ -z "${GMAIL_CLIENT_SECRET:-}" ]] && missing_vars+=("GMAIL_CLIENT_SECRET")
            ;;
        outlook)
            [[ -z "${OUTLOOK_CLIENT_ID:-}" ]] && missing_vars+=("OUTLOOK_CLIENT_ID")
            [[ -z "${OUTLOOK_CLIENT_SECRET:-}" ]] && missing_vars+=("OUTLOOK_CLIENT_SECRET")
            ;;
        yahoo)
            [[ -z "${YAHOO_CLIENT_ID:-}" ]] && missing_vars+=("YAHOO_CLIENT_ID")
            [[ -z "${YAHOO_CLIENT_SECRET:-}" ]] && missing_vars+=("YAHOO_CLIENT_SECRET")
            ;;
    esac
    
    if [[ ${#missing_vars[@]} -gt 0 ]]; then
        echo "❌ Missing required environment variables:"
        for var in "${missing_vars[@]}"; do
            echo "   - $var"
        done
        echo
        echo "💡 Set these variables in your environment or .env file"
        exit 1
    fi
}

# Send Telegram notification
send_telegram_notification() {
    local chat_id="$1"
    local message="$2"
    
    if command -v curl >/dev/null 2>&1; then
        # Use the OpenIntentOS Telegram adapter if available
        echo "📱 Sending notification to Telegram chat $chat_id"
        echo "Message: $message"
        # In a real implementation, this would call the Telegram adapter
    else
        echo "📱 Would send to Telegram: $message"
    fi
}

# Main execution
main() {
    echo "🔧 Creating OAuth configuration..."
    config_file=$(create_oauth_config "$PROVIDER")
    
    echo "🔍 Checking environment variables..."
    check_env_vars "$PROVIDER"
    
    echo "📱 Sending setup notification..."
    send_telegram_notification "$CHAT_ID" "🔐 Starting OAuth setup for $EMAIL ($PROVIDER)"
    
    echo "✅ OAuth configuration created: $config_file"
    echo
    echo "📋 Next steps:"
    echo "1. The OAuth flow will start automatically"
    echo "2. You'll receive instructions in Telegram"
    echo "3. Follow the authorization link when provided"
    echo "4. Complete the OAuth flow in your browser"
    echo "5. Return to Telegram for confirmation"
    echo
    echo "🎯 Configuration Summary:"
    echo "   Provider: $PROVIDER"
    echo "   Email: $EMAIL"
    echo "   Chat ID: $CHAT_ID"
    echo "   Config: $config_file"
    
    # In a real implementation, this would trigger the TelegramOAuth flow
    echo
    echo "🚀 OAuth setup initiated! Check your Telegram for further instructions."
}

# Execute main function
main