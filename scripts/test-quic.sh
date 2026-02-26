#!/usr/bin/env bash
# test-quic.sh — End-to-end HTTP/3 / QUIC test harness for GBP Router
#
# What this script does:
#   1. Builds router + h3_client with `--features quic`
#   2. Starts the router in background using examples/router-quic-test.yaml
#   3. Waits for the TCP health endpoint to become ready
#   4. Runs a battery of curl tests against the TCP/HTTP stack (verifying Alt-Svc header)
#   5. Runs a battery of HTTP/3 tests via h3_client (the native Rust QUIC client)
#   6. Prints a pass/fail summary
#   7. Terminates the router
#
# Usage:
#   ./scripts/test-quic.sh
#
# Requirements:
#   - Rust toolchain
#   - curl (any version, used for TCP/HTTP2 tests only)
#   - No external HTTP/3-capable curl needed — we use h3_client

# NOTE: intentionally NO set -e so individual test failures don't abort the whole run
set -uo pipefail

# ── Colors ────────────────────────────────────────────────────────────────────
RED=$'\033[0;31m'
GRN=$'\033[0;32m'
YLW=$'\033[0;33m'
BLU=$'\033[0;34m'
CYN=$'\033[0;36m'
BOLD=$'\033[1m'
RST=$'\033[0m'

PASS=0
FAIL=0
ROUTER_PID=""

cleanup() {
    if [[ -n "$ROUTER_PID" ]]; then
        printf "\n%s🛑  Stopping router (PID %s)...%s\n" "$YLW" "$ROUTER_PID" "$RST"
        kill "$ROUTER_PID" 2>/dev/null || true
        wait "$ROUTER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

pass()    { printf "%s  ✅  PASS%s  %s\n" "$GRN" "$RST" "$1"; ((PASS++)) || true; }
fail()    { printf "%s  ❌  FAIL%s  %s\n" "$RED" "$RST" "$1"; ((FAIL++)) || true; }
info()    { printf "%s  ℹ️   %s  %s\n"    "$BLU" "$RST" "$1"; }
section() { printf "\n%s%s── %s ────────────────────────────────────────────────────%s\n" "$BOLD" "$CYN" "$1" "$RST"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="$ROOT/examples/router-quic-test.yaml"
PORT=4000
BASE_URL="http://127.0.0.1:$PORT"
H3_BASE="https://127.0.0.1:$PORT"

printf "%s╔═══════════════════════════════════════════════════════════════╗%s\n" "$BOLD" "$RST"
printf "%s║          GBP Router — HTTP/3 QUIC Test Suite                 ║%s\n" "$BOLD" "$RST"
printf "%s╚═══════════════════════════════════════════════════════════════╝%s\n" "$BOLD" "$RST"
echo

# ─── Step 1: Build ────────────────────────────────────────────────────────────
section "Step 1: Build (router + h3_client)"
printf "%s  Building with --features quic (this may take a minute)...%s\n" "$YLW" "$RST"

cd "$ROOT"
BUILD_LOG=$(cargo build --bin router --bin h3_client --features quic 2>&1)
BUILD_EXIT=$?
echo "$BUILD_LOG" | tail -5
if [[ $BUILD_EXIT -eq 0 ]]; then
    pass "cargo build --features quic"
else
    fail "cargo build --features quic"
    printf "%s  Build failed. Aborting tests.%s\n" "$RED" "$RST"
    echo "$BUILD_LOG" | tail -20
    exit 1
fi

# ─── Step 2: Start Router ─────────────────────────────────────────────────────
section "Step 2: Start Router"
info "Config: $CONFIG"

# Run the pre-built binary directly (avoids cargo re-compile overhead)
TARGET_DIR="$ROOT/target/debug"
"$TARGET_DIR/router" "$CONFIG" >"$ROOT/target/gbp-quic-test.log" 2>&1 &
ROUTER_PID=$!
info "Router PID: $ROUTER_PID  (logs → $ROOT/target/gbp-quic-test.log)"

# Wait for TCP health endpoint (up to 20s)
printf "%s  Waiting for router to become ready...%s\n" "$YLW" "$RST"
READY=0
for i in $(seq 1 40); do
    if curl -sf "$BASE_URL/health" >/dev/null 2>&1; then
        READY=1
        break
    fi
    # Check process is still alive
    if ! kill -0 "$ROUTER_PID" 2>/dev/null; then
        printf "%s  Router process died!%s\n" "$RED" "$RST"
        printf "%s  Log output:%s\n" "$RED" "$RST"
        cat "$ROOT/target/gbp-quic-test.log" | tail -30
        exit 1
    fi
    sleep 0.5
done

if [[ $READY -eq 0 ]]; then
    fail "Router did not start within 20 seconds"
    printf "%s  Router log tail:%s\n" "$RED" "$RST"
    tail -30 "$ROOT/target/gbp-quic-test.log" || true
    exit 1
fi
pass "Router started and TCP health endpoint is responsive"

# Allow QUIC endpoint a moment to fully bind
sleep 0.5

# ─── Step 3: TCP / HTTP curl tests ────────────────────────────────────────────
section "Step 3: TCP / HTTP Tests (curl)"

# 3a. Health endpoint
info "GET $BASE_URL/health"
RESP=$(curl -sf "$BASE_URL/health" 2>&1 || echo "CURL_FAILED")
if echo "$RESP" | grep -q '"status"'; then
    pass "GET /health returns JSON with 'status' field"
    printf "  Response: %s\n\n" "$(echo "$RESP" | head -c 200)"
else
    fail "GET /health did not return expected JSON  (got: ${RESP:0:100})"
fi

# 3b. Alt-Svc header is present (proves QUIC is enabled)
info "Checking Alt-Svc header advertisement"
HEADERS=$(curl -sI "$BASE_URL/health" 2>&1 || echo "")
if echo "$HEADERS" | grep -qi "alt-svc"; then
    ALT_SVC=$(echo "$HEADERS" | grep -i "alt-svc" | tr -d '\r')
    pass "Alt-Svc header present: $ALT_SVC"
else
    fail "Alt-Svc header NOT found — check quic.enabled in router-quic-test.yaml"
fi

# 3c. POST /graphql
info "POST /graphql over TCP/HTTP"
GQL_RESP=$(curl -s -X POST \
    -H "Content-Type: application/json" \
    -d '{"query":"{ __typename }"}' \
    "$BASE_URL/graphql" 2>&1 || echo "CURL_FAILED")
if [[ -n "$GQL_RESP" ]] && ! echo "$GQL_RESP" | grep -q "CURL_FAILED"; then
    pass "POST /graphql returned a response over TCP/HTTP"
    printf "  Response: %s\n\n" "$(echo "$GQL_RESP" | head -c 200)"
else
    fail "POST /graphql failed over TCP/HTTP"
fi

# 3d. Security headers
info "Checking security headers"
SEC_HEADERS=$(curl -sI "$BASE_URL/health" 2>&1 || echo "")
for HEADER in "x-content-type-options" "x-frame-options" "strict-transport-security"; do
    if echo "$SEC_HEADERS" | grep -qi "$HEADER"; then
        pass "Security header present: $HEADER"
    else
        fail "Security header missing: $HEADER"
    fi
done

# ─── Step 4: HTTP/3 / QUIC tests via h3_client ───────────────────────────────
section "Step 4: HTTP/3 QUIC Tests (h3_client — native Rust QUIC client)"
info "(macOS system curl lacks HTTP/3; h3_client uses quinn+h3 — same stack as server)"

H3_BIN="$TARGET_DIR/h3_client"

# 4a. Health check over HTTP/3
info "GET /health over HTTP/3"
H3_HEALTH=$("$H3_BIN" --url "$H3_BASE/health" 2>&1 || true)
echo "$H3_HEALTH"
if echo "$H3_HEALTH" | grep -qE "✅|HTTP/3 200"; then
    pass "GET /health over HTTP/3 succeeded"
else
    fail "GET /health over HTTP/3 failed"
fi

# 4b. HTTP status 200
info "Checking HTTP/3 response status for /health"
if echo "$H3_HEALTH" | grep -qE "HTTP/3 (200|2[0-9][0-9])"; then
    pass "HTTP/3 /health returned 2xx status"
else
    fail "HTTP/3 /health did not return 2xx  (see output above)"
fi

# 4c. QUIC transport marker in JSON body
info "Checking HTTP/3 /health body for QUIC transport marker"
if echo "$H3_HEALTH" | grep -qi '"transport"'; then
    pass "HTTP/3 /health body contains 'transport' field"
else
    fail "HTTP/3 /health body missing 'transport' field"
fi

# 4d. POST /graphql over HTTP/3
info "POST /graphql over HTTP/3"
H3_GQL=$("$H3_BIN" \
    --url "$H3_BASE/graphql" \
    --method POST \
    --body '{"query":"{ __typename }"}' 2>&1 || true)
echo "$H3_GQL"
if echo "$H3_GQL" | grep -qE "✅|HTTP/3 200"; then
    pass "POST /graphql over HTTP/3 succeeded"
else
    fail "POST /graphql over HTTP/3 failed  (see output above)"
fi

# 4e. Latency benchmark: 10 sequential requests over HTTP/3
info "HTTP/3 latency benchmark: 10 sequential GET /health requests"
H3_PERF=$("$H3_BIN" --url "$H3_BASE/health" --repeat 10 2>&1 || true)
echo "$H3_PERF"
if echo "$H3_PERF" | grep -q "Avg time"; then
    AVG=$(echo "$H3_PERF" | grep "Avg time" | awk '{print $NF}')
    pass "HTTP/3 latency benchmark complete  (avg: $AVG)"
else
    fail "HTTP/3 latency benchmark did not complete"
fi

# 4f. Verbose headers dump (informational, not a pass/fail)
info "HTTP/3 verbose request/response headers (informational)"
"$H3_BIN" --url "$H3_BASE/health" --verbose 2>&1 | head -50 || true

# ─── Step 5: Verify router logs ───────────────────────────────────────────────
section "Step 5: Router Log Verification"
LOG="$ROOT/target/gbp-quic-test.log"

info "Checking router logs for QUIC startup messages..."
if grep -q "HTTP/3 QUIC endpoint bound" "$LOG" 2>/dev/null; then
    pass "Router log: HTTP/3 QUIC endpoint bound"
else
    fail "Router log missing QUIC endpoint bind message"
fi

if grep -q "ephemeral self-signed\|Loaded QUIC TLS" "$LOG" 2>/dev/null; then
    pass "Router log: TLS certificate configured"
else
    fail "Router log: missing TLS certificate message"
fi

if grep -q "QUIC connection established\|QUIC TLS handshake\|h3 response" "$LOG" 2>/dev/null; then
    pass "Router log: QUIC connection activity recorded"
else
    info "Router log: no QUIC connection activity (requires TRACE log level — expected at INFO)"
fi

printf "\n%s  Router log tail (last 25 lines):%s\n" "$YLW" "$RST"
tail -25 "$LOG" | sed 's/^/    /'

# ─── Summary ──────────────────────────────────────────────────────────────────
section "Summary"
TOTAL=$((PASS + FAIL))
printf "  Total   : %s%d%s\n" "$BOLD" "$TOTAL" "$RST"
printf "  %sPassed  : %d%s\n" "$GRN" "$PASS" "$RST"
printf "  %sFailed  : %d%s\n" "$RED" "$FAIL" "$RST"
echo

if [[ $FAIL -eq 0 ]]; then
    printf "%s%s  🎉  ALL %d TESTS PASSED — HTTP/3 QUIC is working end-to-end!%s\n" "$BOLD" "$GRN" "$TOTAL" "$RST"
    printf "  Protocol  : HTTP/3 (RFC 9114)\n"
    printf "  Transport : QUIC over UDP (RFC 9000)\n"
    printf "  TLS       : 1.3 (RFC 8446)\n"
    printf "  ALPN      : h3\n"
    printf "  Test log  : %s/target/gbp-quic-test.log\n" "$ROOT"
    exit 0
else
    printf "%s%s  💥  %d TEST(S) FAILED%s\n" "$BOLD" "$RED" "$FAIL" "$RST"
    printf "  See %s/target/gbp-quic-test.log for full router output.\n" "$ROOT"
    exit 1
fi
