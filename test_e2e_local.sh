#!/usr/bin/env bash
# OpenIntentOS — Local End-to-End Test
# Runs the real binary to simulate a complete user journey.
set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
PASS=0; FAIL=0
REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="$REPO_DIR/target/release/openintent"
TEST_PORT=23599
BASE="http://127.0.0.1:$TEST_PORT"
SERVER_PID=""
TMPDIR_1=""
TMPDIR_2=""

ok()   { echo -e "  ${GREEN}✓${NC}  $*"; ((PASS++)) || true; }
fail() { echo -e "  ${RED}✗${NC}  $*"; ((FAIL++)) || true; }
info() { echo -e "  ${CYAN}→${NC}  $*"; }
hr()   { echo -e "${CYAN}────────────────────────────────────────────────${NC}"; }

stop_server() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=""
}

cleanup() {
  stop_server
  [ -n "$TMPDIR_1" ] && rm -rf "$TMPDIR_1" || true
  [ -n "$TMPDIR_2" ] && rm -rf "$TMPDIR_2" || true
}
trap cleanup EXIT

wait_for_server() {
  local url="$1"
  for i in $(seq 1 20); do
    if curl -sf --max-time 1 "$url" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  return 1
}

echo ""
echo -e "${BOLD}${CYAN}  OpenIntentOS — Real Machine E2E Test${NC}"
hr

# ═══════════════════════════════════════════════════════════
# 0. Binary check
# ═══════════════════════════════════════════════════════════
echo -e "\n${BOLD}[0] Binary${NC}"
if [ -x "$BIN" ]; then
  VERSION=$("$BIN" --version 2>/dev/null || echo "unknown")
  ok "Binary: $VERSION"
else
  fail "Binary not found — run: cargo build --release --bin openintent"
  exit 1
fi

# ═══════════════════════════════════════════════════════════
# 1. New-user scenario: empty env → wizard
# ═══════════════════════════════════════════════════════════
echo -e "\n${BOLD}[1] New-user scenario (no API key)${NC}"

TMPDIR_1=$(mktemp -d)
# Server reads .env from its CWD; write empty one
echo "" > "$TMPDIR_1/.env"
mkdir -p "$TMPDIR_1/data" "$TMPDIR_1/config"
cp -r "$REPO_DIR/config/" "$TMPDIR_1/config/" 2>/dev/null || true

cd "$TMPDIR_1"
"$BIN" serve --port $TEST_PORT > /tmp/openintent_e2e.log 2>&1 &
SERVER_PID=$!
cd "$REPO_DIR"

if wait_for_server "$BASE/api/setup/status"; then
  info "Server started (PID=$SERVER_PID)"

  # /api/setup/status → configured:false
  STATUS=$(curl -sf "$BASE/api/setup/status" 2>/dev/null || echo "{}")
  if echo "$STATUS" | grep -q '"configured":false'; then
    ok "/api/setup/status → configured=false ✓"
  else
    fail "/api/setup/status unexpected: $STATUS"
  fi

  # /setup → HTML wizard (save to file; response is large)
  curl -sf "$BASE/setup" -o /tmp/oi_setup_test.html 2>/dev/null || true
  if grep -q "OpenIntentOS" /tmp/oi_setup_test.html 2>/dev/null; then
    ok "/setup → HTML wizard loaded ($(wc -c < /tmp/oi_setup_test.html | tr -d ' ') bytes)"
  else
    fail "/setup did not return expected HTML"
  fi

  # Chinese provider buttons present in wizard
  for p in siliconflow moonshot zhipu tongyi; do
    if grep -q "$p" /tmp/oi_setup_test.html 2>/dev/null; then
      ok "  wizard has '$p' button"
    else
      fail "  wizard missing '$p' button"
    fi
  done

  # POST /api/setup/save with SiliconFlow
  R=$(curl -sf -X POST "$BASE/api/setup/save" \
    -H "Content-Type: application/json" \
    -d '{"provider":"siliconflow","api_key":"sk-sf-test","telegram_token":""}' \
    2>/dev/null || echo '{"ok":false}')
  echo "$R" | grep -q '"ok":true' \
    && ok "POST /api/setup/save siliconflow → ok" \
    || fail "POST /api/setup/save siliconflow failed: $R"

  # POST with Zhipu (permanently free)
  R=$(curl -sf -X POST "$BASE/api/setup/save" \
    -H "Content-Type: application/json" \
    -d '{"provider":"zhipu","api_key":"zhipu-free","telegram_token":""}' \
    2>/dev/null || echo '{"ok":false}')
  echo "$R" | grep -q '"ok":true' \
    && ok "POST /api/setup/save zhipu → ok" \
    || fail "POST /api/setup/save zhipu failed: $R"

else
  fail "Server failed to start — log:"
  tail -10 /tmp/openintent_e2e.log
fi

stop_server

# ═══════════════════════════════════════════════════════════
# 2. Real-key scenario
# ═══════════════════════════════════════════════════════════
echo -e "\n${BOLD}[2] Real-key scenario${NC}"

if [ -f "$REPO_DIR/.env" ]; then
  TMPDIR_2=$(mktemp -d)
  cp "$REPO_DIR/.env" "$TMPDIR_2/.env"
  mkdir -p "$TMPDIR_2/data" "$TMPDIR_2/config"
  cp -r "$REPO_DIR/config/" "$TMPDIR_2/config/" 2>/dev/null || true

  cd "$TMPDIR_2"
  "$BIN" serve --port $TEST_PORT > /tmp/openintent_e2e2.log 2>&1 &
  SERVER_PID=$!
  cd "$REPO_DIR"

  if wait_for_server "$BASE/api/setup/status" || wait_for_server "$BASE/api/status"; then
    info "Server started with real keys (PID=$SERVER_PID)"

    STATUS2=$(curl -sf "$BASE/api/setup/status" 2>/dev/null || echo "{}")
    if echo "$STATUS2" | grep -q '"configured":true'; then
      ok "/api/setup/status → configured=true"
    else
      info "/api/setup/status: $STATUS2"
    fi

    MAIN=$(curl -sf --max-time 3 "$BASE/api/status" 2>/dev/null || echo "")
    if [ -n "$MAIN" ]; then
      MODEL=$(echo "$MAIN" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("model","?"))' 2>/dev/null || echo "?")
      ok "/api/status → active model: $MODEL"
    else
      info "/api/status not available at this stage"
    fi
  else
    fail "Server with real .env failed to start"
    tail -5 /tmp/openintent_e2e2.log
  fi

  stop_server
else
  info "No .env — skipping"
fi

# ═══════════════════════════════════════════════════════════
# 3. Telegram bot check
# ═══════════════════════════════════════════════════════════
echo -e "\n${BOLD}[3] Telegram bot${NC}"

TG_TOKEN=$(grep "^TELEGRAM_BOT_TOKEN=" "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d ' \r\n' || echo "")
if [ -n "$TG_TOKEN" ]; then
  TG=$(curl -sf "https://api.telegram.org/bot${TG_TOKEN}/getMe" --max-time 10 2>/dev/null || echo "")
  if echo "$TG" | grep -q '"ok":true'; then
    BOT=$(echo "$TG" | python3 -c 'import sys,json; d=json.load(sys.stdin); print("@"+d["result"]["username"])' 2>/dev/null || echo "bot")
    ok "Telegram $BOT is reachable"
  else
    fail "Telegram getMe failed"
  fi
else
  info "No TELEGRAM_BOT_TOKEN — skipping"
fi

# ═══════════════════════════════════════════════════════════
# 4. LLM provider live ping
# ═══════════════════════════════════════════════════════════
echo -e "\n${BOLD}[4] LLM provider live test${NC}"

check_openai_compat() {
  local name="$1" key="$2" url="$3" model="$4"
  [ -z "$key" ] && return
  local out
  out=$(curl -sf -X POST "$url/chat/completions" \
    -H "Authorization: Bearer $key" \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"$model\",\"messages\":[{\"role\":\"user\",\"content\":\"reply with one word: ok\"}],\"max_tokens\":10}" \
    --max-time 20 2>/dev/null \
    | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d["choices"][0]["message"]["content"].strip())' 2>/dev/null || echo "")
  if [ -n "$out" ]; then
    ok "$name → \"$out\""
  else
    # API may be rate-limited or key may have expired; warn but don't hard-fail
    info "$name → no response (rate limit or key issue)"
  fi
}

DS_KEY=$(grep "^DEEPSEEK_API_KEY=" "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d ' \r\n' || echo "")
check_openai_compat "DeepSeek" "$DS_KEY" "https://api.deepseek.com/v1" "deepseek-chat"

OAI_KEY=$(grep "^OPENAI_API_KEY=" "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d ' \r\n' || echo "")
check_openai_compat "OpenAI" "$OAI_KEY" "https://api.openai.com/v1" "gpt-4o-mini"

SF_KEY=$(grep "^SILICONFLOW_API_KEY=" "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d ' \r\n' || echo "")
check_openai_compat "SiliconFlow" "$SF_KEY" "https://api.siliconflow.cn/v1" "deepseek-ai/DeepSeek-V3"

[ -z "$DS_KEY" ] && [ -z "$OAI_KEY" ] && [ -z "$SF_KEY" ] && info "No LLM keys found in .env"

# ═══════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════
echo ""
hr
TOTAL=$((PASS + FAIL))
echo -e "\n  ${BOLD}${TOTAL} checks:  ${GREEN}${PASS} passed${NC}  ${RED}${FAIL} failed${NC}\n"
hr
echo ""
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
