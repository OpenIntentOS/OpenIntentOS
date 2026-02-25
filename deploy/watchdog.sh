#!/usr/bin/env bash
# =============================================================================
# OpenIntentOS Watchdog
# =============================================================================
# Monitors the bot process and restarts it automatically on crash/exit.
# Works on both Linux and macOS. For production Linux, prefer systemd.
#
# Usage:
#   ./deploy/watchdog.sh                      # Run from project root
#   WATCHDOG_CMD="openintent bot" ./deploy/watchdog.sh  # Custom command
#   LOG_DIR=/var/log/openintent ./deploy/watchdog.sh    # Custom log dir
#
# Environment variables:
#   WATCHDOG_CMD        - Command to run (default: auto-detect)
#   RESTART_DELAY       - Seconds to wait before restart (default: 3)
#   MAX_RAPID_RESTARTS  - Max restarts within RAPID_WINDOW before back-off (default: 5)
#   RAPID_WINDOW        - Window in seconds for rapid restart detection (default: 60)
#   BACKOFF_DELAY       - Seconds to wait after too many rapid restarts (default: 30)
#   LOG_DIR             - Directory for watchdog logs (default: ./data/logs)
# =============================================================================

set -uo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
RESTART_DELAY="${RESTART_DELAY:-3}"
MAX_RAPID_RESTARTS="${MAX_RAPID_RESTARTS:-5}"
RAPID_WINDOW="${RAPID_WINDOW:-60}"
BACKOFF_DELAY="${BACKOFF_DELAY:-30}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${SCRIPT_DIR}/.."
LOG_DIR="${LOG_DIR:-${PROJECT_ROOT}/data/logs}"

mkdir -p "${LOG_DIR}"

# Log file for watchdog itself
WATCHDOG_LOG="${LOG_DIR}/watchdog.log"

# ---------------------------------------------------------------------------
# Auto-detect command
# ---------------------------------------------------------------------------
if [ -z "${WATCHDOG_CMD:-}" ]; then
    # Try release binary first, then debug, then cargo
    if [ -x "${PROJECT_ROOT}/target/release/openintent" ]; then
        WATCHDOG_CMD="${PROJECT_ROOT}/target/release/openintent bot"
    elif [ -x "${PROJECT_ROOT}/target/debug/openintent" ]; then
        WATCHDOG_CMD="${PROJECT_ROOT}/target/debug/openintent bot"
    elif command -v cargo &>/dev/null; then
        WATCHDOG_CMD="cargo run --release --bin openintent -- bot"
    else
        echo "[WATCHDOG] ERROR: Cannot find openintent binary or cargo. Set WATCHDOG_CMD." >&2
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log() {
    local ts
    ts="$(date '+%Y-%m-%d %H:%M:%S')"
    echo "[${ts}] [WATCHDOG] $*" | tee -a "${WATCHDOG_LOG}"
}

# Track restart timestamps for rapid-restart detection
declare -a restart_times=()

check_rapid_restarts() {
    local now
    now="$(date +%s)"

    # Remove timestamps outside the window
    local filtered=()
    for t in "${restart_times[@]}"; do
        if (( now - t < RAPID_WINDOW )); then
            filtered+=("$t")
        fi
    done
    restart_times=("${filtered[@]}")

    # Add current restart
    restart_times+=("$now")

    if (( ${#restart_times[@]} >= MAX_RAPID_RESTARTS )); then
        return 0  # Too many rapid restarts
    fi
    return 1  # OK
}

# ---------------------------------------------------------------------------
# Signal handling
# ---------------------------------------------------------------------------
BOT_PID=""
SHUTDOWN=0

cleanup() {
    SHUTDOWN=1
    if [ -n "$BOT_PID" ] && kill -0 "$BOT_PID" 2>/dev/null; then
        log "Shutting down bot (PID ${BOT_PID})..."
        kill -TERM "$BOT_PID" 2>/dev/null
        wait "$BOT_PID" 2>/dev/null || true
    fi
    log "Watchdog stopped."
    exit 0
}

trap cleanup SIGINT SIGTERM SIGHUP

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
log "Starting watchdog"
log "Command: ${WATCHDOG_CMD}"
log "Restart delay: ${RESTART_DELAY}s"
log "Rapid restart limit: ${MAX_RAPID_RESTARTS} within ${RAPID_WINDOW}s"
log "Working directory: ${PROJECT_ROOT}"

cd "${PROJECT_ROOT}"

while true; do
    if (( SHUTDOWN == 1 )); then
        break
    fi

    log "Starting bot..."
    ${WATCHDOG_CMD} &
    BOT_PID=$!
    log "Bot started (PID ${BOT_PID})"

    # Wait for the process to exit
    wait "$BOT_PID"
    EXIT_CODE=$?
    BOT_PID=""

    if (( SHUTDOWN == 1 )); then
        break
    fi

    log "Bot exited with code ${EXIT_CODE}"

    # Check for rapid restarts (crash loop detection)
    if check_rapid_restarts; then
        log "WARNING: ${MAX_RAPID_RESTARTS} restarts within ${RAPID_WINDOW}s -- possible crash loop"
        log "Backing off for ${BACKOFF_DELAY}s..."
        sleep "${BACKOFF_DELAY}"
        # Reset the counter after back-off
        restart_times=()
    fi

    log "Restarting in ${RESTART_DELAY}s..."
    sleep "${RESTART_DELAY}"
done
