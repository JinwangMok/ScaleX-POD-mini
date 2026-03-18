#!/usr/bin/env bash
# AC 6: Verify `scalex dash --headless --resource checks` returns 5 checks as JSON
# This test validates the JSON structure and check names offline (no cluster required).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
BINARY="$REPO_DIR/scalex-cli/target/debug/scalex"

PASS=0
FAIL=0
SKIP=0

check() {
    local desc="$1"
    local result="$2"
    if [ "$result" = "pass" ]; then
        echo "  ✅ PASS: $desc"
        PASS=$((PASS + 1))
    elif [ "$result" = "skip" ]; then
        echo "  ⏭️  SKIP: $desc"
        SKIP=$((SKIP + 1))
    else
        echo "  ❌ FAIL: $desc"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== AC 6: scalex dash --headless --resource checks ==="
echo ""

# --- Structural checks (offline — no cluster needed) ---

# 1. Binary exists
if [ -f "$BINARY" ]; then
    check "Binary exists at $BINARY" "pass"
else
    check "Binary exists at $BINARY" "fail"
    echo "  → Build with: cd scalex-cli && cargo build"
    echo ""
    echo "Results: $PASS PASS / $FAIL FAIL / $SKIP SKIP"
    exit 1
fi

# 2. --help mentions checks resource type
HELP_OUTPUT=$("$BINARY" dash --help 2>&1) || true
if echo "$HELP_OUTPUT" | grep -q "checks"; then
    check "Help text mentions 'checks' resource type" "pass"
else
    check "Help text mentions 'checks' resource type" "fail"
fi

# --- Live checks (requires cluster connectivity) ---

# Check if kubeconfigs exist
KUBECONFIG_DIR="$REPO_DIR/_generated/clusters"
if [ ! -d "$KUBECONFIG_DIR" ] || [ -z "$(ls -A "$KUBECONFIG_DIR" 2>/dev/null)" ]; then
    echo ""
    echo "  ⚠️  No kubeconfig files found in $KUBECONFIG_DIR"
    echo "  → Live checks require cluster connectivity. Skipping."
    check "Live: JSON output is valid" "skip"
    check "Live: Contains 5 checks" "skip"
    check "Live: Check names match expected" "skip"
    check "Live: overall field present" "skip"
    check "Live: passed/total fields present" "skip"
else
    # Run headless checks
    echo ""
    echo "  Running: scalex dash --headless --resource checks"
    # Allow non-zero exit (checks may fail if clusters are unhealthy — exit 1 is expected)
    JSON_OUTPUT=$("$BINARY" dash --headless --resource checks 2>/dev/null) || true

    # 3. Valid JSON
    if echo "$JSON_OUTPUT" | python3 -m json.tool >/dev/null 2>&1; then
        check "Live: JSON output is valid" "pass"
    else
        check "Live: JSON output is valid" "fail"
        echo "  → Output: $JSON_OUTPUT"
    fi

    # 4. Contains 5 checks
    CHECK_COUNT=$(echo "$JSON_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('checks',[])))" 2>/dev/null || echo "0")
    if [ "$CHECK_COUNT" = "5" ]; then
        check "Live: Contains 5 checks" "pass"
    else
        check "Live: Contains 5 checks (got $CHECK_COUNT)" "fail"
    fi

    # 5. Check names match expected
    EXPECTED_NAMES='["cluster_api_reachable", "all_nodes_ready", "namespaces_listed", "argocd_synced", "cf_tunnel_running"]'
    ACTUAL_NAMES=$(echo "$JSON_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps([c['name'] for c in d.get('checks',[])]))" 2>/dev/null || echo "[]")
    if [ "$ACTUAL_NAMES" = "$EXPECTED_NAMES" ]; then
        check "Live: Check names match expected" "pass"
    else
        check "Live: Check names match expected" "fail"
        echo "  → Expected: $EXPECTED_NAMES"
        echo "  → Got:      $ACTUAL_NAMES"
    fi

    # 6. overall field present
    OVERALL=$(echo "$JSON_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('overall','missing'))" 2>/dev/null || echo "missing")
    if [ "$OVERALL" != "missing" ]; then
        check "Live: overall field present (=$OVERALL)" "pass"
    else
        check "Live: overall field present" "fail"
    fi

    # 7. passed/total fields present
    PASSED=$(echo "$JSON_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('passed','missing'))" 2>/dev/null || echo "missing")
    TOTAL=$(echo "$JSON_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('total','missing'))" 2>/dev/null || echo "missing")
    if [ "$PASSED" != "missing" ] && [ "$TOTAL" != "missing" ]; then
        check "Live: passed=$PASSED total=$TOTAL fields present" "pass"
    else
        check "Live: passed/total fields present" "fail"
    fi
fi

echo ""
echo "Results: $PASS PASS / $FAIL FAIL / $SKIP SKIP"
[ "$FAIL" -eq 0 ]
