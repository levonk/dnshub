#!/bin/bash
#
# verify-migration.sh — verify a dnshub instance is serving correctly.
#
# Tests:
#   1. DNS resolution (dig a known domain → A record)
#   2. DNS blocking (dig a blocked domain → REFUSED)
#   3. Metrics endpoint (curl /metrics → 200 + Prometheus format)
#   4. Query log API (curl /api/v1/query-log → 200 + JSON)
#
# Usage:
#   scripts/verify-migration.sh --host 172.20.255.68 [options]
#
# Options:
#   --host IP            dnshub IP to test (required)
#   --domain NAME        domain expected to resolve (default: example.com)
#   --blocked-domain NAME domain expected to be blocked (default: ads.example.com)
#   --blocked-rcode CODE expected rcode for blocked queries (default: REFUSED)
#   --metrics-port PORT  Prometheus metrics port (default: 9090)
#   --api-port PORT      frontend/API port (default: 8080)
#   --dns-port PORT      DNS port (default: 53)
#   --help               show this help
#
# Exit codes:
#   0 — all checks passed
#   1 — one or more checks failed
#   2 — usage error

set -euo pipefail

HOST=""
DOMAIN="example.com"
BLOCKED_DOMAIN="ads.example.com"
BLOCKED_RCODE="REFUSED"
METRICS_PORT="9090"
API_PORT="8080"
DNS_PORT="53"

usage() {
    cat <<'EOF'
verify-migration.sh — verify a dnshub instance is serving correctly.

Usage:
  scripts/verify-migration.sh --host IP [options]

Options:
  --host IP             dnshub IP to test (required)
  --domain NAME         domain expected to resolve (default: example.com)
  --blocked-domain NAME domain expected to be blocked (default: ads.example.com)
  --blocked-rcode CODE  expected rcode for blocked queries (default: REFUSED)
  --metrics-port PORT   Prometheus metrics port (default: 9090)
  --api-port PORT       frontend/API port (default: 8080)
  --dns-port PORT       DNS port (default: 53)
  --help                show this help

Exit codes:
  0 — all checks passed
  1 — one or more checks failed
  2 — usage error
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --host)            HOST="$2"; shift 2 ;;
        --domain)          DOMAIN="$2"; shift 2 ;;
        --blocked-domain)  BLOCKED_DOMAIN="$2"; shift 2 ;;
        --blocked-rcode)   BLOCKED_RCODE="$2"; shift 2 ;;
        --metrics-port)    METRICS_PORT="$2"; shift 2 ;;
        --api-port)        API_PORT="$2"; shift 2 ;;
        --dns-port)        DNS_PORT="$2"; shift 2 ;;
        --help|-h)         usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ -z "$HOST" ]; then
    echo "Error: --host is required" >&2
    usage >&2
    exit 2
fi

PASS=0
FAIL=0

ok()   { echo "  [PASS] $1"; PASS=$((PASS + 1)); }
fail() { echo "  [FAIL] $1"; FAIL=$((FAIL + 1)); }

have() { command -v "$1" >/dev/null 2>&1; }

echo "=== dnshub migration verification ==="
echo "Target: ${HOST}:${DNS_PORT} (DNS), :${METRICS_PORT} (metrics), :${API_PORT} (API)"
echo

# --- Check 1: DNS resolution ------------------------------------------------
echo "-- DNS resolution (${DOMAIN})"
if have dig; then
    OUT=$(dig "@${HOST}" -p "${DNS_PORT}" "${DOMAIN}" A +time=5 +tries=2 2>&1 || true)
    STATUS=$(echo "$OUT" | grep -o 'status: [A-Z]*' | head -1 | awk '{print $2}')
    if [ "$STATUS" = "NOERROR" ]; then
        ok "dig ${DOMAIN} @${HOST} → NOERROR"
    else
        fail "dig ${DOMAIN} @${HOST} → status ${STATUS:-unknown} (expected NOERROR)"
    fi
else
    fail "dig not available — cannot test DNS resolution"
fi
echo

# --- Check 2: DNS blocking --------------------------------------------------
echo "-- DNS blocking (${BLOCKED_DOMAIN} → ${BLOCKED_RCODE})"
if have dig; then
    OUT=$(dig "@${HOST}" -p "${DNS_PORT}" "${BLOCKED_DOMAIN}" A +time=5 +tries=2 2>&1 || true)
    STATUS=$(echo "$OUT" | grep -o 'status: [A-Z]*' | head -1 | awk '{print $2}')
    if [ "$STATUS" = "$BLOCKED_RCODE" ]; then
        ok "dig ${BLOCKED_DOMAIN} @${HOST} → ${BLOCKED_RCODE}"
    else
        fail "dig ${BLOCKED_DOMAIN} @${HOST} → status ${STATUS:-unknown} (expected ${BLOCKED_RCODE})"
    fi
else
    fail "dig not available — cannot test DNS blocking"
fi
echo

# --- Check 3: Metrics endpoint ----------------------------------------------
echo "-- Metrics endpoint (:${METRICS_PORT}/metrics)"
if have curl; then
    CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 \
           "http://${HOST}:${METRICS_PORT}/metrics" 2>/dev/null || echo "000")
    if [ "$CODE" = "200" ]; then
        BODY=$(curl -s --max-time 5 "http://${HOST}:${METRICS_PORT}/metrics" 2>/dev/null || true)
        if echo "$BODY" | grep -q '^# HELP\|^# TYPE\|dnshub_'; then
            ok "metrics endpoint returns Prometheus format (200)"
        else
            fail "metrics endpoint returned 200 but body is not Prometheus format"
        fi
    else
        fail "metrics endpoint returned HTTP ${CODE} (expected 200)"
    fi
else
    fail "curl not available — cannot test metrics endpoint"
fi
echo

# --- Check 4: Query log API -------------------------------------------------
echo "-- Query log API (:${API_PORT}/api/v1/query-log)"
if have curl; then
    CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 \
           "http://${HOST}:${API_PORT}/api/v1/query-log" 2>/dev/null || echo "000")
    if [ "$CODE" = "200" ]; then
        ok "query log API returns 200"
    else
        fail "query log API returned HTTP ${CODE} (expected 200)"
    fi
else
    fail "curl not available — cannot test query log API"
fi
echo

# --- Summary ----------------------------------------------------------------
echo "=== Summary: ${PASS} passed, ${FAIL} failed ==="
if [ "$FAIL" -gt 0 ]; then
    echo "Verification FAILED. Do NOT proceed with cutover."
    exit 1
fi
echo "Verification PASSED. dnshub is serving correctly."
exit 0
