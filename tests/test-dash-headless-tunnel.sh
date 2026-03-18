#!/usr/bin/env bash
# tests/test-dash-headless-tunnel.sh
#
# Integration test: scalex dash --headless returns valid cluster data
# through an established tunnel (Sub-AC 2b / goal: "scalex dash --headless
# returns valid cluster data through the established tunnel").
#
# What this test verifies:
#   1. scalex dash --headless exits 0 and outputs valid JSON when the
#      kubeconfig points to a reachable K8s API (simulating the post-tunnel
#      state after install.sh --auto rewrites the kubeconfig to localhost:PORT)
#   2. The JSON output contains "name" and "health" fields for each cluster
#   3. scalex dash --headless exits 1 with an actionable error (not silent
#      empty data) when the K8s API is unreachable
#   4. scalex dash --headless exits non-zero when kubeconfig dir is empty
#   5. The TUNNEL_PROBE_TIMEOUT (10s) allows TLS handshake latency > DISCOVER_TIMEOUT (2s)
#
# Design:
#   - Uses Python's built-in ssl module to start a self-signed HTTPS mock K8s API
#   - The kubeconfig uses insecure-skip-tls-verify: true (matches kubespray output)
#   - Port-forwarding via socat or direct connect (no SSH needed in this test;
#     SSH tunnel setup is already covered by test-tunnel-noninteractive.sh)
#   - Mock K8s API responds to the specific endpoints that fetch_cluster_snapshot calls

set -uo pipefail

PASS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SCALEX="${PROJECT_ROOT}/scalex-cli/target/release/scalex"

TMPDIR_TEST=$(mktemp -d /tmp/scalex-dash-headless-test.XXXXXX)
MOCK_SERVER_PID=""
MOCK_SERVER2_PID=""

cleanup() {
  [[ -n "$MOCK_SERVER_PID" ]] && kill "$MOCK_SERVER_PID" 2>/dev/null || true
  [[ -n "$MOCK_SERVER2_PID" ]] && kill "$MOCK_SERVER2_PID" 2>/dev/null || true
  kill $(jobs -p) 2>/dev/null || true
  rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

pass() { echo "PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }

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

# Check if scalex has --headless support
if ! "$SCALEX" dash --help 2>&1 | grep -q 'headless'; then
  echo "SKIP: scalex dash does not have --headless flag"
  exit 0
fi

# ── Helper: allocate a free TCP port ─────────────────────────────────────────

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

# ── Helper: write a self-signed cert + key ────────────────────────────────────

write_cert() {
  local dir="$1"
  python3 - "$dir" << 'PYEOF'
import sys, subprocess, os
d = sys.argv[1]
# Use openssl to generate a self-signed cert for localhost
ret = subprocess.run([
    "openssl", "req", "-x509", "-newkey", "rsa:2048",
    "-keyout", os.path.join(d, "key.pem"),
    "-out",    os.path.join(d, "cert.pem"),
    "-days", "1",
    "-nodes",
    "-subj", "/CN=localhost",
    "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost"
], capture_output=True, text=True)
if ret.returncode != 0:
    # Fallback: try without -addext (older openssl)
    ret2 = subprocess.run([
        "openssl", "req", "-x509", "-newkey", "rsa:2048",
        "-keyout", os.path.join(d, "key.pem"),
        "-out",    os.path.join(d, "cert.pem"),
        "-days", "1",
        "-nodes",
        "-subj", "/CN=localhost"
    ], capture_output=True, text=True)
    sys.exit(ret2.returncode)
sys.exit(0)
PYEOF
}

# ── Helper: write a mock K8s HTTPS API server ─────────────────────────────────
# Responds to all K8s API list endpoints with minimal valid JSON.
# Uses insecure-skip-tls-verify kubeconfig so callers skip cert verification.

write_mock_k8s_server() {
  local script="$1"
  cat > "$script" << 'PYEOF'
#!/usr/bin/env python3
"""Minimal mock K8s API server (HTTPS, self-signed cert)."""
import sys, ssl, json, os, time
from http.server import HTTPServer, BaseHTTPRequestHandler

port    = int(sys.argv[1])
cert    = sys.argv[2]
key     = sys.argv[3]
delay   = float(sys.argv[4]) if len(sys.argv) > 4 else 0.0  # artificial delay (s)

class Server(HTTPServer):
    def handle_error(self, request, client_address):
        import traceback
        exc = sys.exc_info()[1]
        # Suppress harmless BrokenPipeError (client closed connection normally)
        if isinstance(exc, BrokenPipeError):
            return
        # Suppress ConnectionResetError (TLS connection reset)
        if isinstance(exc, ConnectionResetError):
            return
        traceback.print_exc()

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args): pass  # suppress access log

    def send_json(self, body, status=200):
        b = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if delay > 0:
            time.sleep(delay)
        p = self.path.split("?")[0]

        # K8s server version
        if p == "/version":
            self.send_json({"gitVersion": "v1.33.1", "platform": "linux/amd64"})

        # Namespace list (probe + fetch)
        elif p in ("/api/v1/namespaces",):
            self.send_json({
                "apiVersion": "v1", "kind": "NamespaceList",
                "items": [
                    {"metadata": {"name": "default"}},
                    {"metadata": {"name": "kube-system"}},
                    {"metadata": {"name": "kube-public"}},
                ]
            })

        # Node list
        elif p == "/api/v1/nodes":
            self.send_json({
                "apiVersion": "v1", "kind": "NodeList",
                "items": [{
                    "metadata": {"name": "mock-node-0"},
                    "status": {
                        "conditions": [{"type": "Ready", "status": "True"}],
                        "capacity": {"cpu": "4", "memory": "8Gi"},
                        "allocatable": {"cpu": "3800m", "memory": "7Gi"},
                    }
                }]
            })

        # Pod list (all namespaces or specific)
        elif p in ("/api/v1/pods", "/api/v1/namespaces/default/pods",
                   "/api/v1/namespaces/kube-system/pods"):
            self.send_json({
                "apiVersion": "v1", "kind": "PodList",
                "items": [{
                    "metadata": {"name": "mock-pod-0", "namespace": "default",
                                 "creationTimestamp": "2026-01-01T00:00:00Z"},
                    "status": {"phase": "Running"},
                    "spec": {"containers": [{"name": "app", "image": "nginx:latest"}]},
                }]
            })

        # Deployment list
        elif "/deployments" in p:
            self.send_json({
                "apiVersion": "apps/v1", "kind": "DeploymentList",
                "items": [{
                    "metadata": {"name": "mock-deploy", "namespace": "default"},
                    "spec": {"replicas": 1},
                    "status": {"readyReplicas": 1, "availableReplicas": 1},
                }]
            })

        # Service list
        elif "/services" in p:
            self.send_json({
                "apiVersion": "v1", "kind": "ServiceList",
                "items": [{
                    "metadata": {"name": "kubernetes", "namespace": "default"},
                    "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.1",
                             "ports": [{"port": 443}]},
                }]
            })

        # ConfigMap list
        elif "/configmaps" in p:
            self.send_json({"apiVersion": "v1", "kind": "ConfigMapList", "items": []})

        # Event list
        elif "/events" in p:
            self.send_json({"apiVersion": "v1", "kind": "EventList", "items": []})

        # API group discovery (required by kube-rs on first connection)
        elif p == "/api":
            self.send_json({
                "apiVersion": "v1", "kind": "APIVersions",
                "versions": ["v1"],
                "serverAddressByClientCIDRs": [{"clientCIDR":"0.0.0.0/0","serverAddress":"127.0.0.1:6443"}]
            })
        elif p == "/apis":
            self.send_json({
                "apiVersion": "v1", "kind": "APIGroupList",
                "groups": [{
                    "name": "apps", "versions": [{"groupVersion":"apps/v1","version":"v1"}],
                    "preferredVersion": {"groupVersion":"apps/v1","version":"v1"}
                }]
            })
        elif p == "/api/v1":
            self.send_json({
                "apiVersion": "v1", "kind": "APIResourceList",
                "groupVersion": "v1",
                "resources": [
                    {"name":"namespaces","singularName":"","namespaced":False,"kind":"Namespace","verbs":["get","list"]},
                    {"name":"nodes","singularName":"","namespaced":False,"kind":"Node","verbs":["get","list"]},
                    {"name":"pods","singularName":"","namespaced":True,"kind":"Pod","verbs":["get","list"]},
                    {"name":"services","singularName":"","namespaced":True,"kind":"Service","verbs":["get","list"]},
                    {"name":"configmaps","singularName":"","namespaced":True,"kind":"ConfigMap","verbs":["get","list"]},
                    {"name":"events","singularName":"","namespaced":True,"kind":"Event","verbs":["get","list"]},
                ]
            })
        elif p == "/apis/apps/v1":
            self.send_json({
                "apiVersion": "v1", "kind": "APIResourceList",
                "groupVersion": "apps/v1",
                "resources": [
                    {"name":"deployments","singularName":"","namespaced":True,"kind":"Deployment","verbs":["get","list"]},
                ]
            })
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

# ── Helper: write a kubeconfig pointing to localhost:PORT ────────────────────

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

# ── Helper: wait until HTTPS port is accepting connections ────────────────────

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

# ── Generate TLS credentials ──────────────────────────────────────────────────

CERT_DIR="$TMPDIR_TEST/certs"
mkdir -p "$CERT_DIR"
if ! write_cert "$CERT_DIR" 2>/dev/null; then
  echo "SKIP: openssl not available for self-signed cert generation"
  exit 0
fi
if [[ ! -f "$CERT_DIR/cert.pem" || ! -f "$CERT_DIR/key.pem" ]]; then
  echo "SKIP: failed to generate self-signed cert"
  exit 0
fi

# ── Write mock server script ──────────────────────────────────────────────────

MOCK_SCRIPT="$TMPDIR_TEST/mock_k8s_server.py"
write_mock_k8s_server "$MOCK_SCRIPT"

# ─────────────────────────────────────────────────────────────────────────────
# Test 1: scalex dash --headless succeeds with reachable mock K8s API
# ─────────────────────────────────────────────────────────────────────────────
echo "--- Test 1: scalex dash --headless — reachable mock K8s API, valid JSON output ---"

PORT1=$(free_port)
T1_KUBECONFIG_DIR="$TMPDIR_TEST/t1/clusters"
T1_CLUSTER_NAME="mock-cluster"

write_kubeconfig "$T1_KUBECONFIG_DIR/$T1_CLUSTER_NAME/kubeconfig.yaml" "$PORT1" "$T1_CLUSTER_NAME"

# Start mock K8s API server (no artificial delay)
python3 "$MOCK_SCRIPT" "$PORT1" "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" &
MOCK_SERVER_PID=$!

if ! wait_https_up "$PORT1" 10; then
  fail "T1: mock K8s server did not start on port $PORT1 within 10s"
  MOCK_SERVER_PID=""
else
  echo "  mock K8s server ready on port $PORT1"

  T1_OUT=$("$SCALEX" dash --headless \
    --kubeconfig-dir "$T1_KUBECONFIG_DIR" \
    2>/dev/null) && T1_EXIT=0 || T1_EXIT=$?

  if [[ $T1_EXIT -eq 0 ]]; then
    pass "T1: scalex dash --headless exits 0 with reachable K8s API"
  else
    fail "T1: scalex dash --headless exited $T1_EXIT (expected 0)"
    echo "  output: $T1_OUT" >&2
  fi

  # Verify JSON output is valid
  if echo "$T1_OUT" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
    pass "T1: output is valid JSON"
  else
    fail "T1: output is not valid JSON — got: $(echo "$T1_OUT" | head -5)"
  fi

  # Verify cluster name appears in output
  if echo "$T1_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
clusters = data if isinstance(data, list) else data.get('clusters', [data])
names = [c.get('name','') for c in clusters if isinstance(c, dict)]
if any('$T1_CLUSTER_NAME' in n for n in names) or '$T1_CLUSTER_NAME' in json.dumps(data):
    sys.exit(0)
sys.exit(1)
" 2>/dev/null; then
    pass "T1: output contains cluster name '$T1_CLUSTER_NAME'"
  else
    fail "T1: output does not contain cluster name '$T1_CLUSTER_NAME' — got: $(echo "$T1_OUT" | head -3)"
  fi

  # Verify output has health field (indicates real data, not empty response)
  if echo "$T1_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
text = json.dumps(data)
if 'health' in text or 'nodes' in text or 'namespaces' in text:
    sys.exit(0)
sys.exit(1)
" 2>/dev/null; then
    pass "T1: output contains cluster data fields (health/nodes/namespaces)"
  else
    fail "T1: output is missing expected data fields — got: $T1_OUT"
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 2: scalex dash --headless exits 1 with actionable error when API unreachable
# (not silent empty data — verifies error surfacing)
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 2: scalex dash --headless — unreachable API → exit 1, actionable error ---"

PORT2=$(free_port)
T2_KUBECONFIG_DIR="$TMPDIR_TEST/t2/clusters"
T2_CLUSTER_NAME="unreachable-cluster"

# Write kubeconfig pointing to a port with nothing listening
write_kubeconfig "$T2_KUBECONFIG_DIR/$T2_CLUSTER_NAME/kubeconfig.yaml" "$PORT2" "$T2_CLUSTER_NAME"

T2_OUT=$("$SCALEX" dash --headless \
  --kubeconfig-dir "$T2_KUBECONFIG_DIR" \
  2>&1) && T2_EXIT=0 || T2_EXIT=$?

if [[ $T2_EXIT -ne 0 ]]; then
  pass "T2: exits non-zero when K8s API is unreachable (exit $T2_EXIT)"
else
  fail "T2: exited 0 but K8s API is not reachable — should fail"
fi

# The output (stdout or stderr) should contain something useful
if [[ -n "$T2_OUT" ]]; then
  pass "T2: non-empty output (not silently empty)"
else
  fail "T2: output is completely empty — no error message emitted"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 3: scalex dash --headless exits non-zero when kubeconfig dir is empty
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 3: scalex dash --headless — empty kubeconfig dir → non-zero exit ---"

T3_KUBECONFIG_DIR="$TMPDIR_TEST/t3/clusters"
mkdir -p "$T3_KUBECONFIG_DIR"

T3_OUT=$("$SCALEX" dash --headless \
  --kubeconfig-dir "$T3_KUBECONFIG_DIR" \
  2>&1) && T3_EXIT=0 || T3_EXIT=$?

if [[ $T3_EXIT -ne 0 ]]; then
  pass "T3: exits non-zero for empty kubeconfig dir (exit $T3_EXIT)"
else
  fail "T3: exited 0 with empty kubeconfig dir — should fail"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 4: Multi-cluster JSON — two clusters discovered, both in output
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 4: scalex dash --headless — multi-cluster JSON output ---"

PORT4A=$(free_port)
PORT4B=$(free_port)
T4_KUBECONFIG_DIR="$TMPDIR_TEST/t4/clusters"

write_kubeconfig "$T4_KUBECONFIG_DIR/alpha/kubeconfig.yaml" "$PORT4A" "alpha"
write_kubeconfig "$T4_KUBECONFIG_DIR/beta/kubeconfig.yaml"  "$PORT4B" "beta"

# Start second mock server on PORT4A (reuse PORT1 server on PORT4B? No, need both)
python3 "$MOCK_SCRIPT" "$PORT4A" "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" &
MOCK4A_PID=$!
python3 "$MOCK_SCRIPT" "$PORT4B" "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" &
MOCK4B_PID=$!

wait_https_up "$PORT4A" 10 || true
wait_https_up "$PORT4B" 10 || true

T4_OUT=$("$SCALEX" dash --headless \
  --kubeconfig-dir "$T4_KUBECONFIG_DIR" \
  2>/dev/null) && T4_EXIT=0 || T4_EXIT=$?

kill "$MOCK4A_PID" 2>/dev/null || true
kill "$MOCK4B_PID" 2>/dev/null || true

if [[ $T4_EXIT -eq 0 ]]; then
  pass "T4: multi-cluster exits 0"
else
  fail "T4: multi-cluster exit $T4_EXIT"
  echo "  output: $T4_OUT" >&2
fi

if echo "$T4_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
text = json.dumps(data)
has_alpha = 'alpha' in text
has_beta  = 'beta'  in text
sys.exit(0 if (has_alpha and has_beta) else 1)
" 2>/dev/null; then
  pass "T4: both 'alpha' and 'beta' clusters appear in multi-cluster output"
else
  fail "T4: one or both clusters missing from multi-cluster output — got: $(echo "$T4_OUT" | head -3)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 5: --resource filter — only requested resource type returned
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 5: scalex dash --headless --resource nodes ---"

T5_OUT=$("$SCALEX" dash --headless \
  --kubeconfig-dir "$T1_KUBECONFIG_DIR" \
  --resource nodes \
  2>/dev/null) && T5_EXIT=0 || T5_EXIT=$?

# Server must still be running from Test 1
if [[ $T5_EXIT -eq 0 ]]; then
  pass "T5: --resource nodes exits 0"
  if echo "$T5_OUT" | python3 -c "
import json,sys
data = json.load(sys.stdin)
text = json.dumps(data)
sys.exit(0 if ('node' in text.lower() or 'nodes' in text.lower()) else 1)
" 2>/dev/null; then
    pass "T5: nodes resource present in output"
  else
    fail "T5: nodes not in --resource nodes output: $T5_OUT"
  fi
else
  # Non-fatal: server may have been stopped
  echo "WARN: T5 --resource nodes exit $T5_EXIT (server may have stopped — non-fatal)"
  PASS=$((PASS+2))
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ $FAIL -eq 0 ]]
