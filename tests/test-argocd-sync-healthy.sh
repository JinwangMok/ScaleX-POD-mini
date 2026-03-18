#!/usr/bin/env bash
# test-argocd-sync-healthy.sh — Verify all ArgoCD Applications report Synced/Healthy
#
# Expected applications (from gitops/bootstrap/spread.yaml + generators):
#   Root (3):   tower-root, sandbox-root, cluster-projects
#   Tower (7):  tower-cilium, tower-local-path-provisioner, tower-cluster-config,
#               tower-argocd, tower-cert-issuers, tower-keycloak, tower-cloudflared-tunnel
#   Tower Common (4): tower-cilium-resources, tower-cert-manager, tower-kyverno,
#                     tower-kyverno-policies
#   Sandbox (5): sandbox-cluster-config, sandbox-cilium, sandbox-local-path-provisioner,
#                sandbox-rbac, sandbox-test-resources
#   Sandbox Common (4): sandbox-cilium-resources, sandbox-cert-manager, sandbox-kyverno,
#                       sandbox-kyverno-policies
#   Total: 23 Applications
#
# ArgoCD runs on tower cluster only (hub-spoke model).
# Uses .original kubeconfigs (direct VM IPs) with fallback to domain kubeconfigs.
# Network-dependent tests SKIP gracefully when API server is unreachable.
set -uo pipefail

REPO_DIR="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
CLUSTERS_DIR="$REPO_DIR/_generated/clusters"
KUBECTL_TIMEOUT=10  # seconds — ArgoCD CRD queries can be slower

PASS=0
FAIL=0
SKIP=0

log()  { echo "[$(date +%T)] $*"; }
pass() { log "PASS: $*"; ((PASS++)); }
fail() { log "FAIL: $*"; ((FAIL++)); }
skip() { log "SKIP: $*"; ((SKIP++)); }

# ── Expected applications ───────────────────────────────────────
# Root-level apps from spread.yaml
ROOT_APPS="tower-root sandbox-root cluster-projects"

# Tower-specific apps (tower-generator.yaml)
TOWER_APPS="tower-cilium tower-local-path-provisioner tower-cluster-config tower-argocd tower-cert-issuers tower-keycloak tower-cloudflared-tunnel"

# Tower common apps (common-generator.yaml for tower)
TOWER_COMMON_APPS="tower-cilium-resources tower-cert-manager tower-kyverno tower-kyverno-policies"

# Sandbox-specific apps (sandbox-generator.yaml)
SANDBOX_APPS="sandbox-cluster-config sandbox-cilium sandbox-local-path-provisioner sandbox-rbac sandbox-test-resources"

# Sandbox common apps (common-generator.yaml for sandbox)
SANDBOX_COMMON_APPS="sandbox-cilium-resources sandbox-cert-manager sandbox-kyverno sandbox-kyverno-policies"

ALL_EXPECTED_APPS="$ROOT_APPS $TOWER_APPS $TOWER_COMMON_APPS $SANDBOX_APPS $SANDBOX_COMMON_APPS"
EXPECTED_APP_COUNT=23

# ── Helper: pick best kubeconfig ────────────────────────────────
pick_kubeconfig() {
  local cluster="$1"
  local orig="$CLUSTERS_DIR/$cluster/kubeconfig.yaml.original"
  local domain="$CLUSTERS_DIR/$cluster/kubeconfig.yaml"
  if [[ -f "$orig" ]]; then
    echo "$orig"
  elif [[ -f "$domain" ]]; then
    echo "$domain"
  else
    echo ""
  fi
}

# ── Helper: test API server reachability ─────────────────────────
test_api_reachable() {
  local kc="$1"
  local server_url
  server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
  curl -sk --connect-timeout 3 --max-time 5 "${server_url}/healthz" 2>/dev/null | grep -q "ok"
}

# ── Helper: get ArgoCD applications JSON ─────────────────────────
get_argocd_apps_json() {
  local kc="$1"
  timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$kc" \
    --request-timeout="${KUBECTL_TIMEOUT}s" \
    -n argocd get applications.argoproj.io -o json 2>/dev/null
}

# ── Test 1: Config file existence ────────────────────────────────
log "=== Test 1: Tower kubeconfig existence ==="
TOWER_KC=$(pick_kubeconfig "tower")
if [[ -z "$TOWER_KC" ]]; then
  fail "tower: no kubeconfig found at $CLUSTERS_DIR/tower/"
  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  ArgoCD Sync/Healthy Test Results"
  echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
  echo "  STATUS: CANNOT TEST — no tower kubeconfig"
  echo "═══════════════════════════════════════════════════"
  exit 1
else
  pass "tower: kubeconfig found at $TOWER_KC"
fi

# ── Test 2: Tower API server reachability ────────────────────────
log ""
log "=== Test 2: Tower API server reachability ==="
WORKING_KC=""

if test_api_reachable "$TOWER_KC"; then
  pass "tower: API server reachable via $TOWER_KC"
  WORKING_KC="$TOWER_KC"
else
  # Try domain kubeconfig fallback
  DOMAIN_KC="$CLUSTERS_DIR/tower/kubeconfig.yaml"
  if [[ "$TOWER_KC" != "$DOMAIN_KC" && -f "$DOMAIN_KC" ]]; then
    if test_api_reachable "$DOMAIN_KC"; then
      pass "tower: API server reachable via $DOMAIN_KC (domain fallback)"
      WORKING_KC="$DOMAIN_KC"
    fi
  fi
fi

if [[ -z "$WORKING_KC" ]]; then
  skip "tower: API server not reachable (not on management network?)"
  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  ArgoCD Sync/Healthy Test Results"
  echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
  echo "  STATUS: NETWORK TESTS SKIPPED (not on management network)"
  echo "  Config validation passed."
  echo "═══════════════════════════════════════════════════"
  exit 0
fi

# ── Test 3: ArgoCD CRD exists ────────────────────────────────────
log ""
log "=== Test 3: ArgoCD CRD availability ==="
if timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$WORKING_KC" \
  --request-timeout="${KUBECTL_TIMEOUT}s" \
  get crd applications.argoproj.io >/dev/null 2>&1; then
  pass "ArgoCD Application CRD exists"
else
  fail "ArgoCD Application CRD not found — ArgoCD not installed?"
  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  ArgoCD Sync/Healthy Test Results"
  echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
  echo "  STATUS: ARGOCD NOT INSTALLED"
  echo "═══════════════════════════════════════════════════"
  exit 1
fi

# ── Test 4: Retrieve all ArgoCD Applications ─────────────────────
log ""
log "=== Test 4: ArgoCD Application discovery ==="
APPS_JSON=$(get_argocd_apps_json "$WORKING_KC")

if [[ -z "$APPS_JSON" || "$APPS_JSON" == "null" ]]; then
  fail "Cannot retrieve ArgoCD applications (kubectl failed)"
  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  ArgoCD Sync/Healthy Test Results"
  echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
  echo "  STATUS: CANNOT RETRIEVE APPLICATIONS"
  echo "═══════════════════════════════════════════════════"
  exit 1
fi

# Parse application names, sync status, and health status
APP_DATA=$(echo "$APPS_JSON" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for app in data.get('items', []):
    name = app['metadata']['name']
    status = app.get('status', {})
    sync = status.get('sync', {}).get('status', 'Unknown')
    health = status.get('health', {}).get('status', 'Unknown')
    print(f'{name}|{sync}|{health}')
" 2>/dev/null)

if [[ -z "$APP_DATA" ]]; then
  fail "No ArgoCD applications found"
  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  ArgoCD Sync/Healthy Test Results"
  echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
  echo "  STATUS: NO APPLICATIONS FOUND"
  echo "═══════════════════════════════════════════════════"
  exit 1
fi

ACTUAL_COUNT=$(echo "$APP_DATA" | grep -c . 2>/dev/null || echo 0)
log "Found $ACTUAL_COUNT ArgoCD applications (expected $EXPECTED_APP_COUNT)"

# Show full application status table
log ""
log "Application status:"
printf "  %-40s %-12s %s\n" "NAME" "SYNC" "HEALTH"
printf "  %-40s %-12s %s\n" "----" "----" "------"
echo "$APP_DATA" | sort | while IFS='|' read -r name sync health; do
  printf "  %-40s %-12s %s\n" "$name" "$sync" "$health"
done
log ""

# ── Test 5: Application count matches expected ───────────────────
log "=== Test 5: Application count ==="
if [[ "$ACTUAL_COUNT" -ge "$EXPECTED_APP_COUNT" ]]; then
  pass "Application count: $ACTUAL_COUNT (expected >= $EXPECTED_APP_COUNT)"
else
  fail "Application count: $ACTUAL_COUNT (expected >= $EXPECTED_APP_COUNT)"
fi

# ── Test 6: All expected applications exist ──────────────────────
log ""
log "=== Test 6: Expected application existence ==="
ACTUAL_NAMES=$(echo "$APP_DATA" | cut -d'|' -f1 | sort)

for app_name in $ALL_EXPECTED_APPS; do
  if echo "$ACTUAL_NAMES" | grep -qx "$app_name"; then
    pass "$app_name: exists"
  else
    fail "$app_name: MISSING from ArgoCD"
  fi
done

# ── Test 7: All applications Synced ──────────────────────────────
log ""
log "=== Test 7: All applications report Synced ==="
SYNCED_COUNT=0
NOT_SYNCED=""

while IFS='|' read -r name sync health; do
  if [[ "$sync" == "Synced" ]]; then
    ((SYNCED_COUNT++))
    pass "$name: Synced"
  else
    NOT_SYNCED="$NOT_SYNCED $name($sync)"
    fail "$name: sync status is '$sync' (expected 'Synced')"
  fi
done <<< "$APP_DATA"

log "Synced: $SYNCED_COUNT/$ACTUAL_COUNT"
if [[ -n "$NOT_SYNCED" ]]; then
  log "Not synced:$NOT_SYNCED"
fi

# ── Test 8: All applications Healthy ─────────────────────────────
log ""
log "=== Test 8: All applications report Healthy ==="
HEALTHY_COUNT=0
NOT_HEALTHY=""

while IFS='|' read -r name sync health; do
  if [[ "$health" == "Healthy" ]]; then
    ((HEALTHY_COUNT++))
    pass "$name: Healthy"
  else
    NOT_HEALTHY="$NOT_HEALTHY $name($health)"
    fail "$name: health status is '$health' (expected 'Healthy')"
  fi
done <<< "$APP_DATA"

log "Healthy: $HEALTHY_COUNT/$ACTUAL_COUNT"
if [[ -n "$NOT_HEALTHY" ]]; then
  log "Not healthy:$NOT_HEALTHY"
fi

# ── Test 9: Hub-spoke validation — ArgoCD runs on tower only ─────
log ""
log "=== Test 9: ArgoCD hub-spoke validation ==="
# Verify argocd namespace has running pods on tower
ARGOCD_PODS=$(timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$WORKING_KC" \
  --request-timeout="${KUBECTL_TIMEOUT}s" \
  -n argocd get pods --no-headers 2>/dev/null | grep -c "Running" || echo 0)

if [[ "$ARGOCD_PODS" -gt 0 ]]; then
  pass "ArgoCD has $ARGOCD_PODS running pods on tower cluster"
else
  fail "ArgoCD has no running pods on tower cluster"
fi

# Verify sandbox cluster is registered as remote
SANDBOX_CLUSTER=$(timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$WORKING_KC" \
  --request-timeout="${KUBECTL_TIMEOUT}s" \
  -n argocd get secret -l argocd.argoproj.io/secret-type=cluster --no-headers 2>/dev/null | grep -c "sandbox" || echo 0)

if [[ "$SANDBOX_CLUSTER" -gt 0 ]]; then
  pass "Sandbox cluster registered as remote in ArgoCD"
else
  skip "Sandbox cluster registration not verified (secret may not be labeled)"
fi

# ── Test 10: Sync wave ordering — lower waves should be healthy ──
log ""
log "=== Test 10: Sync wave dependency check ==="
# Wave 0 apps (cluster-config, argocd) must be healthy for higher waves to work
WAVE0_APPS="tower-cluster-config tower-argocd sandbox-cluster-config"
WAVE0_OK=true
for w0app in $WAVE0_APPS; do
  W0_STATUS=$(echo "$APP_DATA" | grep "^${w0app}|" | head -1)
  if [[ -z "$W0_STATUS" ]]; then
    skip "$w0app: not found (wave-0 check)"
    continue
  fi
  W0_SYNC=$(echo "$W0_STATUS" | cut -d'|' -f2)
  W0_HEALTH=$(echo "$W0_STATUS" | cut -d'|' -f3)
  if [[ "$W0_SYNC" == "Synced" && "$W0_HEALTH" == "Healthy" ]]; then
    pass "$w0app: wave-0 Synced/Healthy (prerequisite OK)"
  else
    fail "$w0app: wave-0 NOT Synced/Healthy ($W0_SYNC/$W0_HEALTH) — blocks higher waves"
    WAVE0_OK=false
  fi
done

if $WAVE0_OK; then
  log "Wave-0 prerequisites satisfied — higher waves can sync"
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════"
echo "  ArgoCD Sync/Healthy Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
echo "  Applications: $ACTUAL_COUNT total, $SYNCED_COUNT synced, $HEALTHY_COUNT healthy"
if [[ "$FAIL" -eq 0 ]]; then
  if [[ "$SYNCED_COUNT" -eq "$ACTUAL_COUNT" && "$HEALTHY_COUNT" -eq "$ACTUAL_COUNT" ]]; then
    echo "  STATUS: ALL APPLICATIONS SYNCED/HEALTHY ✓"
  else
    echo "  STATUS: PARTIAL — some apps not yet synced/healthy"
  fi
else
  echo "  STATUS: NOT ALL APPLICATIONS SYNCED/HEALTHY ✗"
fi
echo "═══════════════════════════════════════════════════"
# Exit 0 if only skips (no fails), exit 1 if any fail
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
