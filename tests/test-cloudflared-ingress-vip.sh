#!/usr/bin/env bash
# test-cloudflared-ingress-vip.sh — Verify cloudflared ingress rules point to VIP addresses
#
# AC 11: cloudflared ingress rules point to VIP addresses
#
# Tests:
#   1. ConfigMap manifest exists at gitops/tower/cloudflared-tunnel/config.yaml
#   2. Kustomization includes config.yaml resource
#   3. tower-api ingress rule points to tower VIP (192.168.88.99)
#   4. sandbox-api ingress rule points to sandbox VIP (192.168.88.109)
#   5. No individual node IPs in ingress rules (not .100/.101/.102/.110/.120/.121/.122)
#   6. Deployment mounts cloudflared-config volume
#   7. Deployment uses --config flag pointing to mounted path
#   8. ArgoCD ingress rule uses in-cluster service (cd.jinwang.dev)
#   9. Keycloak ingress rule uses in-cluster service (auth.jinwang.dev)
#  10. Catch-all rule returns 404
#  11. VIP addresses in ingress match VIPs in k8s-clusters.yaml
#  12. [Network] ConfigMap exists in kube-tunnel namespace on tower cluster
set -uo pipefail

REPO_DIR="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
CLUSTERS_DIR="$REPO_DIR/_generated/clusters"
KUBECTL_TIMEOUT=8

PASS=0
FAIL=0
SKIP=0

log()  { echo "[$(date +%T)] $*"; }
pass() { log "PASS: $*"; ((PASS++)); }
fail() { log "FAIL: $*"; ((FAIL++)); }
skip() { log "SKIP: $*"; ((SKIP++)); }

CONFIG_YAML="$REPO_DIR/gitops/tower/cloudflared-tunnel/config.yaml"
DEPLOY_YAML="$REPO_DIR/gitops/tower/cloudflared-tunnel/deployment.yaml"
KUSTOMIZE_YAML="$REPO_DIR/gitops/tower/cloudflared-tunnel/kustomization.yaml"
CLUSTERS_YAML="$REPO_DIR/config/k8s-clusters.yaml"

TOWER_VIP="192.168.88.99"
SANDBOX_VIP="192.168.88.109"

# ── Test 1: ConfigMap manifest exists ──────────────────────────────
log "=== Test 1: ConfigMap manifest exists ==="
if [[ -f "$CONFIG_YAML" ]]; then
  pass "config.yaml exists at gitops/tower/cloudflared-tunnel/"
else
  fail "config.yaml missing at gitops/tower/cloudflared-tunnel/"
fi

# ── Test 2: Kustomization includes config.yaml ────────────────────
log ""
log "=== Test 2: Kustomization includes config.yaml resource ==="
if [[ -f "$KUSTOMIZE_YAML" ]]; then
  if grep -q 'config.yaml' "$KUSTOMIZE_YAML"; then
    pass "kustomization.yaml includes config.yaml resource"
  else
    fail "kustomization.yaml does NOT include config.yaml resource"
  fi
else
  fail "kustomization.yaml not found"
fi

# ── Test 3: tower-api ingress points to tower VIP ─────────────────
log ""
log "=== Test 3: tower-api.jinwang.dev ingress → tower VIP ($TOWER_VIP) ==="
if [[ -f "$CONFIG_YAML" ]]; then
  # Check that tower-api hostname has a service pointing to tower VIP
  if grep -A2 'tower-api.jinwang.dev' "$CONFIG_YAML" | grep -q "$TOWER_VIP"; then
    pass "tower-api.jinwang.dev routes to tower VIP $TOWER_VIP"
  else
    fail "tower-api.jinwang.dev does NOT route to tower VIP $TOWER_VIP"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 4: sandbox-api ingress points to sandbox VIP ─────────────
log ""
log "=== Test 4: sandbox-api.jinwang.dev ingress → sandbox VIP ($SANDBOX_VIP) ==="
if [[ -f "$CONFIG_YAML" ]]; then
  if grep -A2 'sandbox-api.jinwang.dev' "$CONFIG_YAML" | grep -q "$SANDBOX_VIP"; then
    pass "sandbox-api.jinwang.dev routes to sandbox VIP $SANDBOX_VIP"
  else
    fail "sandbox-api.jinwang.dev does NOT route to sandbox VIP $SANDBOX_VIP"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 5: No individual node IPs in ingress rules ───────────────
log ""
log "=== Test 5: No individual node IPs in ingress rules ==="
if [[ -f "$CONFIG_YAML" ]]; then
  # Extract only the ingress service lines (not comments)
  NODE_IPS_FOUND=false
  for ip in "192.168.88.100" "192.168.88.101" "192.168.88.102" \
            "192.168.88.110" "192.168.88.120" "192.168.88.121" "192.168.88.122"; do
    if grep -v '^\s*#' "$CONFIG_YAML" | grep -q "$ip"; then
      fail "Individual node IP $ip found in ingress rules (should use VIP)"
      NODE_IPS_FOUND=true
    fi
  done
  if [[ "$NODE_IPS_FOUND" == "false" ]]; then
    pass "No individual node IPs found in ingress rules (only VIPs used)"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 6: Deployment mounts cloudflared-config volume ───────────
log ""
log "=== Test 6: Deployment mounts cloudflared-config volume ==="
if [[ -f "$DEPLOY_YAML" ]]; then
  if grep -q 'cloudflared-config' "$DEPLOY_YAML" && grep -q 'mountPath' "$DEPLOY_YAML"; then
    pass "deployment mounts cloudflared-config volume"
  else
    fail "deployment does NOT mount cloudflared-config volume"
  fi
else
  skip "deployment.yaml not found"
fi

# ── Test 7: Deployment uses --config flag ─────────────────────────
log ""
log "=== Test 7: Deployment uses --config flag ==="
if [[ -f "$DEPLOY_YAML" ]]; then
  if grep -q '\-\-config' "$DEPLOY_YAML"; then
    pass "deployment uses --config flag for cloudflared"
  else
    fail "deployment does NOT use --config flag"
  fi
else
  skip "deployment.yaml not found"
fi

# ── Test 8: ArgoCD ingress rule (cd.jinwang.dev) ──────────────────
log ""
log "=== Test 8: ArgoCD ingress rule uses in-cluster service ==="
if [[ -f "$CONFIG_YAML" ]]; then
  if grep -A2 'cd.jinwang.dev' "$CONFIG_YAML" | grep -q 'argocd-server'; then
    pass "cd.jinwang.dev routes to argocd-server in-cluster service"
  else
    fail "cd.jinwang.dev does NOT route to argocd-server service"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 9: Keycloak ingress rule (auth.jinwang.dev) ─────────────
log ""
log "=== Test 9: Keycloak ingress rule uses in-cluster service ==="
if [[ -f "$CONFIG_YAML" ]]; then
  if grep -A2 'auth.jinwang.dev' "$CONFIG_YAML" | grep -q 'keycloak'; then
    pass "auth.jinwang.dev routes to keycloak in-cluster service"
  else
    fail "auth.jinwang.dev does NOT route to keycloak service"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 10: Catch-all 404 rule ──────────────────────────────────
log ""
log "=== Test 10: Catch-all rule returns 404 ==="
if [[ -f "$CONFIG_YAML" ]]; then
  if grep -q 'http_status:404' "$CONFIG_YAML"; then
    pass "catch-all ingress rule returns http_status:404"
  else
    fail "no catch-all http_status:404 rule found"
  fi
else
  skip "config.yaml not found"
fi

# ── Test 11: VIPs match k8s-clusters.yaml ─────────────────────────
log ""
log "=== Test 11: VIP addresses match k8s-clusters.yaml ==="
if [[ -f "$CLUSTERS_YAML" && -f "$CONFIG_YAML" ]]; then
  # Extract kube_vip_address values from clusters config
  TOWER_VIP_CONFIG=$(grep 'kube_vip_address:' "$CLUSTERS_YAML" | head -1 | tr -d ' "' | cut -d: -f2)
  SANDBOX_VIP_CONFIG=$(grep 'kube_vip_address:' "$CLUSTERS_YAML" | tail -1 | tr -d ' "' | cut -d: -f2)

  MATCH=true
  if grep -v '^\s*#' "$CONFIG_YAML" | grep -q "$TOWER_VIP_CONFIG"; then
    log "  tower VIP in config ($TOWER_VIP_CONFIG) matches ingress rules"
  else
    fail "tower VIP ($TOWER_VIP_CONFIG) from k8s-clusters.yaml not found in ingress rules"
    MATCH=false
  fi

  if grep -v '^\s*#' "$CONFIG_YAML" | grep -q "$SANDBOX_VIP_CONFIG"; then
    log "  sandbox VIP in config ($SANDBOX_VIP_CONFIG) matches ingress rules"
  else
    fail "sandbox VIP ($SANDBOX_VIP_CONFIG) from k8s-clusters.yaml not found in ingress rules"
    MATCH=false
  fi

  if [[ "$MATCH" == "true" ]]; then
    pass "VIP addresses in ingress rules match k8s-clusters.yaml"
  fi
else
  skip "k8s-clusters.yaml or config.yaml not found — cannot cross-validate VIPs"
fi

# ── Network Tests (skip gracefully if unreachable) ───────────────
log ""
log "=== Network Tests: Live cluster verification ==="

pick_kubeconfig() {
  local cluster="$1"
  local orig="$CLUSTERS_DIR/$cluster/kubeconfig.yaml.original"
  local domain="$CLUSTERS_DIR/$cluster/kubeconfig.yaml"
  if [[ -f "$orig" ]]; then echo "$orig"
  elif [[ -f "$domain" ]]; then echo "$domain"
  else echo ""
  fi
}

test_api_reachable() {
  local kc="$1"
  local server_url
  server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
  curl -sk --connect-timeout 3 --max-time 5 "${server_url}/healthz" 2>/dev/null | grep -q "ok"
}

TOWER_KC=$(pick_kubeconfig "tower")

if [[ -z "$TOWER_KC" ]]; then
  skip "tower: no kubeconfig found — skipping network test"
else
  WORKING_KC=""
  if test_api_reachable "$TOWER_KC"; then
    WORKING_KC="$TOWER_KC"
  else
    DOMAIN_KC="$CLUSTERS_DIR/tower/kubeconfig.yaml"
    if [[ "$TOWER_KC" != "$DOMAIN_KC" && -f "$DOMAIN_KC" ]]; then
      if test_api_reachable "$DOMAIN_KC"; then
        WORKING_KC="$DOMAIN_KC"
      fi
    fi
  fi

  if [[ -z "$WORKING_KC" ]]; then
    skip "tower: API server not reachable (not on management network?) — skipping network test"
  else
    # ── Test 12: ConfigMap exists in kube-tunnel namespace ────────
    log ""
    log "=== Test 12: cloudflared-config ConfigMap exists on tower ==="
    if timeout "$KUBECTL_TIMEOUT" kubectl --kubeconfig="$WORKING_KC" \
      --request-timeout="${KUBECTL_TIMEOUT}s" \
      -n kube-tunnel get configmap cloudflared-config >/dev/null 2>&1; then
      pass "cloudflared-config ConfigMap exists in kube-tunnel namespace"
    else
      fail "cloudflared-config ConfigMap NOT found in kube-tunnel namespace"
    fi
  fi
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════"
echo "  Cloudflared Ingress VIP Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
if [[ "$FAIL" -eq 0 ]]; then
  if [[ "$SKIP" -gt 0 ]]; then
    echo "  STATUS: OFFLINE CHECKS PASSED (network tests skipped)"
  else
    echo "  STATUS: ALL CHECKS PASSED"
  fi
else
  echo "  STATUS: SOME CHECKS FAILED"
fi
echo "═══════════════════════════════════════════════════"
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
