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

# ── Step 3: Write initial configuration ───────────────────────────────────────
echo -e "\n${BOLD}Step 3/5 · Configuration${NC}\n"

# If keys are pre-set via environment variables, write them immediately.
# This is the fully silent/automated path:
#   TELEGRAM_BOT_TOKEN=xxx OPENAI_API_KEY=sk-xxx curl -fsSL .../install.sh | bash
SILENT_INSTALL=false
HAS_AI_KEY=false
for k in "${OPENAI_API_KEY:-}" "${NVIDIA_API_KEY:-}" "${GOOGLE_API_KEY:-}" \
          "${DEEPSEEK_API_KEY:-}" "${GROQ_API_KEY:-}" "${ANTHROPIC_API_KEY:-}"; do
  [ -n "$k" ] && HAS_AI_KEY=true
done
[ -n "${TELEGRAM_BOT_TOKEN:-}" ] && $HAS_AI_KEY && SILENT_INSTALL=true

if $SILENT_INSTALL; then
  ok "Credentials detected from environment — silent install"
  cat > "$ENV_FILE" <<EOF
# OpenIntentOS Configuration — auto-generated
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"
OPENAI_API_KEY="${OPENAI_API_KEY:-}"
ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"
NVIDIA_API_KEY="${NVIDIA_API_KEY:-}"
GOOGLE_API_KEY="${GOOGLE_API_KEY:-}"
DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY:-}"
GROQ_API_KEY="${GROQ_API_KEY:-}"
GITHUB_TOKEN="${GITHUB_TOKEN:-}"
DISCORD_BOT_TOKEN="${DISCORD_BOT_TOKEN:-}"
EOF
else
  # Write an empty placeholder .env — the setup wizard will fill it in.
  cat > "$ENV_FILE" <<'EOF'
# OpenIntentOS Configuration
# This file is managed by the setup wizard at http://localhost:23517
# You can also edit it manually and run restart.sh

TELEGRAM_BOT_TOKEN=
OPENAI_API_KEY=
ANTHROPIC_API_KEY=
NVIDIA_API_KEY=
GOOGLE_API_KEY=
DEEPSEEK_API_KEY=
GROQ_API_KEY=
GITHUB_TOKEN=
DISCORD_BOT_TOKEN=
EOF
  ok "Configuration file created — wizard will complete setup"
fi

chmod 600 "$ENV_FILE"

# ── Step 4: System service setup ──────────────────────────────────────────────
echo -e "\n${BOLD}Step 4/5 · Installing system service${NC}\n"
info "Setting up auto-start service (starts on login, restarts on crash)..."

WEB_PORT=23517

write_helper_scripts() {
  # status.sh
  cat > "$INSTALL_DIR/status.sh" <<SCRIPT
#!/usr/bin/env bash
LOG_FILE="\$HOME/.openintentos/openintent.log"
PORT=$WEB_PORT
if curl -s --max-time 2 http://localhost:\$PORT/api/status >/dev/null 2>&1; then
  echo "✓ OpenIntentOS is running  →  http://localhost:\$PORT"
else
  echo "✗ OpenIntentOS is not running"
fi
echo "--- last 20 log lines ---"
tail -20 "\$LOG_FILE" 2>/dev/null || echo "(no log yet)"
SCRIPT

  # restart.sh
  cat > "$INSTALL_DIR/restart.sh" <<SCRIPT
#!/usr/bin/env bash
echo "Restarting OpenIntentOS..."
OS="\$(uname -s)"
if [ "\$OS" = "Darwin" ]; then
  launchctl unload ~/Library/LaunchAgents/io.openintentos.plist 2>/dev/null || true
  launchctl load ~/Library/LaunchAgents/io.openintentos.plist
elif [ "\$OS" = "Linux" ] && command -v systemctl &>/dev/null; then
  systemctl --user restart openintentos
else
  PID_FILE="\$HOME/.openintentos/openintent.pid"
  [ -f "\$PID_FILE" ] && kill "\$(cat "\$PID_FILE")" 2>/dev/null || true
  sleep 1
  cd "\$HOME/.openintentos"
  nohup ./openintent serve --port $WEB_PORT >> openintent.log 2>&1 &
  echo \$! > "\$PID_FILE"
  echo "✓ Restarted (PID \$(cat "\$PID_FILE"))"
fi
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

LOG_FILE="$INSTALL_DIR/openintent.log"
PID_FILE="$INSTALL_DIR/openintent.pid"

if [ "$PLATFORM" = "macos" ]; then
  PLIST="$HOME/Library/LaunchAgents/io.openintentos.plist"
  mkdir -p "$HOME/Library/LaunchAgents"
  cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>io.openintentos</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>serve</string>
    <string>--port</string>
    <string>$WEB_PORT</string>
  </array>
  <key>WorkingDirectory</key>
  <string>$INSTALL_DIR</string>
  <key>StandardOutPath</key>
  <string>$LOG_FILE</string>
  <key>StandardErrorPath</key>
  <string>$LOG_FILE</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>5</integer>
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
Description=OpenIntentOS AI Assistant
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
ExecStart=$BIN serve --port $WEB_PORT
Restart=always
RestartSec=5
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
     echo "@reboot cd $INSTALL_DIR && nohup ./openintent serve --port $WEB_PORT >> $LOG_FILE 2>&1 &") \
     | crontab -
    ok "Cron @reboot entry installed"
    # Start now
    cd "$INSTALL_DIR"
    nohup ./openintent serve --port $WEB_PORT >> "$LOG_FILE" 2>&1 &
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

# ── Step 5: Verify & open browser ─────────────────────────────────────────────
echo -e "\n${BOLD}Step 5/5 · Starting up${NC}\n"
info "Waiting for web server to start..."

SETUP_URL="http://localhost:$WEB_PORT"
SERVER_UP=false
for i in $(seq 1 20); do
  if curl -s --max-time 1 "$SETUP_URL/api/setup/status" >/dev/null 2>&1; then
    SERVER_UP=true
    break
  fi
  sleep 1
done

if $SERVER_UP; then
  ok "Server is running at $SETUP_URL"
else
  warn "Server may still be starting — open $SETUP_URL manually"
fi

# Auto-open browser
if ! $SILENT_INSTALL; then
  if [ "$PLATFORM" = "macos" ]; then
    open "$SETUP_URL" 2>/dev/null || true
    ok "Browser opened"
  elif command -v xdg-open &>/dev/null; then
    xdg-open "$SETUP_URL" 2>/dev/null || true
    ok "Browser opened"
  elif command -v sensible-browser &>/dev/null; then
    sensible-browser "$SETUP_URL" 2>/dev/null || true
    ok "Browser opened"
  fi
fi

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
hr
echo ""
echo -e "${BOLD}${GREEN}  ✓  OpenIntentOS is installed!${NC}"
echo ""

if $SILENT_INSTALL; then
  echo -e "  ${BOLD}Silent install complete. Service is running.${NC}"
  echo -e "  ${DIM}  Web UI: $SETUP_URL${NC}"
  echo -e "  ${DIM}  Open Telegram and message your bot to get started.${NC}"
else
  echo -e "  ${BOLD}Your browser should open automatically.${NC}"
  echo -e "  ${DIM}  If it didn't, open this URL in your browser:${NC}"
  echo ""
  echo -e "  ${CYAN}  $SETUP_URL${NC}"
  echo ""
  echo -e "  ${DIM}The setup wizard will guide you through connecting"
  echo -e "  your AI provider and Telegram bot. Takes 2 minutes.${NC}"
fi

echo ""
echo -e "  ${DIM}── Useful commands ──────────────────────────────────────────${NC}"
echo -e "  ${CYAN}  $INSTALL_DIR/status.sh${NC}    — is the service running?"
echo -e "  ${CYAN}  $INSTALL_DIR/restart.sh${NC}   — restart after config changes"
echo -e "  ${CYAN}  $INSTALL_DIR/uninstall.sh${NC} — remove OpenIntentOS"
echo ""
echo -e "  ${DIM}To update: run the install command again. Your data is safe.${NC}"
echo ""
hr
echo ""
