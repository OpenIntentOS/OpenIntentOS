#!/usr/bin/env bash
# OpenIntentOS Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.sh | bash
#
# Supported platforms:
#   macOS       — Apple Silicon (M1/M2/M3/M4), Intel
#   Linux       — x86_64, ARM64 (Raspberry Pi 4/5)
#   WSL         — Windows Subsystem for Linux (uses Linux binary)
#   Android     — Termux (uses ARM64 Linux binary)
#   FreeBSD     — x86_64, ARM64 (builds from source)
set -euo pipefail

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; DIM='\033[2m'; NC='\033[0m'

ok()   { echo -e "  ${GREEN}✓${NC}  $*"; }
info() { echo -e "  ${CYAN}→${NC}  $*"; }
warn() { echo -e "  ${YELLOW}!${NC}  $*"; }
die()  { echo -e "\n  ${RED}✗  ERROR:${NC} $*\n"; exit 1; }
hr()   { echo -e "${DIM}────────────────────────────────────────────────────────${NC}"; }

# ── Paths ─────────────────────────────────────────────────────────────────────
REPO="OpenIntentOS/OpenIntentOS"
INSTALL_DIR="$HOME/.openintentos"
BIN="$INSTALL_DIR/openintent"
CONFIG_DIR="$INSTALL_DIR/config"
CONFIG_FILE="$CONFIG_DIR/default.toml"
ENV_FILE="$INSTALL_DIR/.env"
SKILLS_DIR="$INSTALL_DIR/skills"
DATA_DIR="$INSTALL_DIR/data"
LOG_FILE="$INSTALL_DIR/bot.log"
PID_FILE="$INSTALL_DIR/bot.pid"

# ── Banner ────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}"
echo "   ╔═══════════════════════════════════════════════╗"
echo "   ║         OpenIntentOS  Installer               ║"
echo "   ║     Intent-Driven AI OS — Full Rust           ║"
echo "   ╚═══════════════════════════════════════════════╝"
echo -e "${NC}"
hr

# ── Step 1: OS / Arch detection ───────────────────────────────────────────────
echo -e "\n${BOLD}Step 1/5 · Detecting your system${NC}\n"

OS="$(uname -s)"
ARCH="$(uname -m)"
IS_WSL=false
IS_TERMUX=false
BUILD_FROM_SOURCE=false

# Detect WSL (Windows Subsystem for Linux)
if grep -qi microsoft /proc/version 2>/dev/null || \
   grep -qi microsoft /proc/sys/kernel/osrelease 2>/dev/null; then
  IS_WSL=true
fi

# Detect Termux (Android)
if [ -n "${TERMUX_VERSION:-}" ] || [ -d "/data/data/com.termux" ]; then
  IS_TERMUX=true
fi

case "$OS" in
  Darwin)
    PLATFORM="macos"
    case "$ARCH" in
      arm64)  TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *)      die "Unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  Linux)
    PLATFORM="linux"
    case "$ARCH" in
      x86_64)          TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64)   TARGET="aarch64-unknown-linux-gnu" ;;
      armv7l|armv7)    TARGET="armv7-unknown-linux-gnueabihf" ; BUILD_FROM_SOURCE=true ;;
      armv6l)          TARGET="arm-unknown-linux-gnueabihf" ; BUILD_FROM_SOURCE=true ;;
      i686|i386)       TARGET="i686-unknown-linux-gnu"       ; BUILD_FROM_SOURCE=true ;;
      riscv64)         TARGET="riscv64gc-unknown-linux-gnu"  ; BUILD_FROM_SOURCE=true ;;
      *)               die "Unsupported Linux architecture: $ARCH. Try building from source: https://github.com/$REPO" ;;
    esac
    ;;
  FreeBSD)
    PLATFORM="freebsd"
    BUILD_FROM_SOURCE=true
    case "$ARCH" in
      amd64)   TARGET="x86_64-unknown-freebsd" ;;
      aarch64) TARGET="aarch64-unknown-freebsd" ;;
      *)       die "Unsupported FreeBSD architecture: $ARCH" ;;
    esac
    ;;
  *)
    echo ""
    echo -e "  ${YELLOW}Unsupported OS: $OS${NC}"
    echo ""
    echo -e "  ${BOLD}Windows users:${NC} run this instead in PowerShell:"
    echo ""
    echo -e "  ${CYAN}  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex${NC}"
    echo ""
    exit 1
    ;;
esac

# Friendly OS label
OS_LABEL="$OS ($ARCH)"
$IS_WSL      && OS_LABEL="Windows WSL ($ARCH)"
$IS_TERMUX   && OS_LABEL="Android / Termux ($ARCH)"

ok "Detected: $OS_LABEL → target: $TARGET"
$IS_WSL    && info "WSL detected — using Linux binary (works natively in WSL)"
$IS_TERMUX && info "Termux detected — using Linux ARM64 binary"
$BUILD_FROM_SOURCE && warn "No prebuilt binary for $ARCH — will compile from source"

# ── Step 2: Download binary ───────────────────────────────────────────────────
echo -e "\n${BOLD}Step 2/5 · Downloading OpenIntentOS${NC}\n"

# Get latest release tag
LATEST_TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\(.*\)".*/\1/' || true)

mkdir -p "$INSTALL_DIR" "$CONFIG_DIR" "$DATA_DIR" "$SKILLS_DIR"

if [ -n "$LATEST_TAG" ]; then
  # Download prebuilt binary
  BINARY_URL="https://github.com/$REPO/releases/download/$LATEST_TAG/openintent-$TARGET.tar.gz"
  info "Downloading openintent $LATEST_TAG for $TARGET ..."

  if curl -fsSL "$BINARY_URL" -o /tmp/openintent.tar.gz 2>/dev/null; then
    tar -xzf /tmp/openintent.tar.gz -C "$INSTALL_DIR"
    chmod +x "$BIN"
    rm -f /tmp/openintent.tar.gz
    ok "Downloaded $(du -sh "$BIN" | cut -f1) binary ($LATEST_TAG)"
  else
    warn "Prebuilt binary not found — will build from source instead"
    LATEST_TAG=""
  fi
fi

if [ -z "$LATEST_TAG" ] || [ ! -f "$BIN" ]; then
  # Fall back: build from source
  echo ""
  warn "No prebuilt binary available. Building from source (~5 min)..."
  echo ""

  # Install Rust if needed
  if ! command -v cargo &>/dev/null; then
    info "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path \
      2>&1 | grep -E "(Downloading|Installing|installed)" | sed 's/^/     /'
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    ok "Rust installed"
  else
    ok "Rust already installed ($(rustc --version))"
  fi

  # Clone repo
  REPO_DIR="/tmp/openintentos-build"
  rm -rf "$REPO_DIR"
  info "Cloning repository..."
  git clone --depth 1 "https://github.com/$REPO.git" "$REPO_DIR" \
    2>&1 | grep -E "(Cloning|done)" | sed 's/^/     /'

  # Build
  info "Building release binary (this takes a few minutes)..."
  cd "$REPO_DIR"
  cargo build --release --bin openintent 2>&1 \
    | grep -E "(Compiling openintent|Finished|error)" | tail -20 | sed 's/^/     /'

  cp "$REPO_DIR/target/release/openintent" "$BIN"
  chmod +x "$BIN"
  cd - >/dev/null
  rm -rf "$REPO_DIR"
  ok "Build complete"
fi

# ── Download config and skills from repo ──────────────────────────────────────
info "Downloading default configuration..."
CONFIG_RAW="https://raw.githubusercontent.com/$REPO/main/config/default.toml"
curl -fsSL "$CONFIG_RAW" -o "$CONFIG_FILE" 2>/dev/null \
  && ok "Configuration ready" \
  || warn "Could not download config — will use built-in defaults"

# Download built-in skills from repo
SKILLS_BASE="https://raw.githubusercontent.com/$REPO/main/skills"
for skill in weather-check email-automation web-search-plus ip-lookup; do
  mkdir -p "$SKILLS_DIR/$skill"
  curl -fsSL "$SKILLS_BASE/$skill/SKILL.md" \
    -o "$SKILLS_DIR/$skill/SKILL.md" 2>/dev/null || true
done

# ── Step 3: API key setup ─────────────────────────────────────────────────────
# Check if all required values are already set via environment variables.
# This enables fully silent/automated installation:
#   TELEGRAM_BOT_TOKEN=xxx OPENAI_API_KEY=sk-xxx curl -fsSL .../install.sh | bash
SILENT_INSTALL=false
if [ -n "${TELEGRAM_BOT_TOKEN:-}" ]; then
  # At least the Telegram token is pre-set — check if an AI key is also present
  if [ -n "${OPENAI_API_KEY:-}" ] || [ -n "${NVIDIA_API_KEY:-}" ] || \
     [ -n "${GOOGLE_API_KEY:-}" ] || [ -n "${DEEPSEEK_API_KEY:-}" ] || \
     [ -n "${GROQ_API_KEY:-}" ] || [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    SILENT_INSTALL=true
    echo -e "\n${BOLD}Step 3/5 · Credentials${NC}\n"
    ok "All credentials detected from environment — skipping prompts"
    ok "Telegram token: set"
    for provider in OPENAI NVIDIA GOOGLE DEEPSEEK GROQ ANTHROPIC; do
      key_var="${provider}_API_KEY"
      [ -n "${!key_var:-}" ] && ok "${provider} API key: set"
    done
  fi
fi

if ! $SILENT_INSTALL; then
  echo -e "\n${BOLD}Step 3/5 · Connect your AI providers${NC}\n"
  echo -e "${DIM}  All values are saved locally in $ENV_FILE"
  echo -e "  Never sent anywhere except the AI APIs you choose.${NC}\n"

  # Read from terminal even when piped (fallback to stdin if /dev/tty unavailable)
  if [ -e /dev/tty ] && exec 3</dev/tty 2>/dev/null; then
    : # /dev/tty opened on fd 3
  else
    exec 3<&0  # fall back to stdin (keys will be visible but at least won't crash)
    warn "/dev/tty not available — input will be visible. You can edit $ENV_FILE after install."
  fi
fi

prompt_secret() {
  local var_name="$1"
  local label="$2"
  local url="$3"
  local required="${4:-optional}"

  echo -e "  ${BOLD}${label}${NC}"
  if [ -n "$url" ]; then
    echo -e "  ${DIM}Get it at: ${url}${NC}"
  fi
  if [ "$required" = "required" ]; then
    echo -e "  ${YELLOW}(required)${NC}"
  else
    echo -e "  ${DIM}(optional — press Enter to skip)${NC}"
  fi
  printf "  Enter: "
  # -s hides input so API keys are not visible on screen
  read -rs value <&3
  echo ""  # newline after hidden input

  if [ -n "$value" ]; then
    printf -v "$var_name" '%s' "$value"
    ok "$label saved"
  elif [ "$required" = "required" ]; then
    warn "Skipped — you can add this later by editing $ENV_FILE"
    printf -v "$var_name" '%s' ""
  else
    printf -v "$var_name" '%s' ""
  fi
}

# ── Telegram (required) ────────────────────────────────────────────────────────
OPENAI_API_KEY="${OPENAI_API_KEY:-}"; NVIDIA_API_KEY="${NVIDIA_API_KEY:-}"
GOOGLE_API_KEY="${GOOGLE_API_KEY:-}"; DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY:-}"
GROQ_API_KEY="${GROQ_API_KEY:-}"; ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"

if ! $SILENT_INSTALL; then
  echo -e "  ${CYAN}📱 Telegram Bot${NC}\n"
  echo -e "  ${DIM}Don't have a bot yet? Here's how:"
  echo -e "    1. Open Telegram, search for @BotFather"
  echo -e "    2. Send: /newbot"
  echo -e "    3. Choose a name and username"
  echo -e "    4. Copy the token it gives you${NC}\n"
  prompt_secret TELEGRAM_BOT_TOKEN "Telegram Bot Token" "https://t.me/BotFather" "required"
fi

# ── Primary LLM provider — smart menu (interactive only) ──────────────────────

# Check if Ollama is running locally (zero-config AI option)
OLLAMA_AVAILABLE=false
if curl -s --max-time 2 http://localhost:11434/api/tags >/dev/null 2>&1; then
  OLLAMA_AVAILABLE=true
fi

if ! $SILENT_INSTALL; then
  echo -e "  ${CYAN}🧠 AI Provider${NC}\n"

  if $OLLAMA_AVAILABLE; then
    ok "Ollama detected on this machine — no API key needed!"
    echo -e "  ${DIM}  The bot will use your local Ollama models by default."
    echo -e "  You can still add a cloud API key for more powerful models.${NC}\n"
  fi

  echo -e "  ${DIM}Which AI provider do you have? (Enter a number)${NC}\n"
  echo -e "  ${BOLD}  1)${NC} OpenAI          ${DIM}(ChatGPT — paid, most popular)${NC}"
  echo -e "  ${BOLD}  2)${NC} Google Gemini   ${DIM}(free tier available at aistudio.google.com)${NC}"
  echo -e "  ${BOLD}  3)${NC} Groq            ${DIM}(free tier, very fast — console.groq.com)${NC}"
  echo -e "  ${BOLD}  4)${NC} NVIDIA NIM      ${DIM}(free \$100 credit for new accounts — build.nvidia.com)${NC}"
  echo -e "  ${BOLD}  5)${NC} DeepSeek        ${DIM}(very affordable — platform.deepseek.com)${NC}"
  if $OLLAMA_AVAILABLE; then
    echo -e "  ${BOLD}  0)${NC} Use local Ollama only ${DIM}(already detected — free, no internet)${NC}"
  fi
  echo -e "  ${BOLD}  s)${NC} Skip for now   ${DIM}(add key later by editing $ENV_FILE)${NC}"
  echo ""
  printf "  Your choice: "
  read -r ai_choice <&3
  echo ""

  case "$ai_choice" in
    1)
      echo -e "  ${DIM}Get your key at: https://platform.openai.com/api-keys${NC}"
      prompt_secret OPENAI_API_KEY "OpenAI API Key" "" "required"
      ;;
    2)
      echo -e "  ${DIM}Get your free key at: https://aistudio.google.com/apikey${NC}"
      prompt_secret GOOGLE_API_KEY "Google Gemini API Key" "" "required"
      ;;
    3)
      echo -e "  ${DIM}Get your free key at: https://console.groq.com/keys${NC}"
      prompt_secret GROQ_API_KEY "Groq API Key" "" "required"
      ;;
    4)
      echo -e "  ${DIM}Get your free key at: https://build.nvidia.com${NC}"
      prompt_secret NVIDIA_API_KEY "NVIDIA NIM API Key" "" "required"
      ;;
    5)
      echo -e "  ${DIM}Get your key at: https://platform.deepseek.com${NC}"
      prompt_secret DEEPSEEK_API_KEY "DeepSeek API Key" "" "required"
      ;;
    0)
      ok "Using local Ollama — no key needed"
      ;;
    s|S|"")
      warn "Skipped — add a key later by editing: $ENV_FILE"
      ;;
    *)
      warn "Unrecognized choice — skipping. Add a key later by editing: $ENV_FILE"
      ;;
  esac

  exec 3<&-
fi

# Validate at least one option available
ALL_KEYS_EMPTY=true
for k in "$OPENAI_API_KEY" "$NVIDIA_API_KEY" "$GOOGLE_API_KEY" "$DEEPSEEK_API_KEY" "$GROQ_API_KEY"; do
  [ -n "$k" ] && ALL_KEYS_EMPTY=false
done
if $ALL_KEYS_EMPTY && ! $OLLAMA_AVAILABLE; then
  echo ""
  warn "No AI key provided and Ollama is not running."
  warn "The bot will start but won't be able to answer until you add a key."
  warn "Edit $ENV_FILE and run $INSTALL_DIR/restart.sh"
  echo ""
fi

# Write .env file
cat > "$ENV_FILE" <<EOF
# OpenIntentOS Configuration
# Edit this file to update your API keys, then restart the bot.
# Run: $INSTALL_DIR/restart.sh

# ── Telegram ──────────────────────────────────────────────────────────────────
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"

# ── AI Providers (cascade failover — first available key wins) ─────────────────
OPENAI_API_KEY="${OPENAI_API_KEY:-}"
NVIDIA_API_KEY="${NVIDIA_API_KEY:-}"
GOOGLE_API_KEY="${GOOGLE_API_KEY:-}"
DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY:-}"
GROQ_API_KEY="${GROQ_API_KEY:-}"

# ── Optional Integrations ──────────────────────────────────────────────────────
GITHUB_TOKEN="${GITHUB_TOKEN:-}"
DISCORD_BOT_TOKEN="${DISCORD_BOT_TOKEN:-}"
EOF

chmod 600 "$ENV_FILE"
ok "Credentials saved to $ENV_FILE"

# ── Step 4: System service setup ──────────────────────────────────────────────
echo -e "\n${BOLD}Step 4/5 · Installing system service${NC}\n"
info "Setting up auto-start service (starts on login, restarts on crash)..."

write_helper_scripts() {
  # status.sh
  cat > "$INSTALL_DIR/status.sh" <<'SCRIPT'
#!/usr/bin/env bash
PID_FILE="$HOME/.openintentos/bot.pid"
LOG_FILE="$HOME/.openintentos/bot.log"
if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
  echo "✓ OpenIntentOS is running (PID $(cat "$PID_FILE"))"
else
  echo "✗ OpenIntentOS is not running"
fi
echo "--- last 20 log lines ---"
tail -20 "$LOG_FILE" 2>/dev/null || echo "(no log yet)"
SCRIPT

  # restart.sh
  cat > "$INSTALL_DIR/restart.sh" <<SCRIPT
#!/usr/bin/env bash
echo "Restarting OpenIntentOS..."
PID_FILE="\$HOME/.openintentos/bot.pid"
if [ -f "\$PID_FILE" ]; then
  kill "\$(cat "\$PID_FILE")" 2>/dev/null || true
  sleep 1
fi
source "\$HOME/.openintentos/.env"
export TELEGRAM_BOT_TOKEN OPENAI_API_KEY NVIDIA_API_KEY GOOGLE_API_KEY
export DEEPSEEK_API_KEY GROQ_API_KEY GITHUB_TOKEN DISCORD_BOT_TOKEN
cd "\$HOME/.openintentos"
nohup ./openintent bot >> bot.log 2>&1 &
echo \$! > "\$PID_FILE"
echo "✓ Restarted (PID \$(cat "\$PID_FILE"))"
SCRIPT

  # uninstall.sh
  cat > "$INSTALL_DIR/uninstall.sh" <<'SCRIPT'
#!/usr/bin/env bash
echo "Uninstalling OpenIntentOS..."
PID_FILE="$HOME/.openintentos/bot.pid"
[ -f "$PID_FILE" ] && kill "$(cat "$PID_FILE")" 2>/dev/null || true

OS="$(uname -s)"
if [ "$OS" = "Darwin" ]; then
  launchctl unload ~/Library/LaunchAgents/io.openintentos.bot.plist 2>/dev/null || true
  rm -f ~/Library/LaunchAgents/io.openintentos.bot.plist
elif [ "$OS" = "Linux" ] && command -v systemctl &>/dev/null; then
  systemctl --user stop openintentos 2>/dev/null || true
  systemctl --user disable openintentos 2>/dev/null || true
  rm -f ~/.config/systemd/user/openintentos.service
fi

rm -rf "$HOME/.openintentos"
echo "✓ OpenIntentOS uninstalled. Data deleted."
SCRIPT

  chmod +x "$INSTALL_DIR/status.sh" "$INSTALL_DIR/restart.sh" "$INSTALL_DIR/uninstall.sh"
}

write_helper_scripts

if [ "$PLATFORM" = "macos" ]; then
  PLIST="$HOME/Library/LaunchAgents/io.openintentos.bot.plist"
  mkdir -p "$HOME/Library/LaunchAgents"
  cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>io.openintentos.bot</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>bot</string>
  </array>
  <key>WorkingDirectory</key>
  <string>$INSTALL_DIR</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>TELEGRAM_BOT_TOKEN</key><string>${TELEGRAM_BOT_TOKEN:-}</string>
    <key>OPENAI_API_KEY</key><string>${OPENAI_API_KEY:-}</string>
    <key>NVIDIA_API_KEY</key><string>${NVIDIA_API_KEY:-}</string>
    <key>GOOGLE_API_KEY</key><string>${GOOGLE_API_KEY:-}</string>
    <key>DEEPSEEK_API_KEY</key><string>${DEEPSEEK_API_KEY:-}</string>
    <key>GROQ_API_KEY</key><string>${GROQ_API_KEY:-}</string>
    <key>GITHUB_TOKEN</key><string>${GITHUB_TOKEN:-}</string>
    <key>DISCORD_BOT_TOKEN</key><string>${DISCORD_BOT_TOKEN:-}</string>
  </dict>
  <key>StandardOutPath</key>
  <string>$LOG_FILE</string>
  <key>StandardErrorPath</key>
  <string>$LOG_FILE</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
</dict>
</plist>
PLIST

  launchctl unload "$PLIST" 2>/dev/null || true
  launchctl load "$PLIST"
  ok "macOS LaunchAgent installed (auto-starts on login)"

elif [ "$PLATFORM" = "linux" ]; then
  if command -v systemctl &>/dev/null; then
    SERVICE_DIR="$HOME/.config/systemd/user"
    mkdir -p "$SERVICE_DIR"
    cat > "$SERVICE_DIR/openintentos.service" <<SERVICE
[Unit]
Description=OpenIntentOS AI Bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
ExecStart=$BIN bot
Restart=always
RestartSec=10
StandardOutput=append:$LOG_FILE
StandardError=append:$LOG_FILE
EnvironmentFile=$ENV_FILE

[Install]
WantedBy=default.target
SERVICE

    systemctl --user daemon-reload
    systemctl --user enable openintentos
    systemctl --user start openintentos
    ok "systemd user service installed (auto-starts on login)"
  else
    # Fallback: cron @reboot
    (crontab -l 2>/dev/null | grep -v openintentos; \
     echo "@reboot source $ENV_FILE && cd $INSTALL_DIR && nohup ./openintent bot >> $LOG_FILE 2>&1 &") \
     | crontab -
    ok "Cron @reboot entry installed"

    # Start now
    source "$ENV_FILE"
    cd "$INSTALL_DIR"
    nohup ./openintent bot >> "$LOG_FILE" 2>&1 &
    echo $! > "$PID_FILE"
  fi
fi

# ── Add binary to PATH ────────────────────────────────────────────────────────
PATH_LINE="export PATH=\"\$HOME/.openintentos:\$PATH\""
for rc_file in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
  if [ -f "$rc_file" ] && ! grep -q ".openintentos" "$rc_file" 2>/dev/null; then
    echo "" >> "$rc_file"
    echo "# OpenIntentOS" >> "$rc_file"
    echo "$PATH_LINE" >> "$rc_file"
  fi
done
export PATH="$HOME/.openintentos:$PATH"

# ── Step 5: Verify ────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Step 5/5 · Verifying bot is running${NC}\n"
info "Waiting for bot to connect to Telegram..."
sleep 10

BOT_RUNNING=false
if [ "$PLATFORM" = "macos" ]; then
  launchctl list | grep -q "io.openintentos.bot" && BOT_RUNNING=true
elif [ "$PLATFORM" = "linux" ] && command -v systemctl &>/dev/null; then
  systemctl --user is-active --quiet openintentos && BOT_RUNNING=true
fi

if [ -f "$LOG_FILE" ] && grep -q "Bot is running" "$LOG_FILE" 2>/dev/null; then
  BOT_RUNNING=true
fi

if $BOT_RUNNING; then
  ok "Bot is running"
else
  warn "Bot may still be starting — check logs if issues arise"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
hr
echo ""
echo -e "${BOLD}${GREEN}  ✓  OpenIntentOS installed!${NC}"
echo ""

if [ -n "${TELEGRAM_BOT_TOKEN:-}" ]; then
  echo -e "  ${BOLD}Next step: open Telegram and send a message to your bot.${NC}"
  echo -e "  ${DIM}  It will respond immediately. Try saying: \"hello\" or \"what can you do?\"${NC}"
else
  echo -e "  ${YELLOW}  You didn't enter a Telegram token.${NC}"
  echo -e "  ${YELLOW}  Edit this file and add your token, then restart:${NC}"
  echo -e "  ${CYAN}    $ENV_FILE${NC}"
  echo -e "  ${CYAN}    $INSTALL_DIR/restart.sh${NC}"
fi

echo ""
echo -e "  ${DIM}── Useful commands ──────────────────────────────────────────${NC}"
echo -e "  ${CYAN}  openintent status${NC}              — check everything is OK"
echo -e "  ${CYAN}  $INSTALL_DIR/status.sh${NC}   — is the bot running?"
echo -e "  ${CYAN}  $INSTALL_DIR/restart.sh${NC}  — restart after config changes"
echo -e "  ${CYAN}  $INSTALL_DIR/uninstall.sh${NC} — remove OpenIntentOS"
echo ""
echo -e "  ${DIM}── Add more integrations later (edit $ENV_FILE) ─────────────${NC}"
echo -e "  ${DIM}  GITHUB_TOKEN        — lets the bot read/write your GitHub repos${NC}"
echo -e "  ${DIM}  DISCORD_BOT_TOKEN   — connect to a Discord server too${NC}"
echo -e "  ${DIM}  (restart the bot after editing the file)${NC}"
echo ""
echo -e "  ${DIM}To update to a newer version: run the install command again.${NC}"
echo -e "  ${DIM}Your data and settings are never deleted on update.${NC}"
echo ""
hr
echo ""
