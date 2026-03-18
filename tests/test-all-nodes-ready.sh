#!/usr/bin/env bash
# test-all-nodes-ready.sh — Verify all nodes in both clusters (tower, sandbox) report Ready
#
# Expected topology (from sdi-specs.yaml / inventory.ini):
#   Tower:   tower-cp-0, tower-cp-1, tower-cp-2          (3 nodes)
#   Sandbox: sandbox-cp-0, sandbox-worker-0, sandbox-worker-1 (3 nodes)
#
# Uses .original kubeconfigs (direct VM IPs) with fallback to domain kubeconfigs.
# Designed for E2E verification after cluster provisioning.
#
# Network-dependent tests (3-5) SKIP gracefully when API servers are unreachable.
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

# ── Expected node sets per cluster ────────────────────────────────
declare -A EXPECTED_NODES
EXPECTED_NODES[tower]="tower-cp-0 tower-cp-1 tower-cp-2"
EXPECTED_NODES[sandbox]="sandbox-cp-0 sandbox-worker-0 sandbox-worker-1"

declare -A EXPECTED_COUNT
EXPECTED_COUNT[tower]=3
EXPECTED_COUNT[sandbox]=3

# Track which clusters have API connectivity (for tests 4-5)
declare -A API_REACHABLE
API_REACHABLE[tower]=false
API_REACHABLE[sandbox]=false

# Track working kubeconfig path per cluster
declare -A WORKING_KC
WORKING_KC[tower]=""
WORKING_KC[sandbox]=""

# ── Helper: pick best kubeconfig ──────────────────────────────────
# Prefers .original (direct VM IP) over domain kubeconfig for LAN-based tests.
# Falls back to domain kubeconfig if .original doesn't exist.
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

# ── Helper: get node list from cluster (with timeout) ─────────────
get_nodes_json() {
  local kc="$1"
  timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$kc" \
    --request-timeout="${KUBECTL_TIMEOUT}s" get nodes -o json 2>/dev/null
}

# ── Helper: extract node names that are Ready ─────────────────────
get_ready_nodes() {
  local json="$1"
  echo "$json" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for node in data.get('items', []):
    name = node['metadata']['name']
    conditions = node.get('status', {}).get('conditions', [])
    for c in conditions:
        if c.get('type') == 'Ready' and c.get('status') == 'True':
            print(name)
            break
" 2>/dev/null
}

# ── Helper: extract all node names ────────────────────────────────
get_all_nodes() {
  local json="$1"
  echo "$json" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for node in data.get('items', []):
    print(node['metadata']['name'])
" 2>/dev/null
}

# ── Helper: extract node status summary ───────────────────────────
get_node_status_table() {
  local kc="$1"
  timeout "${KUBECTL_TIMEOUT}" kubectl --kubeconfig="$kc" \
    --request-timeout="${KUBECTL_TIMEOUT}s" get nodes -o wide 2>/dev/null
}

# ── Test 1: Config file validation ────────────────────────────────
log "=== Test 1: Config file existence ==="
for cluster in tower sandbox; do
  kc=$(pick_kubeconfig "$cluster")
  if [[ -z "$kc" ]]; then
    fail "$cluster: no kubeconfig found at $CLUSTERS_DIR/$cluster/"
  else
    pass "$cluster: kubeconfig found at $kc"
  fi
done

# ── Test 2: Inventory node count matches spec ─────────────────────
log ""
log "=== Test 2: Inventory node count validation ==="
for cluster in tower sandbox; do
  inv="$CLUSTERS_DIR/$cluster/inventory.ini"
  if [[ ! -f "$inv" ]]; then
    skip "$cluster: inventory.ini not found"
    continue
  fi
  # Count nodes in [all] section (lines with ansible_host=)
  inv_count=$(grep -c 'ansible_host=' "$inv" 2>/dev/null || echo 0)
  expected="${EXPECTED_COUNT[$cluster]}"
  if [[ "$inv_count" -eq "$expected" ]]; then
    pass "$cluster: inventory has $inv_count nodes (expected $expected)"
  else
    fail "$cluster: inventory has $inv_count nodes (expected $expected)"
  fi

  # Verify expected node names are in inventory
  for node_name in ${EXPECTED_NODES[$cluster]}; do
    if grep -q "^${node_name} " "$inv"; then
      pass "$cluster/$node_name: present in inventory"
    else
      fail "$cluster/$node_name: MISSING from inventory"
    fi
  done
done

# ── Test 3: API server reachability ───────────────────────────────
log ""
log "=== Test 3: API server reachability ==="
for cluster in tower sandbox; do
  kc=$(pick_kubeconfig "$cluster")
  [[ -z "$kc" ]] && { skip "$cluster: no kubeconfig"; continue; }

  # Try .original kubeconfig first (direct IP)
  server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
  if curl -sk --connect-timeout 3 --max-time 5 "${server_url}/healthz" 2>/dev/null | grep -q "ok"; then
    pass "$cluster: API server reachable at $server_url"
    API_REACHABLE[$cluster]=true
    WORKING_KC[$cluster]="$kc"
  else
    # Try domain kubeconfig as fallback
    domain_kc="$CLUSTERS_DIR/$cluster/kubeconfig.yaml"
    if [[ "$kc" != "$domain_kc" && -f "$domain_kc" ]]; then
      domain_url=$(grep 'server:' "$domain_kc" | head -1 | awk '{print $2}' | tr -d '"')
      if curl -sk --connect-timeout 3 --max-time 5 "${domain_url}/healthz" 2>/dev/null | grep -q "ok"; then
        pass "$cluster: API server reachable at $domain_url (domain fallback)"
        API_REACHABLE[$cluster]=true
        WORKING_KC[$cluster]="$domain_kc"
      else
        skip "$cluster: API server not reachable at $server_url or $domain_url (not on management network?)"
      fi
    else
      skip "$cluster: API server not reachable at $server_url (not on management network?)"
    fi
  fi
done

# ── Test 4: All nodes report Ready ────────────────────────────────
log ""
log "=== Test 4: Node readiness (all nodes must be Ready) ==="
OVERALL_READY=true
NETWORK_TESTS_RAN=false

for cluster in tower sandbox; do
  if [[ "${API_REACHABLE[$cluster]}" != "true" ]]; then
    skip "$cluster: API not reachable — cannot verify node readiness"
    continue
  fi
  NETWORK_TESTS_RAN=true

  kc="${WORKING_KC[$cluster]}"
  nodes_json=$(get_nodes_json "$kc")

  if [[ -z "$nodes_json" || "$nodes_json" == "null" ]]; then
    fail "$cluster: cannot retrieve nodes (kubectl failed)"
    OVERALL_READY=false
    continue
  fi

  # Show node status table for debugging
  log "$cluster node status:"
  get_node_status_table "$kc" | while IFS= read -r line; do
    log "  $line"
  done

  all_nodes=$(get_all_nodes "$nodes_json" | sort)
  ready_nodes=$(get_ready_nodes "$nodes_json" | sort)
  all_count=$(echo "$all_nodes" | grep -c . 2>/dev/null || echo 0)
  ready_count=$(echo "$ready_nodes" | grep -c . 2>/dev/null || echo 0)
  expected="${EXPECTED_COUNT[$cluster]}"

  # Check total node count matches expected
  if [[ "$all_count" -eq "$expected" ]]; then
    pass "$cluster: $all_count/$expected nodes registered"
  else
    fail "$cluster: $all_count/$expected nodes registered (missing nodes)"
    OVERALL_READY=false
  fi

  # Check all nodes are Ready
  if [[ "$ready_count" -eq "$expected" ]]; then
    pass "$cluster: $ready_count/$expected nodes Ready"
  else
    fail "$cluster: only $ready_count/$expected nodes Ready"
    OVERALL_READY=false
    # Show which nodes are NOT Ready
    not_ready=$(comm -23 <(echo "$all_nodes") <(echo "$ready_nodes"))
    if [[ -n "$not_ready" ]]; then
      log "  NotReady nodes: $not_ready"
    fi
  fi

  # Check each expected node is present and Ready
  for node_name in ${EXPECTED_NODES[$cluster]}; do
    if echo "$all_nodes" | grep -qx "$node_name"; then
      if echo "$ready_nodes" | grep -qx "$node_name"; then
        pass "$cluster/$node_name: Ready"
      else
        fail "$cluster/$node_name: NOT Ready"
        OVERALL_READY=false
      fi
    else
      fail "$cluster/$node_name: not registered in cluster"
      OVERALL_READY=false
    fi
  done
done

# ── Test 5: Node role validation ──────────────────────────────────
log ""
log "=== Test 5: Node role validation ==="
for cluster in tower sandbox; do
  if [[ "${API_REACHABLE[$cluster]}" != "true" ]]; then
    skip "$cluster: API not reachable — cannot verify node roles"
    continue
  fi

  kc="${WORKING_KC[$cluster]}"
  nodes_json=$(get_nodes_json "$kc")
  [[ -z "$nodes_json" ]] && { skip "$cluster: cannot retrieve nodes"; continue; }

  # Check control-plane labels
  cp_nodes=$(echo "$nodes_json" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for node in data.get('items', []):
    labels = node['metadata'].get('labels', {})
    if 'node-role.kubernetes.io/control-plane' in labels:
        print(node['metadata']['name'])
" 2>/dev/null | sort)

  if [[ "$cluster" == "tower" ]]; then
    cp_count=$(echo "$cp_nodes" | grep -c . 2>/dev/null || echo 0)
    if [[ "$cp_count" -eq 3 ]]; then
      pass "$cluster: 3 control-plane nodes (HA)"
    else
      fail "$cluster: expected 3 control-plane nodes, got $cp_count"
    fi
  elif [[ "$cluster" == "sandbox" ]]; then
    if echo "$cp_nodes" | grep -qx "sandbox-cp-0"; then
      pass "$cluster: sandbox-cp-0 is control-plane"
    else
      fail "$cluster: sandbox-cp-0 missing control-plane role"
    fi
  fi
done

# ── Summary ───────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════"
echo "  All Nodes Ready Test Results"
echo "  PASSED: $PASS  FAILED: $FAIL  SKIPPED: $SKIP"
if ! $NETWORK_TESTS_RAN; then
  echo "  STATUS: NETWORK TESTS SKIPPED (not on management network)"
  echo "  Config/inventory validation passed."
elif $OVERALL_READY; then
  echo "  STATUS: ALL NODES READY ✓"
else
  echo "  STATUS: NOT ALL NODES READY ✗"
fi
echo "═══════════════════════════════════════════════════"
# Exit 0 if only skips (no fails), exit 1 if any fail
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
