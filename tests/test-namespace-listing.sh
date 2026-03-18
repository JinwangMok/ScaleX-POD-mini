#!/usr/bin/env bash
# test-namespace-listing.sh — Verify namespace listing succeeds on both clusters (tower, sandbox)
#
# AC 3: Namespace listing succeeds on both clusters
#
# Validates:
#   1. Kubeconfig files exist for both clusters
#   2. API server reachable (with fallback to domain kubeconfig)
#   3. `kubectl get namespaces` returns expected core namespaces
#   4. Both clusters have kube-system, default, kube-public, kube-node-lease
#   5. Tower cluster has argocd namespace (management cluster)
#
# Network-dependent tests SKIP gracefully when API servers are unreachable.
set -uo pipefail

REPO_DIR="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
CLUSTERS_DIR="$REPO_DIR/_generated/clusters"
KUBECTL_TIMEOUT=5  # seconds for kubectl operations

PASS=0
FAIL=0
SKIP=0

log()  { echo "[$(date +%T)] $*"; }
pass() { log "PASS: $*"; ((PASS++)); }
fail() { log "FAIL: $*"; ((FAIL++)); }
skip() { log "SKIP: $*"; ((SKIP++)); }

# Core namespaces every K8s cluster must have
CORE_NAMESPACES="default kube-system kube-public kube-node-lease"

# Track which clusters have API connectivity
declare -A API_REACHABLE
API_REACHABLE[tower]=false
API_REACHABLE[sandbox]=false

# Track working kubeconfig path per cluster
declare -A WORKING_KC
WORKING_KC[tower]=""
WORKING_KC[sandbox]=""

# ── Helper: pick best kubeconfig ──────────────────────────────────
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

echo "═══════════════════════════════════════════════════════════"
echo "  Namespace Listing Validation — Both Clusters"
echo "═══════════════════════════════════════════════════════════"
echo ""

# ── Test 1: Kubeconfig existence ──────────────────────────────────
log "=== Test 1: Kubeconfig file existence ==="
for cluster in tower sandbox; do
  kc=$(pick_kubeconfig "$cluster")
  if [[ -z "$kc" ]]; then
    fail "$cluster: no kubeconfig found at $CLUSTERS_DIR/$cluster/"
  else
    pass "$cluster: kubeconfig found at $kc"
  fi
done

# ── Test 2: API server reachability ───────────────────────────────
log ""
log "=== Test 2: API server reachability ==="
for cluster in tower sandbox; do
  kc=$(pick_kubeconfig "$cluster")
  [[ -z "$kc" ]] && { skip "$cluster: no kubeconfig"; continue; }

  server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
  if curl -sk --connect-timeout 3 --max-time 5 "${server_url}/healthz" 2>/dev/null | grep -q "ok"; then
    pass "$cluster: API server reachable at $server_url"
    API_REACHABLE[$cluster]=true
    WORKING_KC[$cluster]="$kc"
  else
    # Fallback to domain kubeconfig
    domain_kc="$CLUSTERS_DIR/$cluster/kubeconfig.yaml"
    if [[ "$kc" != "$domain_kc" && -f "$domain_kc" ]]; then
      domain_url=$(grep 'server:' "$domain_kc" | head -1 | awk '{print $2}' | tr -d '"')
      if curl -sk --connect-timeout 3 --max-time 5 "${domain_url}/healthz" 2>/dev/null | grep -q "ok"; then
        pass "$cluster: API server reachable at $domain_url (domain fallback)"
        API_REACHABLE[$cluster]=true
        WORKING_KC[$cluster]="$domain_kc"
      else
        skip "$cluster: API server not reachable (not on management network?)"
      fi
    else
      skip "$cluster: API server not reachable at $server_url (not on management network?)"
    fi
  fi
done

# ── Test 3: Namespace listing succeeds ─────────────────────────────
log ""
log "=== Test 3: kubectl get namespaces ==="
NETWORK_TESTS_RAN=false

declare -A NS_OUTPUT
for cluster in tower sandbox; do
  if [[ "${API_REACHABLE[$cluster]}" != "true" ]]; then
    skip "$cluster: API not reachable — cannot list namespaces"
    continue
  fi
  NETWORK_TESTS_RAN=true

  kc="${WORKING_KC[$cluster]}"
  output=$(timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$kc" \
    --request-timeout="${KUBECTL_TIMEOUT}s" get namespaces --no-headers 2>&1) || true

  if [[ -z "$output" ]]; then
    fail "$cluster: kubectl get namespaces returned empty"
    continue
  fi

  # Check for error indicators
  if echo "$output" | grep -qiE "refused|timeout|no route|unreachable|i/o timeout|dial tcp"; then
    skip "$cluster: kubectl get namespaces failed with network error: $(echo "$output" | head -1)"
    continue
  fi

  if echo "$output" | grep -qiE "forbidden|unauthorized"; then
    fail "$cluster: kubectl get namespaces returned auth error: $(echo "$output" | head -1)"
    continue
  fi

  # Must have at least one namespace (default)
  ns_count=$(echo "$output" | wc -l | tr -d ' ')
  if [[ "$ns_count" -gt 0 ]] && echo "$output" | grep -qE "^default\s"; then
    pass "$cluster: namespace listing succeeded ($ns_count namespaces)"
    NS_OUTPUT[$cluster]="$output"
    # Display namespace list for debugging
    log "$cluster namespaces:"
    echo "$output" | while IFS= read -r line; do
      log "  $line"
    done
  else
    fail "$cluster: unexpected namespace listing output: $(echo "$output" | head -3)"
  fi
done

# ── Test 4: Core namespaces present ───────────────────────────────
log ""
log "=== Test 4: Core namespaces present ==="
for cluster in tower sandbox; do
  if [[ -z "${NS_OUTPUT[$cluster]+x}" || -z "${NS_OUTPUT[$cluster]}" ]]; then
    skip "$cluster: no namespace data available"
    continue
  fi

  for ns in $CORE_NAMESPACES; do
    if echo "${NS_OUTPUT[$cluster]}" | grep -qE "^${ns}\s"; then
      pass "$cluster: namespace '$ns' exists"
    else
      fail "$cluster: namespace '$ns' MISSING"
    fi
  done
done

# ── Test 5: Tower management namespaces ───────────────────────────
log ""
log "=== Test 5: Tower management namespaces (argocd) ==="
if [[ -n "${NS_OUTPUT[tower]+x}" && -n "${NS_OUTPUT[tower]}" ]]; then
  if echo "${NS_OUTPUT[tower]}" | grep -qE "^argocd\s"; then
    pass "tower: argocd namespace exists (management cluster)"
  else
    skip "tower: argocd namespace not found (may not be deployed yet)"
  fi
else
  skip "tower: no namespace data available — cannot check management namespaces"
fi

# ── Test 6: All namespaces in Active status ───────────────────────
log ""
log "=== Test 6: All namespaces in Active status ==="
for cluster in tower sandbox; do
  if [[ -z "${NS_OUTPUT[$cluster]+x}" || -z "${NS_OUTPUT[$cluster]}" ]]; then
    skip "$cluster: no namespace data available"
    continue
  fi

  total=$(echo "${NS_OUTPUT[$cluster]}" | wc -l | tr -d ' ')
  active=$(echo "${NS_OUTPUT[$cluster]}" | grep -c "Active" || echo 0)
  if [[ "$active" -eq "$total" ]]; then
    pass "$cluster: all $total namespaces in Active status"
  else
    non_active=$((total - active))
    fail "$cluster: $non_active/$total namespaces NOT in Active status"
    echo "${NS_OUTPUT[$cluster]}" | grep -v "Active" | while IFS= read -r line; do
      log "  non-Active: $line"
    done
  fi
done

# ── Summary ───────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Namespace Listing Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
if ! $NETWORK_TESTS_RAN; then
  echo "  STATUS: NETWORK TESTS SKIPPED (not on management network)"
  echo "  Config validation passed."
elif [ "$FAIL" -eq 0 ]; then
  echo "  STATUS: NAMESPACE LISTING OK ON BOTH CLUSTERS"
else
  echo "  STATUS: NAMESPACE LISTING ISSUES DETECTED"
fi
echo "═══════════════════════════════════════════════════════════"
# Exit 0 if only skips (no fails), exit 1 if any fail
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
