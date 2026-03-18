#!/usr/bin/env bash
# tests/test-tunnel-noninteractive.sh
#
# Sub-AC 2a: install.sh --auto initiates the tunnel (SSH or CF) non-interactively
#            and exits 0 after tunnel process is running in background.
#
# What this test verifies:
#   1. setup_api_tunnels() returns exit 0 after SSH bastion tunnel is established
#   2. The SSH tunnel PID is alive in the background after the function returns
#   3. No interactive prompts are issued (BatchMode=yes, no TTY required)
#   4. TUNNEL_STATE_FILE is written with transport_type, endpoint, auth_method
#   5. Idempotency: re-running setup_api_tunnels with an existing live tunnel
#      reuses it (returns 0, no new SSH process started)
#   6. CF Tunnel transport is selected when api_endpoint is configured + reachable
#      (returns 0, no SSH process started)
#   7. Transport selection is EXPLICIT — logged as TRANSPORT=ssh_bastion or
#      TRANSPORT=cf_tunnel, never silently inferred
#
# Design notes:
#   - Each test uses its own isolated SCALEX_HOME to prevent cross-test interference
#   - The watchdog background process (started by start_tunnel_watchdog) would hold
#     a pipe open if we use "| tee". Instead, we redirect the entire subshell to a
#     file with "> out 2>&1" — the subshell exits cleanly, while the watchdog process
#     is orphaned (inheriting the file FD) but doesn't block the outer test.
#   - Mock SSH binds the allocated local port via python3, enabling wait_for_tunnel_port
#     to confirm the tunnel is listening without needing a real network.
#   - MOCK_PID_FILE tracks python3 listener PIDs for cleanup.

set -uo pipefail

PASS=0
FAIL=0

INSTALL_SH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/install.sh"
TEST_TMPDIR=$(mktemp -d /tmp/scalex-tunnel-init-test.XXXXXX)

# MOCK_PID_FILE: mock ssh writes python3 listener PIDs here for cleanup
export MOCK_PID_FILE="$TEST_TMPDIR/mock_pids"
: > "$MOCK_PID_FILE"

cleanup() {
  # Kill python3 port-listener processes (orphaned by mock ssh exec)
  if [[ -s "$MOCK_PID_FILE" ]]; then
    while IFS= read -r pid; do
      [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    done < "$MOCK_PID_FILE"
  fi
  kill $(jobs -p) 2>/dev/null || true
  rm -rf "$TEST_TMPDIR"
}
trap cleanup EXIT

pass() { echo "PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }

# ─── Helper: extract a function body from install.sh using awk ──────────────
extract_func() {
  local func_name="$1"
  awk "/^${func_name}\(\)/{found=1; depth=0} found{print; for(i=1;i<=length(\$0);i++){c=substr(\$0,i,1); if(c==\"{\") depth++; if(c==\"}\") depth--}; if(found && depth==0 && NR>1){exit}}" "$INSTALL_SH"
}

# ─── Helper: load all tunnel functions into the current shell ─────────────
# Takes an optional base_dir for SCALEX_HOME (for test isolation).
load_tunnel_functions() {
  local base_dir="${1:-$TEST_TMPDIR/default}"

  # Stubs for logging / i18n
  i18n()      { echo "$1"; }
  log_info()  { echo "[INFO] $*" >&2; }
  log_warn()  { echo "[WARN] $*" >&2; }
  log_error() { echo "[ERROR] $*" >&2; }
  log_raw()   { :; }
  error_msg() { echo "[ERROR_MSG] $1" >&2; }
  mask_secrets() { cat; }

  # Per-test isolated directories
  SCALEX_HOME="${base_dir}/.scalex"
  INSTALLER_DIR="${SCALEX_HOME}/installer"
  TUNNEL_STATE_FILE="${SCALEX_HOME}/tunnel-state.yaml"
  API_TUNNEL_PIDS=()
  API_TUNNEL_BACKUPS=()
  TUNNEL_WATCHDOG_PID=""
  TUNNEL_CONF_DIR=""
  mkdir -p "$INSTALLER_DIR"

  # Load actual functions from install.sh
  eval "$(extract_func '_ssh_tunnel_start')"
  eval "$(extract_func 'wait_for_tunnel_port')"
  eval "$(extract_func 'validate_tunnel_conf')"
  eval "$(extract_func 'write_tunnel_config')"
  eval "$(extract_func 'start_tunnel_watchdog')"
  eval "$(extract_func 'stop_tunnel_watchdog')"
  eval "$(extract_func 'setup_api_tunnels')"
}

# ─── Helper: create a minimal fake repo directory ────────────────────────────
create_fake_repo() {
  local base="$1" cluster_name="$2" server_ip="$3" bastion_name="$4"
  local server_port="${5:-6443}"

  mkdir -p "$base/credentials" \
           "$base/_generated/clusters/$cluster_name" \
           "$base/config"

  # Kubeconfig pointing to the (unreachable) cluster API server
  cat > "$base/_generated/clusters/$cluster_name/kubeconfig.yaml" << EOF
apiVersion: v1
kind: Config
clusters:
- cluster:
    insecure-skip-tls-verify: true
    server: https://${server_ip}:${server_port}
  name: $cluster_name
contexts:
- context:
    cluster: $cluster_name
    user: admin
  name: $cluster_name
current-context: $cluster_name
users:
- name: admin
  user:
    client-certificate-data: dGVzdA==
    client-key-data: dGVzdA==
EOF
  chmod 600 "$base/_generated/clusters/$cluster_name/kubeconfig.yaml"

  # Password auth (no SSH key required → validate_tunnel_credentials passes)
  cat > "$base/credentials/.baremetal-init.yaml" << EOF
nodes:
  - name: "$bastion_name"
    ip: "${server_ip}"
    sshAuthMode: "password"
EOF
  echo 'PLAYBOX_0_PASSWORD="testpass"' > "$base/credentials/.env"
  chmod 600 "$base/credentials/.env"
}

# ─── Helper: create mock bin (fake ssh + curl) ───────────────────────────────
#
# mock ssh:
#   - Parses -L LOCAL_PORT:IP:PORT, starts a python3 TCP listener on LOCAL_PORT
#   - Then exec sleep 300 to stay alive (passes _ssh_tunnel_start stability check)
#   - Writes python3 PID to MOCK_PID_FILE (env var) for cleanup
#   - Fully non-interactive: no password prompts, no host-key checks
#
# mock curl:
#   - Returns 0 for localhost/127.0.0.1 URLs (tunnel API accessible)
#   - Returns 1 for all other URLs (forces SSH bastion, not direct/CF)
create_mock_bin() {
  local mock_bin="$1"
  mkdir -p "$mock_bin"

  cat > "$mock_bin/ssh" << 'ENDSSH'
#!/usr/bin/env bash
# Non-interactive mock SSH for tunnel tests.
LPORT=""
args=("$@")
i=0
while [[ $i -lt ${#args[@]} ]]; do
  if [[ "${args[$i]}" == "-L" ]] && [[ $((i+1)) -lt ${#args[@]} ]]; then
    LPORT="${args[$((i+1))]%%:*}"
    i=$((i+2))
    continue
  fi
  i=$((i+1))
done
if [[ -n "$LPORT" ]] && [[ "$LPORT" =~ ^[0-9]+$ ]]; then
  python3 - "$LPORT" << 'PYEOF' &
import socket, sys
port = int(sys.argv[1])
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.bind(('127.0.0.1', port))
    s.listen(128)
    while True:
        try:
            conn, _ = s.accept()
            conn.close()
        except Exception:
            pass
except Exception:
    pass
finally:
    try:
        s.close()
    except Exception:
        pass
PYEOF
  PY_PID=$!
  [[ -n "${MOCK_PID_FILE:-}" ]] && echo "$PY_PID" >> "$MOCK_PID_FILE"
  sleep 0.5
fi
exec sleep 300
ENDSSH
  chmod +x "$mock_bin/ssh"

  cat > "$mock_bin/curl" << 'ENDCURL'
#!/usr/bin/env bash
# Succeed for localhost (simulates API through tunnel), fail for others.
for arg in "$@"; do
  if [[ "$arg" == *"://localhost:"* ]] || [[ "$arg" == *"://127.0.0.1:"* ]]; then
    exit 0
  fi
done
exit 1
ENDCURL
  chmod +x "$mock_bin/curl"
}

# ─────────────────────────────────────────────────────────────────────────────
# Test 1: SSH bastion transport — non-interactive, exit 0, background PID alive
# ─────────────────────────────────────────────────────────────────────────────
echo "--- Test 1: SSH bastion — non-interactive start, exit 0, PID alive ---"

T1_BASE="$TEST_TMPDIR/t1"
T1_REPO="$T1_BASE/repo"
T1_MOCK_BIN="$T1_BASE/bin"
create_fake_repo "$T1_REPO" "testcluster" "10.99.0.1" "testbastion"
create_mock_bin "$T1_MOCK_BIN"

(
  export PATH="$T1_MOCK_BIN:$PATH"
  load_tunnel_functions "$T1_BASE"
  set +e
  setup_api_tunnels "$T1_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t1_out" 2>&1
T1_RC=$?

if [[ $T1_RC -eq 0 ]]; then
  pass "T1: setup_api_tunnels returns exit 0"
else
  fail "T1: setup_api_tunnels returned exit $T1_RC (expected 0)"
  cat "$TEST_TMPDIR/t1_out" >&2
fi

# Verify TUNNEL_STATE_FILE written with required fields
T1_STATE="$T1_BASE/.scalex/tunnel-state.yaml"
if [[ -f "$T1_STATE" ]]; then
  pass "T1: TUNNEL_STATE_FILE written"
else
  fail "T1: TUNNEL_STATE_FILE not written ($T1_STATE)"
fi

if [[ -f "$T1_STATE" ]] && grep -q 'ssh_bastion' "$T1_STATE"; then
  pass "T1: TUNNEL_STATE_FILE contains transport_type=ssh_bastion"
else
  fail "T1: TUNNEL_STATE_FILE missing transport_type=ssh_bastion"
  [[ -f "$T1_STATE" ]] && cat "$T1_STATE" >&2
fi

if [[ -f "$T1_STATE" ]] && grep -q 'localhost:' "$T1_STATE"; then
  pass "T1: TUNNEL_STATE_FILE contains localhost endpoint"
else
  fail "T1: TUNNEL_STATE_FILE missing localhost endpoint"
fi

if [[ -f "$T1_STATE" ]] && grep -q 'auth_method' "$T1_STATE"; then
  pass "T1: TUNNEL_STATE_FILE contains auth_method"
else
  fail "T1: TUNNEL_STATE_FILE missing auth_method"
fi

# Verify explicit transport selection log
if grep -q 'TRANSPORT=ssh_bastion' "$TEST_TMPDIR/t1_out" 2>/dev/null; then
  pass "T1: explicit TRANSPORT=ssh_bastion logged (not silently inferred)"
else
  fail "T1: TRANSPORT=ssh_bastion not logged — transport selection may be silent"
  grep -i 'transport\|bastion' "$TEST_TMPDIR/t1_out" >&2 || echo "(no transport log)" >&2
fi

# Verify tunnel conf file written with a live PID
T1_CONF_DIR="$T1_BASE/.scalex/installer/tunnels"
T1_CONF_FILE=$(ls "$T1_CONF_DIR"/*.conf 2>/dev/null | head -1)
if [[ -n "$T1_CONF_FILE" ]]; then
  pass "T1: tunnel conf file written"
  IFS=: read -r lp sip sp bt tpid < "$T1_CONF_FILE" 2>/dev/null || true
  if [[ "$lp" =~ ^[0-9]+$ ]] && [[ -n "$sip" ]] && [[ "$sp" =~ ^[0-9]+$ ]] && [[ -n "$bt" ]] && [[ "$tpid" =~ ^[0-9]+$ ]]; then
    pass "T1: conf file has all required fields (port=$lp target=$sip:$sp bastion=$bt pid=$tpid)"
    if kill -0 "$tpid" 2>/dev/null; then
      pass "T1: SSH tunnel PID $tpid is ALIVE in background — non-interactive start confirmed"
    else
      fail "T1: SSH tunnel PID $tpid is NOT alive (tunnel process died)"
    fi
  else
    fail "T1: conf file missing/malformed fields: '$lp' '$sip' '$sp' '$bt' '$tpid'"
  fi
else
  fail "T1: no tunnel conf file in $T1_CONF_DIR"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 2: CF Tunnel transport — exit 0, no SSH started, explicit TRANSPORT log
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 2: CF Tunnel — exit 0, no SSH, TRANSPORT=cf_tunnel logged ---"

T2_BASE="$TEST_TMPDIR/t2"
T2_REPO="$T2_BASE/repo"
T2_MOCK_BIN="$T2_BASE/bin"
create_fake_repo "$T2_REPO" "cfcluster" "10.99.0.2" "cfbastion"
mkdir -p "$T2_MOCK_BIN"

# curl succeeds for all URLs → CF Tunnel api_endpoint reachable → no SSH needed
cat > "$T2_MOCK_BIN/curl" << 'ENDCURL2'
#!/usr/bin/env bash
exit 0
ENDCURL2
chmod +x "$T2_MOCK_BIN/curl"

# ssh should NOT be called in this scenario
SSH_CALLED_FILE="$TEST_TMPDIR/t2_ssh_called"
cat > "$T2_MOCK_BIN/ssh" << ENDSSH2
#!/usr/bin/env bash
echo "UNEXPECTED_SSH_CALL" >> "$SSH_CALLED_FILE"
exit 1
ENDSSH2
chmod +x "$T2_MOCK_BIN/ssh"

# Add api_endpoint so CF Tunnel transport is selected
cat > "$T2_REPO/config/k8s-clusters.yaml" << 'EOF'
config:
  clusters:
    - cluster_name: "cfcluster"
      api_endpoint: "https://api.cfcluster.example.com"
EOF

(
  export PATH="$T2_MOCK_BIN:$PATH"
  load_tunnel_functions "$T2_BASE"
  set +e
  setup_api_tunnels "$T2_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t2_out" 2>&1
T2_RC=$?

if [[ $T2_RC -eq 0 ]]; then
  pass "T2: setup_api_tunnels returns exit 0 (CF Tunnel transport)"
else
  fail "T2: setup_api_tunnels returned exit $T2_RC (expected 0)"
  cat "$TEST_TMPDIR/t2_out" >&2
fi

if grep -q 'TRANSPORT=cf_tunnel' "$TEST_TMPDIR/t2_out" 2>/dev/null; then
  pass "T2: explicit TRANSPORT=cf_tunnel logged"
else
  fail "T2: TRANSPORT=cf_tunnel not logged"
  cat "$TEST_TMPDIR/t2_out" >&2
fi

if [[ ! -f "$SSH_CALLED_FILE" ]]; then
  pass "T2: no SSH process started (CF Tunnel is the transport)"
else
  fail "T2: SSH was unexpectedly called when CF Tunnel should be used"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 3: Idempotency — existing live tunnel is reused (no new SSH on re-run)
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 3: Idempotency — existing live tunnel reused on re-run ---"

T3_BASE="$TEST_TMPDIR/t3"
T3_REPO="$T3_BASE/repo"
T3_MOCK_BIN="$T3_BASE/bin"
SSH_CALL_COUNT="$TEST_TMPDIR/t3_ssh_count"
: > "$SSH_CALL_COUNT"
create_fake_repo "$T3_REPO" "idcluster" "10.99.0.3" "idbastion"

mkdir -p "$T3_MOCK_BIN"
# Use a call-counting mock ssh
cat > "$T3_MOCK_BIN/ssh" << ENDSSH3
#!/usr/bin/env bash
echo "called" >> "$SSH_CALL_COUNT"
LPORT=""
args=("\$@")
i=0
while [[ \$i -lt \${#args[@]} ]]; do
  if [[ "\${args[\$i]}" == "-L" ]] && [[ \$((\$i+1)) -lt \${#args[@]} ]]; then
    LPORT="\${args[\$((\$i+1))]%%:*}"
    i=\$((\$i+2))
    continue
  fi
  i=\$((\$i+1))
done
if [[ -n "\$LPORT" ]] && [[ "\$LPORT" =~ ^[0-9]+\$ ]]; then
  python3 - "\$LPORT" << 'PYEOF' &
import socket, sys
port = int(sys.argv[1])
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.bind(('127.0.0.1', port))
    s.listen(128)
    while True:
        try: conn, _ = s.accept(); conn.close()
        except: pass
except: pass
finally:
    try: s.close()
    except: pass
PYEOF
  PY_PID=\$!
  [[ -n "\${MOCK_PID_FILE:-}" ]] && echo "\$PY_PID" >> "\$MOCK_PID_FILE"
  sleep 0.5
fi
exec sleep 300
ENDSSH3
chmod +x "$T3_MOCK_BIN/ssh"

cat > "$T3_MOCK_BIN/curl" << 'ENDCURL3'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == *"://localhost:"* ]] || [[ "$arg" == *"://127.0.0.1:"* ]]; then exit 0; fi
done
exit 1
ENDCURL3
chmod +x "$T3_MOCK_BIN/curl"

# First run: establish tunnel
(
  export PATH="$T3_MOCK_BIN:$PATH"
  load_tunnel_functions "$T3_BASE"
  set +e
  setup_api_tunnels "$T3_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t3_first_out" 2>&1
T3_FIRST_RC=$?

if [[ $T3_FIRST_RC -eq 0 ]]; then
  pass "T3: first run (establish tunnel) returns exit 0"
else
  fail "T3: first run returned exit $T3_FIRST_RC (expected 0)"
  cat "$TEST_TMPDIR/t3_first_out" >&2
fi

SSH_CALLS_AFTER_FIRST=$(wc -l < "$SSH_CALL_COUNT" 2>/dev/null | tr -d ' ')
echo "  (SSH calls on first run: $SSH_CALLS_AFTER_FIRST)"

# Second run: should reuse existing tunnel
(
  export PATH="$T3_MOCK_BIN:$PATH"
  load_tunnel_functions "$T3_BASE"
  # Restore TUNNEL_CONF_DIR so second run finds existing conf
  TUNNEL_CONF_DIR="$T3_BASE/.scalex/installer/tunnels"
  set +e
  setup_api_tunnels "$T3_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t3_second_out" 2>&1
T3_SECOND_RC=$?

if [[ $T3_SECOND_RC -eq 0 ]]; then
  pass "T3: second run (idempotent re-run) returns exit 0"
else
  fail "T3: second run returned exit $T3_SECOND_RC (expected 0)"
  cat "$TEST_TMPDIR/t3_second_out" >&2
fi

SSH_CALLS_TOTAL=$(wc -l < "$SSH_CALL_COUNT" 2>/dev/null | tr -d ' ')
SSH_CALLS_SECOND=$((SSH_CALLS_TOTAL - SSH_CALLS_AFTER_FIRST))
if [[ "$SSH_CALLS_SECOND" -eq 0 ]]; then
  pass "T3: no new SSH calls on re-run (existing tunnel reused)"
else
  fail "T3: $SSH_CALLS_SECOND new SSH calls on re-run (expected 0)"
fi

if grep -qiE 'reuse|already running|already|재사용|이미' "$TEST_TMPDIR/t3_second_out" 2>/dev/null; then
  pass "T3: tunnel reuse is logged (idempotency confirmed)"
else
  fail "T3: no reuse log message found"
  head -10 "$TEST_TMPDIR/t3_second_out" >&2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 4: SSH bastion fallback — explicit log when CF api_endpoint unreachable
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 4: SSH bastion fallback (api_endpoint configured but unreachable) ---"

T4_BASE="$TEST_TMPDIR/t4"
T4_REPO="$T4_BASE/repo"
T4_MOCK_BIN="$T4_BASE/bin"
create_fake_repo "$T4_REPO" "fbcluster" "10.99.0.4" "fbbastion"
create_mock_bin "$T4_MOCK_BIN"  # curl fails for non-localhost

# api_endpoint configured but NOT reachable (mock curl fails non-localhost)
cat > "$T4_REPO/config/k8s-clusters.yaml" << 'EOF'
config:
  clusters:
    - cluster_name: "fbcluster"
      api_endpoint: "https://api.fbcluster.example.com"
EOF

(
  export PATH="$T4_MOCK_BIN:$PATH"
  load_tunnel_functions "$T4_BASE"
  set +e
  setup_api_tunnels "$T4_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t4_out" 2>&1
T4_RC=$?

if [[ $T4_RC -eq 0 ]]; then
  pass "T4: returns exit 0 (SSH bastion fallback when CF unreachable)"
else
  fail "T4: returned exit $T4_RC (expected 0)"
  cat "$TEST_TMPDIR/t4_out" >&2
fi

if grep -q 'TRANSPORT=ssh_bastion' "$TEST_TMPDIR/t4_out" 2>/dev/null; then
  pass "T4: TRANSPORT=ssh_bastion explicitly logged (fallback not silent)"
else
  fail "T4: TRANSPORT=ssh_bastion not logged"
  grep -i 'transport' "$TEST_TMPDIR/t4_out" >&2 || echo "(no transport log)" >&2
fi

T4_CONF_DIR="$T4_BASE/.scalex/installer/tunnels"
if compgen -G "$T4_CONF_DIR/*.conf" &>/dev/null; then
  pass "T4: SSH tunnel conf written (SSH bastion started)"
else
  fail "T4: no SSH conf file — SSH bastion not started"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 5: No clusters directory → graceful exit 0 (pre-cluster-init state)
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 5: No clusters dir → graceful exit 0 ---"

T5_BASE="$TEST_TMPDIR/t5"
T5_REPO="$T5_BASE/repo"
mkdir -p "$T5_REPO/credentials"
# No _generated/clusters — simulates state before cluster init

(
  load_tunnel_functions "$T5_BASE"
  set +e
  setup_api_tunnels "$T5_REPO"
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t5_out" 2>&1
T5_RC=$?

if [[ $T5_RC -eq 0 ]]; then
  pass "T5: no clusters dir → returns exit 0 (graceful)"
else
  fail "T5: no clusters dir → returned exit $T5_RC (should be graceful 0)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ $FAIL -eq 0 ]]
