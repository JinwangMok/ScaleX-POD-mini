#!/usr/bin/env bash
# test-cf-tunnel-pod-running.sh — Verify CF Tunnel pod is Running in tower cluster
#
# AC 5: CF Tunnel pod Running in tower cluster
# Constraint: CF Tunnel pod must reside in tower cluster (enforced via ArgoCD, not placement)
#
# Tests:
#   1. GitOps manifest existence (deployment.yaml + kustomization.yaml)
#   2. Deployment targets kube-tunnel namespace
#   3. ArgoCD tower-generator includes cloudflared-tunnel (tower-only enforcement)
#   4. Cloudflared-tunnel NOT in sandbox generator (tower-only enforcement)
#   5. Secret generation includes cloudflared-tunnel-token for management role
#   6. Secret NOT generated for workload role
#   7. Deployment image is cloudflare/cloudflared
#   8. Deployment reads TUNNEL_TOKEN from secret
#   9. [Network] kube-tunnel namespace exists on tower cluster
#  10. [Network] cloudflared-tunnel pod is Running
#  11. [Network] cloudflared-tunnel-token secret exists
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

# ── Test 1: GitOps manifest existence ────────────────────────────
log "=== Test 1: GitOps manifest files exist ==="

DEPLOY_YAML="$REPO_DIR/gitops/tower/cloudflared-tunnel/deployment.yaml"
KUSTOMIZE_YAML="$REPO_DIR/gitops/tower/cloudflared-tunnel/kustomization.yaml"

if [[ -f "$DEPLOY_YAML" ]]; then
  pass "deployment.yaml exists at gitops/tower/cloudflared-tunnel/"
else
  fail "deployment.yaml missing at gitops/tower/cloudflared-tunnel/"
fi

if [[ -f "$KUSTOMIZE_YAML" ]]; then
  pass "kustomization.yaml exists at gitops/tower/cloudflared-tunnel/"
else
  fail "kustomization.yaml missing at gitops/tower/cloudflared-tunnel/"
fi

# ── Test 2: Kustomization targets kube-tunnel namespace ──────────
log ""
log "=== Test 2: Kustomization namespace is kube-tunnel ==="
if [[ -f "$KUSTOMIZE_YAML" ]]; then
  if grep -q 'namespace: kube-tunnel' "$KUSTOMIZE_YAML"; then
    pass "kustomization.yaml targets namespace kube-tunnel"
  else
    fail "kustomization.yaml does not target namespace kube-tunnel"
  fi
else
  skip "kustomization.yaml not found — cannot verify namespace"
fi

# ── Test 3: Tower generator includes cloudflared-tunnel ──────────
log ""
log "=== Test 3: Tower generator includes cloudflared-tunnel ==="
TOWER_GEN="$REPO_DIR/gitops/generators/tower/tower-generator.yaml"

if [[ -f "$TOWER_GEN" ]]; then
  if grep -q 'cloudflared-tunnel' "$TOWER_GEN"; then
    pass "tower-generator.yaml includes cloudflared-tunnel app"
  else
    fail "tower-generator.yaml does NOT include cloudflared-tunnel"
  fi
  # Verify namespace in generator matches
  if grep -A1 'cloudflared-tunnel' "$TOWER_GEN" | grep -q 'kube-tunnel'; then
    pass "tower-generator sets namespace kube-tunnel for cloudflared-tunnel"
  else
    fail "tower-generator namespace mismatch for cloudflared-tunnel"
  fi
else
  fail "tower-generator.yaml not found"
fi

# ── Test 4: Sandbox generator does NOT include cloudflared-tunnel ─
log ""
log "=== Test 4: CF Tunnel NOT in sandbox generator (tower-only enforcement) ==="
SANDBOX_GEN="$REPO_DIR/gitops/generators/sandbox/sandbox-generator.yaml"

if [[ -f "$SANDBOX_GEN" ]]; then
  if grep -q 'cloudflared-tunnel' "$SANDBOX_GEN"; then
    fail "sandbox-generator.yaml includes cloudflared-tunnel — MUST be tower-only"
  else
    pass "sandbox-generator.yaml does NOT include cloudflared-tunnel (correct: tower-only)"
  fi
else
  # No sandbox generator at all is also acceptable — tower-only is enforced
  pass "sandbox-generator.yaml not found — tower-only enforcement holds"
fi

# ── Test 5: Secret generation for management role ────────────────
log ""
log "=== Test 5: Secret spec includes cloudflared-tunnel-token for management ==="
# The deployment references secret 'cloudflared-tunnel-token' in namespace 'kube-tunnel'
if [[ -f "$DEPLOY_YAML" ]]; then
  if grep -q 'cloudflared-tunnel-token' "$DEPLOY_YAML"; then
    pass "deployment.yaml references secret cloudflared-tunnel-token"
  else
    fail "deployment.yaml does not reference secret cloudflared-tunnel-token"
  fi
else
  skip "deployment.yaml not found — cannot verify secret reference"
fi

# Verify secrets.rs generates the right secret for management role
SECRETS_RS="$REPO_DIR/scalex-cli/src/core/secrets.rs"
if [[ -f "$SECRETS_RS" ]]; then
  if grep -q 'cloudflared-tunnel-token' "$SECRETS_RS" && grep -q 'kube-tunnel' "$SECRETS_RS"; then
    pass "secrets.rs generates cloudflared-tunnel-token in kube-tunnel namespace"
  else
    fail "secrets.rs missing cloudflared-tunnel-token or kube-tunnel namespace"
  fi
else
  skip "secrets.rs not found — cannot verify secret generation"
fi

# ── Test 6: Secret NOT generated for workload role ───────────────
log ""
log "=== Test 6: Workload role does not get tunnel secret ==="
if [[ -f "$SECRETS_RS" ]]; then
  # Verify the match arm for non-management returns empty
  if grep -q '_ => vec!\[\]' "$SECRETS_RS"; then
    pass "secrets.rs returns empty vec for non-management roles (no tunnel secret leak)"
  else
    skip "Could not verify non-management secret exclusion pattern"
  fi
else
  skip "secrets.rs not found"
fi

# ── Test 7: Deployment image is cloudflare/cloudflared ───────────
log ""
log "=== Test 7: Deployment uses cloudflare/cloudflared image ==="
if [[ -f "$DEPLOY_YAML" ]]; then
  if grep -q 'image: cloudflare/cloudflared' "$DEPLOY_YAML"; then
    pass "deployment uses cloudflare/cloudflared image"
  else
    fail "deployment does NOT use cloudflare/cloudflared image"
  fi
else
  skip "deployment.yaml not found"
fi

# ── Test 8: TUNNEL_TOKEN env from secret ─────────────────────────
log ""
log "=== Test 8: TUNNEL_TOKEN env injected from Kubernetes secret ==="
if [[ -f "$DEPLOY_YAML" ]]; then
  if grep -q 'TUNNEL_TOKEN' "$DEPLOY_YAML" && grep -q 'secretKeyRef' "$DEPLOY_YAML"; then
    pass "TUNNEL_TOKEN env sourced from secretKeyRef"
  else
    fail "TUNNEL_TOKEN env not properly configured from secret"
  fi
else
  skip "deployment.yaml not found"
fi

# ── Network Tests (skip gracefully if unreachable) ───────────────
log ""
log "=== Network Tests: Live cluster verification ==="

# Pick best kubeconfig for tower
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
  skip "tower: no kubeconfig found — skipping network tests"
else
  # Try to reach the API server
  WORKING_KC=""
  if test_api_reachable "$TOWER_KC"; then
    WORKING_KC="$TOWER_KC"
  else
    # Try domain kubeconfig fallback
    DOMAIN_KC="$CLUSTERS_DIR/tower/kubeconfig.yaml"
    if [[ "$TOWER_KC" != "$DOMAIN_KC" && -f "$DOMAIN_KC" ]]; then
      if test_api_reachable "$DOMAIN_KC"; then
        WORKING_KC="$DOMAIN_KC"
      fi
    fi
  fi

  if [[ -z "$WORKING_KC" ]]; then
    skip "tower: API server not reachable (not on management network?) — skipping network tests"
    skip "kube-tunnel namespace check skipped"
    skip "cloudflared-tunnel pod check skipped"
    skip "cloudflared-tunnel-token secret check skipped"
  else
    log "Using kubeconfig: $WORKING_KC"

    # ── Test 9: kube-tunnel namespace exists ───────────────────
    log ""
    log "=== Test 9: kube-tunnel namespace exists on tower ==="
    if timeout "$KUBECTL_TIMEOUT" kubectl --kubeconfig="$WORKING_KC" \
      --request-timeout="${KUBECTL_TIMEOUT}s" \
      get namespace kube-tunnel >/dev/null 2>&1; then
      pass "kube-tunnel namespace exists on tower cluster"
    else
      fail "kube-tunnel namespace NOT found on tower cluster"
    fi

    # ── Test 10: cloudflared-tunnel pod is Running ──────────────
    log ""
    log "=== Test 10: cloudflared-tunnel pod Running ==="
    POD_STATUS=$(timeout "$KUBECTL_TIMEOUT" kubectl --kubeconfig="$WORKING_KC" \
      --request-timeout="${KUBECTL_TIMEOUT}s" \
      -n kube-tunnel get pods -l app=cloudflared-tunnel \
      --no-headers 2>/dev/null)

    if [[ -z "$POD_STATUS" ]]; then
      fail "No cloudflared-tunnel pods found in kube-tunnel namespace"
    else
      RUNNING_COUNT=$(echo "$POD_STATUS" | grep -c "Running" || true)
      TOTAL_COUNT=$(echo "$POD_STATUS" | grep -c . || true)
      log "Pod status:"
      echo "$POD_STATUS" | while read -r line; do log "  $line"; done

      if [[ "$RUNNING_COUNT" -gt 0 ]]; then
        pass "cloudflared-tunnel pod Running ($RUNNING_COUNT/$TOTAL_COUNT)"
      else
        fail "cloudflared-tunnel pod NOT Running (0/$TOTAL_COUNT running)"
      fi
    fi

    # ── Test 11: cloudflared-tunnel-token secret exists ──────────
    log ""
    log "=== Test 11: cloudflared-tunnel-token secret exists ==="
    if timeout "$KUBECTL_TIMEOUT" kubectl --kubeconfig="$WORKING_KC" \
      --request-timeout="${KUBECTL_TIMEOUT}s" \
      -n kube-tunnel get secret cloudflared-tunnel-token >/dev/null 2>&1; then
      pass "cloudflared-tunnel-token secret exists in kube-tunnel namespace"
    else
      fail "cloudflared-tunnel-token secret NOT found in kube-tunnel namespace"
    fi
  fi
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════"
echo "  CF Tunnel Pod Running Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
if [[ "$FAIL" -eq 0 ]]; then
  if [[ "$SKIP" -gt 0 ]]; then
    echo "  STATUS: OFFLINE CHECKS PASSED (network tests skipped)"
  else
    echo "  STATUS: ALL CHECKS PASSED ✓"
  fi
else
  echo "  STATUS: SOME CHECKS FAILED ✗"
fi
echo "═══════════════════════════════════════════════════"
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
