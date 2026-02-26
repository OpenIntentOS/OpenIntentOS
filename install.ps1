# OpenIntentOS Windows Installer
# Usage: irm https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.ps1 | iex
#
# Supported: Windows 10/11, Windows Server 2019+  (x64 and ARM64)
# Requires:  PowerShell 5.1+ or PowerShell 7+

#Requires -Version 5.1
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$REPO       = "OpenIntentOS/OpenIntentOS"
$INSTALL_DIR = "$env:APPDATA\openintentos"
$BIN         = "$INSTALL_DIR\openintent-cli.exe"
$ENV_FILE    = "$INSTALL_DIR\.env"
$CONFIG_DIR  = "$INSTALL_DIR\config"
$DATA_DIR    = "$INSTALL_DIR\data"
$LOG_FILE    = "$INSTALL_DIR\bot.log"

# ── Colors ────────────────────────────────────────────────────────────────────
function Write-Ok($msg)   { Write-Host "  " -NoNewline; Write-Host "✓" -ForegroundColor Green -NoNewline; Write-Host "  $msg" }
function Write-Info($msg) { Write-Host "  " -NoNewline; Write-Host "→" -ForegroundColor Cyan  -NoNewline; Write-Host "  $msg" }
function Write-Warn($msg) { Write-Host "  " -NoNewline; Write-Host "!" -ForegroundColor Yellow -NoNewline; Write-Host "  $msg" }
function Write-Hr        { Write-Host ("─" * 56) -ForegroundColor DarkGray }
function Write-Step($n, $total, $msg) {
  Write-Host ""
  Write-Host "Step $n/$total · $msg" -ForegroundColor White
  Write-Host ""
}

# ── Banner ────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "   ╔═══════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "   ║         OpenIntentOS  Installer               ║" -ForegroundColor Cyan
Write-Host "   ║     Intent-Driven AI OS — Full Rust           ║" -ForegroundColor Cyan
Write-Host "   ╚═══════════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""
Write-Hr

# ── Step 1: Detect architecture ───────────────────────────────────────────────
Write-Step 1 5 "Detecting your system"

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
$target = switch ($arch) {
  "X64"   { "x86_64-pc-windows-msvc" }
  "Arm64" { "aarch64-pc-windows-msvc" }
  default { throw "Unsupported architecture: $arch" }
}

$winVer = [System.Environment]::OSVersion.Version
Write-Ok "Windows $($winVer.Major).$($winVer.Minor) ($arch) → $target"

# ── Step 2: Download binary ───────────────────────────────────────────────────
Write-Step 2 5 "Downloading OpenIntentOS"

New-Item -ItemType Directory -Force -Path $INSTALL_DIR, $CONFIG_DIR, $DATA_DIR | Out-Null

# Get latest release
try {
  $release = Invoke-RestMethod "https://api.github.com/repos/$REPO/releases/latest"
  $tag     = $release.tag_name
  $zipUrl  = "https://github.com/$REPO/releases/download/$tag/openintent-cli-$target.zip"

  Write-Info "Downloading openintent-cli $tag for $target ..."
  $tmp = "$env:TEMP\openintent.zip"
  Invoke-WebRequest -Uri $zipUrl -OutFile $tmp -UseBasicParsing
  Expand-Archive -Path $tmp -DestinationPath $INSTALL_DIR -Force
  Remove-Item $tmp

  $sizeMB = [math]::Round((Get-Item $BIN).Length / 1MB, 1)
  Write-Ok "Downloaded ${sizeMB}MB binary ($tag)"
}
catch {
  # Fall back: build from source via WSL or winget cargo
  Write-Warn "No prebuilt binary found — attempting to build from source..."

  $hasWsl = (Get-Command wsl -ErrorAction SilentlyContinue) -ne $null
  $hasCargo = (Get-Command cargo -ErrorAction SilentlyContinue) -ne $null

  if ($hasWsl) {
    Write-Info "Building via WSL (this takes a few minutes)..."
    wsl bash -c "curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | bash"
    Write-Ok "Installed via WSL. To run, open WSL and use: ~/.openintentos/status.sh"
    Write-Host ""
    Write-Hr
    exit 0
  }
  elseif ($hasCargo) {
    Write-Info "Building with Cargo (this takes a few minutes)..."
    $tmpRepo = "$env:TEMP\openintentos-build"
    git clone --depth 1 "https://github.com/$REPO.git" $tmpRepo
    Push-Location $tmpRepo
    cargo build --release --bin openintent-cli
    Copy-Item "target\release\openintent-cli.exe" $BIN
    Pop-Location
    Remove-Item -Recurse -Force $tmpRepo
    Write-Ok "Build complete"
  }
  else {
    Write-Host ""
    Write-Host "  No prebuilt binary available and no build tools found." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  Options:" -ForegroundColor White
    Write-Host "    1. Install via WSL (recommended for Windows):" -ForegroundColor Gray
    Write-Host "       https://aka.ms/wslinstall  →  then run:" -ForegroundColor Gray
    Write-Host "       curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | bash" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "    2. Install Rust then rerun this script:" -ForegroundColor Gray
    Write-Host "       https://win.rustup.rs" -ForegroundColor Cyan
    Write-Host ""
    exit 1
  }
}

# ── Download config ───────────────────────────────────────────────────────────
try {
  $configUrl = "https://raw.githubusercontent.com/$REPO/main/config/default.toml"
  Invoke-WebRequest -Uri $configUrl -OutFile "$CONFIG_DIR\default.toml" -UseBasicParsing
  Write-Ok "Configuration downloaded"
} catch {
  Write-Warn "Could not download config — will use built-in defaults"
}

# ── Step 3: API keys ──────────────────────────────────────────────────────────
Write-Step 3 5 "Connect your AI providers"

Write-Host "  No IT skills needed — just paste the keys when asked." -ForegroundColor DarkGray
Write-Host "  Keys are saved locally to: $ENV_FILE" -ForegroundColor DarkGray
Write-Host "  They are never sent anywhere except the AI service APIs." -ForegroundColor DarkGray
Write-Host ""

function Read-Key($label, $url, $required = $false) {
  Write-Host "  " -NoNewline
  Write-Host $label -ForegroundColor White
  if ($url) { Write-Host "  Get it at: $url" -ForegroundColor DarkGray }
  if ($required) { Write-Host "  (required)" -ForegroundColor Yellow }
  else            { Write-Host "  (optional — press Enter to skip)" -ForegroundColor DarkGray }
  $val = Read-Host "  Enter"
  Write-Host ""
  if ($val) { Write-Ok "$label saved" }
  return $val
}

Write-Host "  📱 Telegram Bot" -ForegroundColor Cyan
Write-Host ""
Write-Host "  Don't have a bot yet? Here's how:" -ForegroundColor DarkGray
Write-Host "    1. Open Telegram, search for @BotFather" -ForegroundColor DarkGray
Write-Host "    2. Send: /newbot" -ForegroundColor DarkGray
Write-Host "    3. Follow the steps, copy the token it gives you" -ForegroundColor DarkGray
Write-Host ""
$TELEGRAM = Read-Key "Telegram Bot Token" "https://t.me/BotFather" $true

Write-Host "  🧠 AI Provider (enter at least one)" -ForegroundColor Cyan
Write-Host ""
$OPENAI   = Read-Key "OpenAI API Key"              "https://platform.openai.com/api-keys"
$NVIDIA   = Read-Key "NVIDIA NIM API Key (free)"   "https://build.nvidia.com"
$GOOGLE   = Read-Key "Google Gemini Key"            "https://aistudio.google.com/apikey"
$DEEPSEEK = Read-Key "DeepSeek API Key"             "https://platform.deepseek.com"
$GROQ     = Read-Key "Groq API Key"                 "https://console.groq.com/keys"

Write-Host "  🔗 Optional Integrations" -ForegroundColor Cyan
Write-Host ""
$GITHUB   = Read-Key "GitHub Token (enables self-repair)" "https://github.com/settings/tokens"
$DISCORD  = Read-Key "Discord Bot Token"            "https://discord.com/developers/applications"

# Write .env
@"
# OpenIntentOS Configuration — edit then restart the bot
# Run: $INSTALL_DIR\restart.bat

TELEGRAM_BOT_TOKEN=$TELEGRAM
OPENAI_API_KEY=$OPENAI
NVIDIA_API_KEY=$NVIDIA
GOOGLE_API_KEY=$GOOGLE
DEEPSEEK_API_KEY=$DEEPSEEK
GROQ_API_KEY=$GROQ
GITHUB_TOKEN=$GITHUB
DISCORD_BOT_TOKEN=$DISCORD
"@ | Set-Content $ENV_FILE -Encoding UTF8

Write-Ok "Credentials saved to $ENV_FILE"

# ── Step 4: Task Scheduler service ────────────────────────────────────────────
Write-Step 4 5 "Installing Windows service (auto-start)"

# Load env vars for this session
Get-Content $ENV_FILE | Where-Object { $_ -match "^[A-Z_]+=.+" } | ForEach-Object {
  $parts = $_ -split "=", 2
  [System.Environment]::SetEnvironmentVariable($parts[0], $parts[1], "Process")
}

# Build the action command that sources .env then runs the bot
$startScript = "$INSTALL_DIR\start.bat"
@"
@echo off
for /f "tokens=1,* delims==" %%a in ($ENV_FILE) do (
  if not "%%a"=="" if not "%%a:~0,1%"=="#" set "%%a=%%b"
)
cd /d "$INSTALL_DIR"
"$BIN" bot >> "$LOG_FILE" 2>&1
"@ | Set-Content $startScript -Encoding ASCII

$restartScript = "$INSTALL_DIR\restart.bat"
@"
@echo off
echo Restarting OpenIntentOS...
taskkill /f /im openintent-cli.exe 2>nul
timeout /t 2 /nobreak >nul
start "" /b "$startScript"
echo Done. Check logs: $LOG_FILE
"@ | Set-Content $restartScript -Encoding ASCII

$statusScript = "$INSTALL_DIR\status.bat"
@"
@echo off
tasklist /fi "imagename eq openintent-cli.exe" | find "openintent-cli.exe" >nul
if %errorlevel%==0 (
  echo [OK] OpenIntentOS is running
) else (
  echo [X]  OpenIntentOS is NOT running
)
echo.
echo --- Last 30 log lines ---
powershell -command "Get-Content '$LOG_FILE' -Tail 30 -ErrorAction SilentlyContinue"
"@ | Set-Content $statusScript -Encoding ASCII

$uninstallScript = "$INSTALL_DIR\uninstall.bat"
@"
@echo off
echo Uninstalling OpenIntentOS...
taskkill /f /im openintent-cli.exe 2>nul
schtasks /delete /tn "OpenIntentOS" /f 2>nul
rmdir /s /q "$INSTALL_DIR"
echo Done. OpenIntentOS has been removed.
"@ | Set-Content $uninstallScript -Encoding ASCII

# Register Task Scheduler task
try {
  $action  = New-ScheduledTaskAction -Execute $startScript
  $trigger = New-ScheduledTaskTrigger -AtLogOn
  $settings = New-ScheduledTaskSettingsSet -RestartCount 99 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([System.TimeSpan]::Zero)
  $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Limited

  Unregister-ScheduledTask -TaskName "OpenIntentOS" -Confirm:$false -ErrorAction SilentlyContinue
  Register-ScheduledTask -TaskName "OpenIntentOS" -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force | Out-Null

  Write-Ok "Windows Task Scheduler task registered (auto-starts on login)"
} catch {
  Write-Warn "Could not register Task Scheduler task: $_"
  Write-Warn "You can start the bot manually: $startScript"
}

# ── Step 5: Start and verify ──────────────────────────────────────────────────
Write-Step 5 5 "Starting OpenIntentOS"

Start-Process -FilePath $startScript -WindowStyle Hidden
Start-Sleep -Seconds 4

$proc = Get-Process "openintent-cli" -ErrorAction SilentlyContinue
if ($proc) {
  Write-Ok "Bot is running (PID $($proc.Id))"
} else {
  Write-Warn "Bot may still be starting — check logs if issues arise"
}

# ── Done ──────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Hr
Write-Host ""
Write-Host "  ✓  OpenIntentOS is installed and running!" -ForegroundColor Green
Write-Host ""
if ($TELEGRAM) {
  Write-Host "  Open Telegram and message your bot to get started." -ForegroundColor White
} else {
  Write-Host "  Add your Telegram token to: $ENV_FILE" -ForegroundColor Yellow
  Write-Host "  Then run: $INSTALL_DIR\restart.bat" -ForegroundColor Yellow
}
Write-Host ""
Write-Host "  Useful commands:" -ForegroundColor DarkGray
Write-Host "    $INSTALL_DIR\status.bat    — check if bot is running" -ForegroundColor Cyan
Write-Host "    $INSTALL_DIR\restart.bat   — apply config changes" -ForegroundColor Cyan
Write-Host "    $INSTALL_DIR\uninstall.bat — remove everything" -ForegroundColor Cyan
Write-Host "    notepad $LOG_FILE          — view logs" -ForegroundColor Cyan
Write-Host ""
Write-Host "  To update: run the install command again." -ForegroundColor DarkGray
Write-Host ""
Write-Hr
Write-Host ""
