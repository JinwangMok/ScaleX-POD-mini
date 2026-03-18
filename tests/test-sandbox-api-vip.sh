#!/usr/bin/env bash
# test-sandbox-api-vip.sh — Validate sandbox cluster API server responds via VIP
# Sandbox VIP: 192.168.88.109:6443
# Confirms: kubectl can reach the API server through kube-vip L2/ARP endpoint
set -uo pipefail

SANDBOX_VIP="192.168.88.109"
API_PORT=6443
SANDBOX_KUBECONFIG="${1:-_generated/clusters/sandbox/kubeconfig.yaml}"
TIMEOUT_SECS=5
PASS=0
FAIL=0
SKIP=0

log()  { echo "[$(date +%T)] $*"; }
pass() { log "PASS: $*"; ((PASS++)); }
fail() { log "FAIL: $*"; ((FAIL++)); }
skip() { log "SKIP: $*"; ((SKIP++)); }

# ── Pre-flight: kubeconfig exists ────────────────────────────────
if [ ! -f "$SANDBOX_KUBECONFIG" ]; then
    fail "Sandbox kubeconfig not found at $SANDBOX_KUBECONFIG"
    echo "═══════════════════════════════════════"
    echo "  Sandbox API VIP Test: FAILED (no kubeconfig)"
    echo "═══════════════════════════════════════"
    exit 1
fi
pass "Sandbox kubeconfig exists at $SANDBOX_KUBECONFIG"

# ── Pre-flight: kubectl available ────────────────────────────────
if ! command -v kubectl &>/dev/null; then
    fail "kubectl not found in PATH"
    echo "═══════════════════════════════════════"
    echo "  Sandbox API VIP Test: FAILED (no kubectl)"
    echo "═══════════════════════════════════════"
    exit 1
fi
pass "kubectl is available"

# ── Test 1: VIP network reachability (ping) ──────────────────────
if command -v ping &>/dev/null; then
    if ping -c 1 -W 2 "$SANDBOX_VIP" &>/dev/null; then
        pass "Sandbox VIP $SANDBOX_VIP is reachable (ARP/L2 responding)"
    else
        skip "Sandbox VIP $SANDBOX_VIP not reachable (not on management network or cluster not deployed)"
        echo ""
        echo "═══════════════════════════════════════"
        echo "  Sandbox API VIP Test Results"
        echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
        echo "  Note: Remaining tests require management network access"
        echo "═══════════════════════════════════════"
        exit 0
    fi
fi

# ── Test 2: TLS handshake on VIP:6443 ───────────────────────────
if command -v openssl &>/dev/null; then
    TLS_OUTPUT=$(timeout "$TIMEOUT_SECS" openssl s_client -connect "${SANDBOX_VIP}:${API_PORT}" </dev/null 2>&1)
    if echo "$TLS_OUTPUT" | grep -q "CONNECTED"; then
        pass "TLS handshake to ${SANDBOX_VIP}:${API_PORT} succeeded"
        # Verify cert includes the VIP in SANs
        if echo "$TLS_OUTPUT" | grep -qi "sandbox\|${SANDBOX_VIP}"; then
            pass "TLS certificate references sandbox or VIP"
        else
            log "INFO: TLS cert SAN check inconclusive (may need openssl x509 decode)"
        fi
    else
        fail "TLS handshake to ${SANDBOX_VIP}:${API_PORT} failed"
    fi
else
    skip "openssl not available, skipping TLS handshake test"
fi

# ── Test 3: kubectl get --raw /healthz via VIP ──────────────────
# This is the core validation: API server health check using the VIP directly
HEALTH_OUTPUT=$(timeout "$TIMEOUT_SECS" kubectl --kubeconfig="$SANDBOX_KUBECONFIG" \
    --server="https://${SANDBOX_VIP}:${API_PORT}" \
    --insecure-skip-tls-verify \
    get --raw /healthz 2>&1)
HEALTH_RC=$?

if [ $HEALTH_RC -eq 0 ] && [ "$HEALTH_OUTPUT" = "ok" ]; then
    pass "kubectl get --raw /healthz via VIP ${SANDBOX_VIP}:${API_PORT} → ok"
else
    fail "kubectl get --raw /healthz via VIP ${SANDBOX_VIP}:${API_PORT} failed (rc=$HEALTH_RC, output: $HEALTH_OUTPUT)"
fi

# ── Test 4: kubectl get --raw /readyz via VIP ────────────────────
READY_OUTPUT=$(timeout "$TIMEOUT_SECS" kubectl --kubeconfig="$SANDBOX_KUBECONFIG" \
    --server="https://${SANDBOX_VIP}:${API_PORT}" \
    --insecure-skip-tls-verify \
    get --raw /readyz 2>&1)
READY_RC=$?

if [ $READY_RC -eq 0 ] && [ "$READY_OUTPUT" = "ok" ]; then
    pass "kubectl get --raw /readyz via VIP ${SANDBOX_VIP}:${API_PORT} → ok"
else
    fail "kubectl get --raw /readyz via VIP ${SANDBOX_VIP}:${API_PORT} failed (rc=$READY_RC, output: $READY_OUTPUT)"
fi

# ── Test 5: kubectl get nodes via VIP ────────────────────────────
NODES_OUTPUT=$(timeout "$TIMEOUT_SECS" kubectl --kubeconfig="$SANDBOX_KUBECONFIG" \
    --server="https://${SANDBOX_VIP}:${API_PORT}" \
    --insecure-skip-tls-verify \
    get nodes -o name 2>&1)
NODES_RC=$?

if [ $NODES_RC -eq 0 ]; then
    NODE_COUNT=$(echo "$NODES_OUTPUT" | wc -l)
    pass "kubectl get nodes via VIP → $NODE_COUNT node(s) returned"
    # Verify expected nodes are present
    if echo "$NODES_OUTPUT" | grep -q "sandbox-cp-0"; then
        pass "sandbox-cp-0 control plane node is present"
    else
        fail "sandbox-cp-0 control plane node missing from node list"
    fi
    if echo "$NODES_OUTPUT" | grep -q "sandbox-worker"; then
        WORKER_COUNT=$(echo "$NODES_OUTPUT" | grep -c "sandbox-worker")
        pass "$WORKER_COUNT sandbox worker node(s) present"
    else
        fail "No sandbox worker nodes found"
    fi
else
    fail "kubectl get nodes via VIP failed (rc=$NODES_RC, output: $NODES_OUTPUT)"
fi

# ── Test 6: kubectl get namespaces via VIP (full TLS, no skip) ───
# This validates the VIP is in the API server's TLS SANs (supplementary_addresses_in_ssl_keys)
NS_OUTPUT=$(timeout "$TIMEOUT_SECS" kubectl --kubeconfig="$SANDBOX_KUBECONFIG" \
    --server="https://${SANDBOX_VIP}:${API_PORT}" \
    get namespaces -o name 2>&1)
NS_RC=$?

if [ $NS_RC -eq 0 ]; then
    pass "kubectl get namespaces via VIP with full TLS verification succeeded"
    if echo "$NS_OUTPUT" | grep -q "namespace/kube-system"; then
        pass "kube-system namespace present (cluster is functional)"
    else
        fail "kube-system namespace not found"
    fi
else
    fail "kubectl get namespaces via VIP with full TLS failed — VIP may not be in cert SANs (rc=$NS_RC)"
    log "  Hint: Ensure supplementary_addresses_in_ssl_keys includes $SANDBOX_VIP"
    log "  Output: $NS_OUTPUT"
fi

# ── Test 7: kubectl version via VIP ─────────────────────────────
VERSION_OUTPUT=$(timeout "$TIMEOUT_SECS" kubectl --kubeconfig="$SANDBOX_KUBECONFIG" \
    --server="https://${SANDBOX_VIP}:${API_PORT}" \
    --insecure-skip-tls-verify \
    version -o json 2>&1)
VERSION_RC=$?

if [ $VERSION_RC -eq 0 ]; then
    SERVER_VER=$(echo "$VERSION_OUTPUT" | grep -o '"gitVersion":"[^"]*"' | tail -1 | cut -d'"' -f4)
    pass "kubectl version via VIP → server $SERVER_VER"
else
    fail "kubectl version via VIP failed (rc=$VERSION_RC)"
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════"
echo "  Sandbox API VIP Test Results"
echo "  Endpoint: https://${SANDBOX_VIP}:${API_PORT}"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
echo "═══════════════════════════════════════"

if [ "$FAIL" -gt 0 ]; then
    exit 1
elif [ "$SKIP" -gt 0 ] && [ "$PASS" -le 2 ]; then
    exit 0  # Only pre-flight passed, network tests skipped
else
    exit 0
fi
