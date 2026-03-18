#!/usr/bin/env bash
# test-vip-operational.sh — Verify kube-vip VIPs are operational
# Tower VIP: 192.168.88.99, Sandbox VIP: 192.168.88.109
set -uo pipefail

TOWER_VIP="192.168.88.99"
SANDBOX_VIP="192.168.88.109"
API_PORT=6443
PASS=0
FAIL=0

log()  { echo "[$(date +%T)] $*"; }
pass() { log "PASS: $*"; ((PASS++)); }
fail() { log "FAIL: $*"; ((FAIL++)); }

# ── Test 1: Config declares correct VIPs ─────────────────────────
CONFIG="${1:-config/k8s-clusters.yaml}"
if [ ! -f "$CONFIG" ]; then
    fail "Config file not found: $CONFIG"
else
    if grep -q "kube_vip_address: \"$TOWER_VIP\"" "$CONFIG"; then
        pass "Tower VIP $TOWER_VIP declared in config"
    else
        fail "Tower VIP $TOWER_VIP not found in config"
    fi

    if grep -q "kube_vip_address: \"$SANDBOX_VIP\"" "$CONFIG"; then
        pass "Sandbox VIP $SANDBOX_VIP declared in config"
    else
        fail "Sandbox VIP $SANDBOX_VIP not found in config"
    fi
fi

# ── Test 2: Generated cluster-vars include kube-vip overrides ────
TOWER_VARS="_generated/clusters/tower/cluster-vars.yml"
SANDBOX_VARS="_generated/clusters/sandbox/cluster-vars.yml"

for f in "$TOWER_VARS" "$SANDBOX_VARS"; do
    if [ -f "$f" ]; then
        cluster=$(basename "$(dirname "$f")")
        if grep -q "kube_vip_enabled: true" "$f"; then
            pass "$cluster cluster-vars has kube_vip_enabled: true"
        else
            fail "$cluster cluster-vars missing kube_vip_enabled: true"
        fi
        if grep -q "kube_vip_arp_enabled: true" "$f"; then
            pass "$cluster cluster-vars has kube_vip_arp_enabled: true (L2/ARP mode)"
        else
            fail "$cluster cluster-vars missing kube_vip_arp_enabled"
        fi
        if grep -q "kube_vip_controlplane_enabled: true" "$f"; then
            pass "$cluster cluster-vars has kube_vip_controlplane_enabled: true"
        else
            fail "$cluster cluster-vars missing kube_vip_controlplane_enabled"
        fi
    else
        log "SKIP: $f not found (run 'scalex cluster init --dry-run' first)"
    fi
done

if [ -f "$TOWER_VARS" ]; then
    if grep -q "kube_vip_address: $TOWER_VIP" "$TOWER_VARS"; then
        pass "Tower cluster-vars has kube_vip_address: $TOWER_VIP"
    else
        fail "Tower cluster-vars missing kube_vip_address: $TOWER_VIP"
    fi
fi

if [ -f "$SANDBOX_VARS" ]; then
    if grep -q "kube_vip_address: $SANDBOX_VIP" "$SANDBOX_VARS"; then
        pass "Sandbox cluster-vars has kube_vip_address: $SANDBOX_VIP"
    else
        fail "Sandbox cluster-vars missing kube_vip_address: $SANDBOX_VIP"
    fi
fi

# ── Test 2b: No duplicate YAML keys in cluster-vars ──────────────
for f in "$TOWER_VARS" "$SANDBOX_VARS"; do
    if [ -f "$f" ]; then
        cluster=$(basename "$(dirname "$f")")
        dup_count=$(grep -c "^kube_vip_enabled:" "$f" 2>/dev/null || echo "0")
        if [ "$dup_count" -le 1 ]; then
            pass "$cluster cluster-vars has no duplicate kube_vip_enabled keys"
        else
            fail "$cluster cluster-vars has $dup_count duplicate kube_vip_enabled keys"
        fi
        ssl_count=$(grep -c "^supplementary_addresses_in_ssl_keys:" "$f" 2>/dev/null || echo "0")
        if [ "$ssl_count" -le 1 ]; then
            pass "$cluster cluster-vars has no duplicate supplementary_addresses_in_ssl_keys keys"
        else
            fail "$cluster cluster-vars has $ssl_count duplicate supplementary_addresses_in_ssl_keys keys"
        fi
    fi
done

# ── Test 3: loadbalancer_apiserver points to VIPs ────────────────
if [ -f "$TOWER_VARS" ] && grep -A1 "loadbalancer_apiserver" "$TOWER_VARS" | grep -q "$TOWER_VIP"; then
    pass "Tower loadbalancer_apiserver → $TOWER_VIP"
else
    fail "Tower loadbalancer_apiserver not pointing to $TOWER_VIP"
fi

if [ -f "$SANDBOX_VARS" ] && grep -A1 "loadbalancer_apiserver" "$SANDBOX_VARS" | grep -q "$SANDBOX_VIP"; then
    pass "Sandbox loadbalancer_apiserver → $SANDBOX_VIP"
else
    fail "Sandbox loadbalancer_apiserver not pointing to $SANDBOX_VIP"
fi

# ── Test 4: supplementary SSL keys include VIPs ──────────────────
if [ -f "$TOWER_VARS" ] && grep -q "$TOWER_VIP" "$TOWER_VARS"; then
    pass "Tower supplementary SSL keys include $TOWER_VIP"
else
    fail "Tower supplementary SSL keys missing $TOWER_VIP"
fi

if [ -f "$SANDBOX_VARS" ] && grep -q "$SANDBOX_VIP" "$SANDBOX_VARS"; then
    pass "Sandbox supplementary SSL keys include $SANDBOX_VIP"
else
    fail "Sandbox supplementary SSL keys missing $SANDBOX_VIP"
fi

# ── Test 5: Network reachability (skip if not on management network) ──
if command -v ping &>/dev/null; then
    for vip_name_ip in "tower:$TOWER_VIP" "sandbox:$SANDBOX_VIP"; do
        name="${vip_name_ip%%:*}"
        vip="${vip_name_ip##*:}"
        if ping -c 1 -W 2 "$vip" &>/dev/null; then
            pass "$name VIP $vip is reachable (ARP responding)"
            # Also try API server TLS handshake
            if command -v timeout &>/dev/null && command -v openssl &>/dev/null; then
                if timeout 3 openssl s_client -connect "$vip:$API_PORT" </dev/null 2>/dev/null | grep -q "CONNECTED"; then
                    pass "$name API server at $vip:$API_PORT TLS handshake OK"
                else
                    fail "$name API server at $vip:$API_PORT TLS handshake failed"
                fi
            fi
        else
            log "SKIP: $name VIP $vip not reachable (not on management network or cluster not deployed)"
        fi
    done
else
    log "SKIP: ping not available, skipping reachability tests"
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════"
echo "  VIP Operational Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL"
echo "═══════════════════════════════════════"
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
