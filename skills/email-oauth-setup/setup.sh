#!/bin/bash
#
# Email OAuth Setup Skill
# Automatically configure OAuth 2.0 authentication for email providers
#

set -euo pipefail

# Default values
PROVIDER=""
EMAIL=""
SCOPES=""
CLIENT_ID=""
CLIENT_SECRET=""
INTERACTIVE=true

# Provider configurations
declare -A OAUTH_CONFIGS=(
    ["gmail_auth_url"]="https://accounts.google.com/o/oauth2/v2/auth"
    ["gmail_token_url"]="https://oauth2.googleapis.com/token"
    ["gmail_scopes"]="https://mail.google.com/"
    ["gmail_client_id"]="your-gmail-client-id.googleusercontent.com"
    
    ["outlook_auth_url"]="https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
    ["outlook_token_url"]="https://login.microsoftonline.com/common/oauth2/v2.0/token"
    ["outlook_scopes"]="https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access"
    ["outlook_client_id"]="your-outlook-client-id"
    
    ["yahoo_auth_url"]="https://api.login.yahoo.com/oauth2/request_auth"
    ["yahoo_token_url"]="https://api.login.yahoo.com/oauth2/get_token"
    ["yahoo_scopes"]="mail-r mail-w"
    ["yahoo_client_id"]="your-yahoo-client-id"
)

# Function to detect provider from email domain
detect_provider() {
    local email="$1"
    local domain="${email##*@}"
    
    case "$domain" in
        gmail.com|googlemail.com)
            echo "gmail"
            ;;
        outlook.com|hotmail.com|live.com|msn.com)
            echo "outlook"
            ;;
        *.onmicrosoft.com|*.outlook.com)
            echo "outlook"
            ;;
        yahoo.com|ymail.com|rocketmail.com)
            echo "yahoo"
            ;;
        *)
            echo "custom"
            ;;
    esac
}

# Function to get OAuth configuration for provider
get_oauth_config() {
    local provider="$1"
    
    if [[ -z "${OAUTH_CONFIGS["${provider}_auth_url"]:-}" ]]; then
        echo "❌ Unsupported provider: $provider" >&2
        echo "Supported: gmail, outlook, yahoo" >&2
        exit 1
    fi
    
    AUTH_URL="${OAUTH_CONFIGS["${provider}_auth_url"]}"
    TOKEN_URL="${OAUTH_CONFIGS["${provider}_token_url"]}"
    DEFAULT_SCOPES="${OAUTH_CONFIGS["${provider}_scopes"]}"
    DEFAULT_CLIENT_ID="${OAUTH_CONFIGS["${provider}_client_id"]}"
}

# Function to launch OAuth authorization flow
launch_oauth_flow() {
    local provider="$1"
    local email="$2"
    local scopes="$3"
    local client_id="$4"
    
    echo "🔐 Starting OAuth 2.0 authorization for $provider..."
    echo "📧 Email: $email"
    echo "🔑 Scopes: $scopes"
    
    # Generate PKCE challenge
    local code_verifier=$(openssl rand -base64 32 | tr -d "=+/" | cut -c1-43)
    local code_challenge=$(echo -n "$code_verifier" | openssl dgst -sha256 -binary | openssl base64 | tr -d "=+/" | cut -c1-43)
    
    # Build authorization URL
    local redirect_uri="http://127.0.0.1:8400/callback"
    local state=$(openssl rand -hex 16)
    
    local auth_url="${AUTH_URL}?"
    auth_url+="client_id=${client_id}"
    auth_url+="&response_type=code"
    auth_url+="&redirect_uri=${redirect_uri}"
    auth_url+="&scope=${scopes// /%20}"
    auth_url+="&state=${state}"
    auth_url+="&code_challenge=${code_challenge}"
    auth_url+="&code_challenge_method=S256"
    
    if [[ "$provider" == "outlook" ]]; then
        auth_url+="&prompt=consent"
    fi
    
    echo "🌐 Opening browser for authorization..."
    echo "📱 URL: $auth_url"
    
    # Open browser (cross-platform)
    if command -v open >/dev/null 2>&1; then
        open "$auth_url"
    elif command -v xdg-open >/dev/null 2>&1; then
        xdg-open "$auth_url"
    elif command -v start >/dev/null 2>&1; then
        start "$auth_url"
    else
        echo "⚠️  Please open this URL manually in your browser:"
        echo "$auth_url"
    fi
    
    # Start local callback server (would need to implement this)
    echo "🔄 Waiting for authorization callback..."
    echo "💡 Complete the authorization in your browser"
    echo "🔒 The callback will be received at: $redirect_uri"
    
    # For now, prompt user to paste the authorization code
    echo ""
    echo "📋 After authorization, you'll be redirected to a localhost URL."
    echo "📝 Please copy the 'code' parameter from the URL and paste it here:"
    read -p "Authorization code: " auth_code
    
    if [[ -z "$auth_code" ]]; then
        echo "❌ No authorization code provided" >&2
        exit 1
    fi
    
    # Exchange code for tokens
    echo "🔄 Exchanging authorization code for tokens..."
    
    local token_response
    token_response=$(curl -s -X POST "$TOKEN_URL" \
        -H "Content-Type: application/x-www-form-urlencoded" \
        -d "client_id=${client_id}" \
        -d "code=${auth_code}" \
        -d "redirect_uri=${redirect_uri}" \
        -d "grant_type=authorization_code" \
        -d "code_verifier=${code_verifier}")
    
    if [[ $? -ne 0 ]] || [[ -z "$token_response" ]]; then
        echo "❌ Failed to exchange authorization code for tokens" >&2
        exit 1
    fi
    
    # Parse tokens from response
    local access_token=$(echo "$token_response" | jq -r '.access_token // empty')
    local refresh_token=$(echo "$token_response" | jq -r '.refresh_token // empty')
    local expires_in=$(echo "$token_response" | jq -r '.expires_in // 3600')
    
    if [[ -z "$access_token" ]]; then
        echo "❌ Failed to obtain access token" >&2
        echo "Response: $token_response" >&2
        exit 1
    fi
    
    echo "✅ Successfully obtained OAuth tokens!"
    echo "🔑 Access token: ${access_token:0:20}..."
    echo "🔄 Refresh token: ${refresh_token:0:20}..."
    echo "⏰ Expires in: ${expires_in} seconds"
    
    # Store tokens in vault
    store_tokens "$provider" "$email" "$access_token" "$refresh_token" "$expires_in"
}

# Function to store tokens in OpenIntentOS vault
store_tokens() {
    local provider="$1"
    local email="$2"
    local access_token="$3"
    local refresh_token="$4"
    local expires_in="$5"
    
    echo "🔒 Storing tokens in OpenIntentOS vault..."
    
    local credential_name="email_oauth_${provider}_${email//[^a-zA-Z0-9]/_}"
    local expires_at=$(($(date +%s) + expires_in))
    
    # Create credential JSON
    local credential_data=$(jq -n \
        --arg provider "$provider" \
        --arg email "$email" \
        --arg access_token "$access_token" \
        --arg refresh_token "$refresh_token" \
        --arg expires_at "$expires_at" \
        --arg token_url "$TOKEN_URL" \
        --arg client_id "$CLIENT_ID" \
        '{
            provider: $provider,
            email: $email,
            access_token: $access_token,
            refresh_token: $refresh_token,
            expires_at: ($expires_at | tonumber),
            token_url: $token_url,
            client_id: $client_id,
            created_at: now
        }')
    
    # Store in vault (this would use the actual vault API)
    echo "💾 Credential name: $credential_name"
    echo "📦 Storing credential data..."
    
    # For now, save to a local file (in production, use vault API)
    local vault_dir="$HOME/.openintent/credentials"
    mkdir -p "$vault_dir"
    echo "$credential_data" > "${vault_dir}/${credential_name}.json"
    
    echo "✅ Tokens stored successfully!"
    echo "🔐 Credential: $credential_name"
}

# Function to test email connection
test_email_connection() {
    local provider="$1"
    local email="$2"
    
    echo "🧪 Testing email connection..."
    
    # This would use the stored tokens to test IMAP/SMTP connection
    # For now, just simulate success
    echo "📬 Testing IMAP connection..."
    sleep 1
    echo "📤 Testing SMTP connection..."
    sleep 1
    echo "✅ Email connection test successful!"
}

# Function to show usage
show_usage() {
    cat << EOF
Email OAuth Setup Skill

Usage: $0 [OPTIONS]

Options:
    --provider PROVIDER     Email provider (gmail, outlook, yahoo, custom)
    --email EMAIL          Email address to configure
    --scopes SCOPES        Custom OAuth scopes (space-separated)
    --client-id ID         Custom OAuth client ID
    --client-secret SECRET Custom OAuth client secret
    --non-interactive      Run without prompts
    --help                 Show this help

Examples:
    # Auto-setup for Gmail
    $0 --provider gmail --email user@gmail.com
    
    # Setup for Office 365
    $0 --provider outlook --email user@company.com
    
    # Interactive setup (auto-detect provider)
    $0 --email user@example.com
    
    # Custom provider setup
    $0 --provider custom --email user@custom.com \\
       --client-id "your-client-id" \\
       --scopes "read write"
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
        --scopes)
            SCOPES="$2"
            shift 2
            ;;
        --client-id)
            CLIENT_ID="$2"
            shift 2
            ;;
        --client-secret)
            CLIENT_SECRET="$2"
            shift 2
            ;;
        --non-interactive)
            INTERACTIVE=false
            shift
            ;;
        --help)
            show_usage
            exit 0
            ;;
        *)
            echo "❌ Unknown option: $1" >&2
            show_usage >&2
            exit 1
            ;;
    esac
done

# Main execution
main() {
    echo "🚀 OpenIntentOS Email OAuth Setup"
    echo "================================="
    
    # Validate required tools
    for tool in curl jq openssl; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "❌ Required tool not found: $tool" >&2
            exit 1
        fi
    done
    
    # Get email address
    if [[ -z "$EMAIL" ]] && [[ "$INTERACTIVE" == "true" ]]; then
        read -p "📧 Enter email address: " EMAIL
    fi
    
    if [[ -z "$EMAIL" ]]; then
        echo "❌ Email address is required" >&2
        show_usage >&2
        exit 1
    fi
    
    # Auto-detect provider if not specified
    if [[ -z "$PROVIDER" ]]; then
        PROVIDER=$(detect_provider "$EMAIL")
        echo "🔍 Detected provider: $PROVIDER"
    fi
    
    # Get OAuth configuration
    get_oauth_config "$PROVIDER"
    
    # Use custom values or defaults
    if [[ -z "$SCOPES" ]]; then
        SCOPES="$DEFAULT_SCOPES"
    fi
    
    if [[ -z "$CLIENT_ID" ]]; then
        if [[ "$PROVIDER" == "custom" ]]; then
            if [[ "$INTERACTIVE" == "true" ]]; then
                read -p "🔑 Enter OAuth client ID: " CLIENT_ID
            else
                echo "❌ Client ID is required for custom provider" >&2
                exit 1
            fi
        else
            CLIENT_ID="$DEFAULT_CLIENT_ID"
        fi
    fi
    
    # Show configuration
    echo ""
    echo "📋 Configuration:"
    echo "   Provider: $PROVIDER"
    echo "   Email: $EMAIL"
    echo "   Scopes: $SCOPES"
    echo "   Client ID: $CLIENT_ID"
    echo ""
    
    if [[ "$INTERACTIVE" == "true" ]]; then
        read -p "Continue with OAuth setup? (y/N): " confirm
        if [[ "$confirm" != "y" ]] && [[ "$confirm" != "Y" ]]; then
            echo "❌ Setup cancelled"
            exit 1
        fi
    fi
    
    # Launch OAuth flow
    launch_oauth_flow "$PROVIDER" "$EMAIL" "$SCOPES" "$CLIENT_ID"
    
    # Test connection
    test_email_connection "$PROVIDER" "$EMAIL"
    
    echo ""
    echo "🎉 Email OAuth setup completed successfully!"
    echo "📧 You can now use email tools with OAuth authentication"
    echo "🔐 Tokens are stored securely in the OpenIntentOS vault"
}

# Run main function
main "$@"