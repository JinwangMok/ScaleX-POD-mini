#!/usr/bin/env bash
# tests/test-dash-resource-browse.sh
#
# Integration test: scalex dash TUI browses and displays resources
# (pods, services, deployments) within a selected namespace across clusters.
#
# Sub-AC 3 of AC 7: Verifies that the headless mode returns correct
# resource data scoped by namespace and cluster, which validates the
# same data pipeline used by the interactive TUI.
#
# What this test verifies:
#   1. --resource pods returns pod data with namespace fields across clusters
#   2. --resource deployments returns deployment data across clusters
#   3. --resource services returns service data across clusters
#   4. --namespace filter scopes resources to a specific namespace
#   5. Multi-cluster output contains resources from both clusters
#   6. Each resource retains its namespace field for TUI display

set -uo pipefail

PASS=0
FAIL=0
SKIP=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SCALEX="${PROJECT_ROOT}/scalex-cli/target/release/scalex"

TMPDIR_TEST=$(mktemp -d /tmp/scalex-dash-browse-test.XXXXXX)
MOCK_PIDS=()

cleanup() {
  for pid in "${MOCK_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  kill $(jobs -p) 2>/dev/null || true
  rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

pass() { echo "PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }
skip() { echo "SKIP: $1"; SKIP=$((SKIP+1)); }

# ── Pre-flight checks ─────────────────────────────────────────────────────────

if [[ ! -x "$SCALEX" ]]; then
  echo "SKIP: scalex binary not found at $SCALEX"
  echo "  Build with: cd scalex-cli && cargo build --release"
  exit 0
fi

if ! python3 --version &>/dev/null; then
  echo "SKIP: python3 not available"
  exit 0
fi

if ! "$SCALEX" dash --help 2>&1 | grep -q 'headless'; then
  echo "SKIP: scalex dash does not have --headless flag"
  exit 0
fi

# ── Helpers ────────────────────────────────────────────────────────────────────

free_port() {
  python3 -c "
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', 0))
print(s.getsockname()[1])
s.close()
"
}

write_cert() {
  local dir="$1"
  openssl req -x509 -newkey rsa:2048 \
    -keyout "$dir/key.pem" -out "$dir/cert.pem" \
    -days 1 -nodes -subj "/CN=localhost" \
    -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" 2>/dev/null \
  || openssl req -x509 -newkey rsa:2048 \
    -keyout "$dir/key.pem" -out "$dir/cert.pem" \
    -days 1 -nodes -subj "/CN=localhost" 2>/dev/null
}

write_kubeconfig() {
  local kc_path="$1" port="$2" cluster_name="$3"
  mkdir -p "$(dirname "$kc_path")"
  cat > "$kc_path" << EOF
apiVersion: v1
kind: Config
clusters:
- cluster:
    insecure-skip-tls-verify: true
    server: https://127.0.0.1:${port}
  name: ${cluster_name}
contexts:
- context:
    cluster: ${cluster_name}
    user: admin
  name: ${cluster_name}
current-context: ${cluster_name}
users:
- name: admin
  user:
    token: mock-token-for-testing
EOF
  chmod 600 "$kc_path"
}

# ── Mock K8s API server with rich namespace-scoped data ────────────────────────

write_rich_mock_server() {
  local script="$1" cluster_name="$2"
  cat > "$script" << PYEOF
#!/usr/bin/env python3
"""Mock K8s API with multi-namespace resources for browse testing."""
import sys, ssl, json
from http.server import HTTPServer, BaseHTTPRequestHandler

port = int(sys.argv[1])
cert = sys.argv[2]
key  = sys.argv[3]
cluster = "$cluster_name"

class Server(HTTPServer):
    def handle_error(self, request, client_address):
        exc = sys.exc_info()[1]
        if isinstance(exc, (BrokenPipeError, ConnectionResetError)):
            return
        import traceback; traceback.print_exc()

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args): pass

    def send_json(self, body, status=200):
        b = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        p = self.path.split("?")[0]

        if p == "/version":
            self.send_json({"gitVersion": "v1.33.1", "platform": "linux/amd64"})

        elif p == "/api/v1/namespaces":
            self.send_json({
                "apiVersion": "v1", "kind": "NamespaceList",
                "items": [
                    {"metadata": {"name": "default"}},
                    {"metadata": {"name": "kube-system"}},
                    {"metadata": {"name": "monitoring"}},
                ]
            })

        elif p == "/api/v1/nodes":
            self.send_json({
                "apiVersion": "v1", "kind": "NodeList",
                "items": [{
                    "metadata": {"name": f"{cluster}-node-0"},
                    "status": {
                        "conditions": [{"type": "Ready", "status": "True"}],
                        "capacity": {"cpu": "8", "memory": "16Gi"},
                        "allocatable": {"cpu": "8", "memory": "16Gi"},
                    }
                }]
            })

        elif p == "/api/v1/pods" or "/pods" in p:
            # Return pods in multiple namespaces
            pods = [
                {"metadata": {"name": f"nginx-{cluster}", "namespace": "default",
                              "creationTimestamp": "2026-01-01T00:00:00Z"},
                 "status": {"phase": "Running"},
                 "spec": {"containers": [{"name": "nginx"}]}},
                {"metadata": {"name": f"coredns-{cluster}", "namespace": "kube-system",
                              "creationTimestamp": "2026-01-01T00:00:00Z"},
                 "status": {"phase": "Running"},
                 "spec": {"containers": [{"name": "coredns"}]}},
                {"metadata": {"name": f"prometheus-{cluster}", "namespace": "monitoring",
                              "creationTimestamp": "2026-01-01T00:00:00Z"},
                 "status": {"phase": "Running"},
                 "spec": {"containers": [{"name": "prometheus"}]}},
            ]
            # Namespace filter from path
            for ns in ["default", "kube-system", "monitoring"]:
                if f"/namespaces/{ns}/pods" in p:
                    pods = [pod for pod in pods if pod["metadata"]["namespace"] == ns]
                    break
            self.send_json({"apiVersion": "v1", "kind": "PodList", "items": pods})

        elif "/deployments" in p:
            deploys = [
                {"metadata": {"name": "nginx", "namespace": "default"},
                 "spec": {"replicas": 1},
                 "status": {"readyReplicas": 1, "availableReplicas": 1}},
                {"metadata": {"name": "coredns", "namespace": "kube-system"},
                 "spec": {"replicas": 2},
                 "status": {"readyReplicas": 2, "availableReplicas": 2}},
                {"metadata": {"name": "prometheus", "namespace": "monitoring"},
                 "spec": {"replicas": 1},
                 "status": {"readyReplicas": 1, "availableReplicas": 1}},
            ]
            for ns in ["default", "kube-system", "monitoring"]:
                if f"/namespaces/{ns}/deployments" in p:
                    deploys = [d for d in deploys if d["metadata"]["namespace"] == ns]
                    break
            self.send_json({"apiVersion": "apps/v1", "kind": "DeploymentList", "items": deploys})

        elif "/services" in p:
            svcs = [
                {"metadata": {"name": "kubernetes", "namespace": "default"},
                 "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.1",
                          "ports": [{"port": 443}]}},
                {"metadata": {"name": "kube-dns", "namespace": "kube-system"},
                 "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.10",
                          "ports": [{"port": 53}]}},
                {"metadata": {"name": "prometheus-svc", "namespace": "monitoring"},
                 "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.20",
                          "ports": [{"port": 9090}]}},
            ]
            for ns in ["default", "kube-system", "monitoring"]:
                if f"/namespaces/{ns}/services" in p:
                    svcs = [s for s in svcs if s["metadata"]["namespace"] == ns]
                    break
            self.send_json({"apiVersion": "v1", "kind": "ServiceList", "items": svcs})

        elif "/configmaps" in p:
            self.send_json({"apiVersion": "v1", "kind": "ConfigMapList", "items": []})
        elif "/events" in p:
            self.send_json({"apiVersion": "v1", "kind": "EventList", "items": []})
        elif p == "/api":
            self.send_json({"apiVersion": "v1", "kind": "APIVersions", "versions": ["v1"],
                "serverAddressByClientCIDRs": [{"clientCIDR":"0.0.0.0/0","serverAddress":"127.0.0.1:6443"}]})
        elif p == "/apis":
            self.send_json({"apiVersion": "v1", "kind": "APIGroupList",
                "groups": [{"name": "apps", "versions": [{"groupVersion":"apps/v1","version":"v1"}],
                    "preferredVersion": {"groupVersion":"apps/v1","version":"v1"}}]})
        elif p == "/api/v1":
            self.send_json({"apiVersion": "v1", "kind": "APIResourceList", "groupVersion": "v1",
                "resources": [
                    {"name":"namespaces","singularName":"","namespaced":False,"kind":"Namespace","verbs":["get","list"]},
                    {"name":"nodes","singularName":"","namespaced":False,"kind":"Node","verbs":["get","list"]},
                    {"name":"pods","singularName":"","namespaced":True,"kind":"Pod","verbs":["get","list"]},
                    {"name":"services","singularName":"","namespaced":True,"kind":"Service","verbs":["get","list"]},
                    {"name":"configmaps","singularName":"","namespaced":True,"kind":"ConfigMap","verbs":["get","list"]},
                    {"name":"events","singularName":"","namespaced":True,"kind":"Event","verbs":["get","list"]},
                ]})
        elif p == "/apis/apps/v1":
            self.send_json({"apiVersion": "v1", "kind": "APIResourceList", "groupVersion": "apps/v1",
                "resources": [
                    {"name":"deployments","singularName":"","namespaced":True,"kind":"Deployment","verbs":["get","list"]},
                ]})
        else:
            self.send_json({"kind":"Status","status":"Failure","reason":"NotFound"}, 404)

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)
srv = Server(("127.0.0.1", port), Handler)
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
srv.serve_forever()
PYEOF
  chmod +x "$script"
}

wait_https_up() {
  local port="$1" max_secs="${2:-10}"
  local elapsed=0
  while [[ $elapsed -lt $max_secs ]]; do
    if python3 -c "
import socket, ssl, sys
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE
s = socket.create_connection(('127.0.0.1', $port), timeout=1)
ss = ctx.wrap_socket(s)
ss.close()
sys.exit(0)
" 2>/dev/null; then
      return 0
    fi
    sleep 0.5
    elapsed=$(( elapsed + 1 ))
  done
  return 1
}

# ── Setup: two mock clusters ──────────────────────────────────────────────────

CERT_DIR="$TMPDIR_TEST/certs"
mkdir -p "$CERT_DIR"
if ! write_cert "$CERT_DIR" 2>/dev/null; then
  echo "SKIP: openssl not available"
  exit 0
fi
if [[ ! -f "$CERT_DIR/cert.pem" || ! -f "$CERT_DIR/key.pem" ]]; then
  echo "SKIP: failed to generate certs"
  exit 0
fi

PORT_TOWER=$(free_port)
PORT_SANDBOX=$(free_port)
KC_DIR="$TMPDIR_TEST/clusters"

write_kubeconfig "$KC_DIR/tower/kubeconfig.yaml" "$PORT_TOWER" "tower"
write_kubeconfig "$KC_DIR/sandbox/kubeconfig.yaml" "$PORT_SANDBOX" "sandbox"

TOWER_SCRIPT="$TMPDIR_TEST/mock_tower.py"
SANDBOX_SCRIPT="$TMPDIR_TEST/mock_sandbox.py"
write_rich_mock_server "$TOWER_SCRIPT" "tower"
write_rich_mock_server "$SANDBOX_SCRIPT" "sandbox"

python3 "$TOWER_SCRIPT" "$PORT_TOWER" "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" &
MOCK_PIDS+=($!)
python3 "$SANDBOX_SCRIPT" "$PORT_SANDBOX" "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" &
MOCK_PIDS+=($!)

wait_https_up "$PORT_TOWER" 10 || { echo "FAIL: tower mock did not start"; exit 1; }
wait_https_up "$PORT_SANDBOX" 10 || { echo "FAIL: sandbox mock did not start"; exit 1; }
echo "Mock K8s servers ready (tower:$PORT_TOWER, sandbox:$PORT_SANDBOX)"

# ─────────────────────────────────────────────────────────────────────────────
# Test 1: --resource pods returns pod data across both clusters
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 1: scalex dash --headless --resource pods — cross-cluster pods ---"

T1_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" --resource pods 2>/dev/null) && T1_EXIT=0 || T1_EXIT=$?

if [[ $T1_EXIT -eq 0 ]]; then
  pass "T1: exits 0"
else
  fail "T1: exit $T1_EXIT"
fi

if echo "$T1_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
assert len(clusters) == 2, f'expected 2 clusters, got {len(clusters)}'
for c in clusters:
    pods = c.get('pods', [])
    assert len(pods) >= 3, f'cluster {c[\"cluster\"]} has {len(pods)} pods, expected >= 3'
    # Each pod must have a namespace field
    for pod in pods:
        assert 'namespace' in pod, f'pod {pod.get(\"name\")} missing namespace'
    # Check namespace diversity
    ns_set = set(p['namespace'] for p in pods)
    assert len(ns_set) >= 2, f'cluster {c[\"cluster\"]} pods only in {ns_set}'
sys.exit(0)
" 2>/dev/null; then
  pass "T1: both clusters have pods across multiple namespaces"
else
  fail "T1: pod data validation failed — got: $(echo "$T1_OUT" | head -5)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 2: --resource deployments returns deployment data across clusters
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 2: scalex dash --headless --resource deployments — cross-cluster deploys ---"

T2_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" --resource deployments 2>/dev/null) && T2_EXIT=0 || T2_EXIT=$?

if [[ $T2_EXIT -eq 0 ]]; then
  pass "T2: exits 0"
else
  fail "T2: exit $T2_EXIT"
fi

if echo "$T2_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
for c in clusters:
    deploys = c.get('deployments', [])
    assert len(deploys) >= 3, f'{c[\"cluster\"]} has {len(deploys)} deploys'
    names = [d['name'] for d in deploys]
    assert 'nginx' in names, 'missing nginx deployment'
    assert 'coredns' in names, 'missing coredns deployment'
    # Verify namespace field present
    for d in deploys:
        assert 'namespace' in d, f'deploy {d[\"name\"]} missing namespace'
sys.exit(0)
" 2>/dev/null; then
  pass "T2: deployments present across namespaces in both clusters"
else
  fail "T2: deployment data validation failed"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 3: --resource services returns service data across clusters
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 3: scalex dash --headless --resource services — cross-cluster services ---"

T3_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" --resource services 2>/dev/null) && T3_EXIT=0 || T3_EXIT=$?

if [[ $T3_EXIT -eq 0 ]]; then
  pass "T3: exits 0"
else
  fail "T3: exit $T3_EXIT"
fi

if echo "$T3_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
for c in clusters:
    svcs = c.get('services', [])
    assert len(svcs) >= 3, f'{c[\"cluster\"]} has {len(svcs)} services'
    # Services span namespaces
    ns_set = set(s['namespace'] for s in svcs)
    assert 'default' in ns_set, 'missing default ns service'
    assert 'kube-system' in ns_set, 'missing kube-system ns service'
sys.exit(0)
" 2>/dev/null; then
  pass "T3: services present across namespaces in both clusters"
else
  fail "T3: service data validation failed"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 4: --namespace filter scopes pods to specific namespace
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 4: scalex dash --headless --namespace kube-system --resource pods ---"

T4_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" \
  --namespace kube-system --resource pods 2>/dev/null) && T4_EXIT=0 || T4_EXIT=$?

if [[ $T4_EXIT -eq 0 ]]; then
  pass "T4: exits 0 with namespace filter"
else
  fail "T4: exit $T4_EXIT"
fi

if echo "$T4_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
for c in clusters:
    pods = c.get('pods', [])
    # With namespace filter, all pods should be in kube-system
    for pod in pods:
        ns = pod.get('namespace', '')
        assert ns == 'kube-system', f'pod {pod[\"name\"]} in namespace {ns}, expected kube-system'
    assert len(pods) >= 1, f'{c[\"cluster\"]} has no kube-system pods'
sys.exit(0)
" 2>/dev/null; then
  pass "T4: namespace filter correctly scopes pods to kube-system"
else
  fail "T4: namespace filter validation failed — got: $(echo "$T4_OUT" | head -5)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 5: Full output (no --resource) includes pods, deployments, services
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 5: scalex dash --headless — full output includes all resource types ---"

T5_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" 2>/dev/null) && T5_EXIT=0 || T5_EXIT=$?

if [[ $T5_EXIT -eq 0 ]]; then
  pass "T5: exits 0"
else
  fail "T5: exit $T5_EXIT"
fi

if echo "$T5_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
assert len(clusters) == 2, f'expected 2 clusters, got {len(clusters)}'
for c in clusters:
    assert 'pods' in c, f'{c[\"name\"]} missing pods'
    assert 'deployments' in c, f'{c[\"name\"]} missing deployments'
    assert 'services' in c, f'{c[\"name\"]} missing services'
    assert 'namespaces' in c, f'{c[\"name\"]} missing namespaces'
    assert len(c['namespaces']) >= 3, f'{c[\"name\"]} has {len(c[\"namespaces\"])} ns'
sys.exit(0)
" 2>/dev/null; then
  pass "T5: full output contains pods, deployments, services, and namespaces"
else
  fail "T5: full output validation failed"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 6: --cluster filter + --resource shows single cluster resources
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 6: scalex dash --headless --cluster tower --resource pods ---"

T6_OUT=$("$SCALEX" dash --headless --kubeconfig-dir "$KC_DIR" \
  --cluster tower --resource pods 2>/dev/null) && T6_EXIT=0 || T6_EXIT=$?

if [[ $T6_EXIT -eq 0 ]]; then
  pass "T6: exits 0 with cluster filter"
else
  fail "T6: exit $T6_EXIT"
fi

if echo "$T6_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data.get('clusters', [])
assert len(clusters) == 1, f'expected 1 cluster, got {len(clusters)}'
assert clusters[0]['cluster'] == 'tower', f'expected tower, got {clusters[0][\"cluster\"]}'
pods = clusters[0]['pods']
# tower pods should be named with 'tower' in mock
tower_names = [p['name'] for p in pods if 'tower' in p['name']]
assert len(tower_names) >= 1, f'no tower-named pods found'
sys.exit(0)
" 2>/dev/null; then
  pass "T6: cluster filter returns only tower cluster resources"
else
  fail "T6: cluster filter validation failed"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
[[ $FAIL -eq 0 ]]
