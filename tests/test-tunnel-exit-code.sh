#!/usr/bin/env bash
# tests/test-tunnel-exit-code.sh
#
# Sub-AC 3: install.sh exits with code 0 on successful auto-tunnel setup,
#            and non-zero with clear error message on failure.
#
# What this test verifies:
#   1. setup_api_tunnels returns exit 0 when SSH tunnel establishes successfully
#   2. setup_api_tunnels returns non-zero (exit 1) when SSH always fails
#   3. A clear, human-readable error message is printed to stderr on failure
#      (What/Why/How format via error_msg, or at minimum an [ERROR] line)
#   4. phase_provision returns non-zero when setup_api_tunnels fails
#      (i.e., the error is not swallowed at the phase level)
#   5. main() --auto exits non-zero when phase_provision fails (the main fix)
#   6. main() --auto exits 0 when phase_provision succeeds
#
# Design:
#   - Functions are extracted from install.sh using awk (same pattern as other tests)
#   - Mock SSH either succeeds (python3 port listener + sleep 300) or always fails
#   - Mock curl fails for non-localhost URLs (forces SSH bastion path)
#   - Each test uses an isolated SCALEX_HOME to prevent cross-test interference

set -uo pipefail

PASS=0
FAIL=0

INSTALL_SH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/install.sh"
TEST_TMPDIR=$(mktemp -d /tmp/scalex-tunnel-exitcode-test.XXXXXX)

export MOCK_PID_FILE="$TEST_TMPDIR/mock_pids"
: > "$MOCK_PID_FILE"

cleanup() {
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

# ─── Helper: extract a function body from install.sh ─────────────────────────
extract_func() {
  local func_name="$1"
  awk "/^${func_name}\(\)/{found=1; depth=0} found{print; for(i=1;i<=length(\$0);i++){c=substr(\$0,i,1); if(c==\"{\") depth++; if(c==\"}\") depth--}; if(found && depth==0 && NR>1){exit}}" "$INSTALL_SH"
}

# ─── Helper: load all tunnel functions into the current shell ─────────────────
load_tunnel_functions() {
  local base_dir="${1:-$TEST_TMPDIR/default}"

  i18n()      { echo "$1"; }
  log_info()  { echo "[INFO] $*" >&2; }
  log_warn()  { echo "[WARN] $*" >&2; }
  log_error() { echo "[ERROR] $*" >&2; }
  log_raw()   { :; }
  error_msg() {
    local what="$1" why="${2:-}" how="${3:-}"
    echo "[ERROR_MSG] What: $what" >&2
    [[ -n "$why" ]] && echo "[ERROR_MSG] Why:  $why" >&2
    [[ -n "$how" ]] && echo "[ERROR_MSG] How:  $how" >&2
  }
  mask_secrets() { cat; }

  SCALEX_HOME="${base_dir}/.scalex"
  INSTALLER_DIR="${SCALEX_HOME}/installer"
  TUNNEL_STATE_FILE="${SCALEX_HOME}/tunnel-state.yaml"
  API_TUNNEL_PIDS=()
  API_TUNNEL_BACKUPS=()
  TUNNEL_WATCHDOG_PID=""
  TUNNEL_CONF_DIR=""
  mkdir -p "$INSTALLER_DIR"

  eval "$(extract_func '_ssh_tunnel_start')"
  eval "$(extract_func 'wait_for_tunnel_port')"
  eval "$(extract_func 'validate_tunnel_conf')"
  eval "$(extract_func 'write_tunnel_config')"
  eval "$(extract_func 'start_tunnel_watchdog')"
  eval "$(extract_func 'stop_tunnel_watchdog')"
  eval "$(extract_func 'setup_api_tunnels')"
}

# ─── Helper: common stubs used in multiple tests ─────────────────────────────
common_stubs() {
  # Logging
  i18n()              { echo "$1"; }
  log_info()          { echo "[INFO] $*" >&2; }
  log_warn()          { echo "[WARN] $*" >&2; }
  log_error()         { echo "[ERROR] $*" >&2; }
  log_phase()         { :; }
  log_raw()           { :; }
  error_msg() {
    local what="$1" why="${2:-}" how="${3:-}"
    echo "[ERROR_MSG] What: $what" >&2
    [[ -n "$why" ]] && echo "[ERROR_MSG] Why:  $why" >&2
    [[ -n "$how" ]] && echo "[ERROR_MSG] How:  $how" >&2
  }
  mask_secrets()     { cat; }
  # State
  state_set()        { :; }
  state_get()        { echo "${3:-}"; }
  state_get_phase()  { echo "-1"; }
  state_save_phase() { :; }
  # TUI
  detect_tui()       { TUI="fallback"; }
  tui_yesno()        { return 0; }
  tui_input()        { echo "${3:-}"; }
  # Other
  detect_os()        { echo "linux"; }
  show_dashboard()   { :; }
  post_install_summary() { :; }
  resume_check()     { :; }
  validate_tunnel_credentials() { return 0; }
  generate_ssh_config() { :; }
  ensure_sudo()      { return 0; }
  init_dirs()        { mkdir -p "$INSTALLER_DIR" "$LOG_DIR"; }
  parse_args() {
    for arg in "$@"; do
      [[ "$arg" == "--auto" ]] && AUTO_MODE="true"
    done
  }
  cleanup_handler()  { :; }
  # Color vars (referenced in phase_provision echo statements)
  RED='' GREEN='' YELLOW='' BLUE='' CYAN='' BOLD='' NC=''
}

# ─── Helper: create a minimal fake repo directory ────────────────────────────
create_fake_repo() {
  local base="$1" cluster_name="$2" server_ip="$3" bastion_name="$4"
  local server_port="${5:-6443}"

  mkdir -p "$base/credentials" \
           "$base/_generated/clusters/$cluster_name" \
           "$base/config"

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

  cat > "$base/credentials/.baremetal-init.yaml" << EOF
nodes:
  - name: "$bastion_name"
    ip: "${server_ip}"
    sshAuthMode: "password"
EOF
  echo 'PLAYBOX_0_PASSWORD="testpass"' > "$base/credentials/.env"
  chmod 600 "$base/credentials/.env"
}

# ─── Helper: create mock bin (successful ssh) ────────────────────────────────
create_mock_bin_success() {
  local mock_bin="$1"
  mkdir -p "$mock_bin"

  # Successful mock ssh: binds a TCP port (via python3), then exec sleep 300
  cat > "$mock_bin/ssh" << 'ENDSSH'
#!/usr/bin/env bash
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

  # curl: succeed for localhost (tunnel API accessible), fail for external URLs
  cat > "$mock_bin/curl" << 'ENDCURL'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == *"://localhost:"* ]] || [[ "$arg" == *"://127.0.0.1:"* ]]; then
    exit 0
  fi
done
exit 1
ENDCURL
  chmod +x "$mock_bin/curl"
}

# ─── Helper: create mock bin (always-failing ssh) ────────────────────────────
create_mock_bin_fail() {
  local mock_bin="$1"
  mkdir -p "$mock_bin"

  # Failing mock ssh: exits immediately with code 255 (simulates connection refused)
  cat > "$mock_bin/ssh" << 'ENDSSH'
#!/usr/bin/env bash
echo "ssh: connect to host bastion port 22: Connection refused" >&2
exit 255
ENDSSH
  chmod +x "$mock_bin/ssh"

  # curl always fails (no direct access, no CF tunnel)
  cat > "$mock_bin/curl" << 'ENDCURL'
#!/usr/bin/env bash
exit 1
ENDCURL
  chmod +x "$mock_bin/curl"
}

# ─────────────────────────────────────────────────────────────────────────────
# Test 1: setup_api_tunnels exits 0 on successful tunnel establishment
# ─────────────────────────────────────────────────────────────────────────────
echo "--- Test 1: setup_api_tunnels exits 0 on success ---"

T1_BASE="$TEST_TMPDIR/t1"
T1_REPO="$T1_BASE/repo"
T1_MOCK_BIN="$T1_BASE/bin"
create_fake_repo "$T1_REPO" "testcluster" "10.99.1.1" "testbastion"
create_mock_bin_success "$T1_MOCK_BIN"

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
  pass "T1: setup_api_tunnels exits 0 on success"
else
  fail "T1: setup_api_tunnels exited $T1_RC (expected 0)"
  cat "$TEST_TMPDIR/t1_out" >&2
fi

# Verify no ERROR messages on success path
if grep -q '\[ERROR\]\|\[ERROR_MSG\]' "$TEST_TMPDIR/t1_out" 2>/dev/null; then
  fail "T1: unexpected ERROR messages on success path"
  grep '\[ERROR\]\|\[ERROR_MSG\]' "$TEST_TMPDIR/t1_out" >&2
else
  pass "T1: no spurious error messages on success"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 2: setup_api_tunnels exits non-zero when SSH always fails
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 2: setup_api_tunnels exits non-zero when SSH always fails ---"

T2_BASE="$TEST_TMPDIR/t2"
T2_REPO="$T2_BASE/repo"
T2_MOCK_BIN="$T2_BASE/bin"
create_fake_repo "$T2_REPO" "failcluster" "10.99.2.2" "failbastion"
create_mock_bin_fail "$T2_MOCK_BIN"

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

if [[ $T2_RC -ne 0 ]]; then
  pass "T2: setup_api_tunnels exits non-zero ($T2_RC) when SSH fails"
else
  fail "T2: setup_api_tunnels returned exit 0 when SSH always fails (expected non-zero)"
  cat "$TEST_TMPDIR/t2_out" >&2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 3: Clear error message on SSH failure (What/Why/How or [ERROR] line)
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 3: clear error message on SSH failure ---"

# Reuse T2 output
if grep -qE '\[ERROR\]|\[ERROR_MSG\]' "$TEST_TMPDIR/t2_out" 2>/dev/null; then
  pass "T3: error message present on stderr when SSH fails"
else
  fail "T3: no [ERROR] or [ERROR_MSG] in output when SSH fails — error may be silent"
  cat "$TEST_TMPDIR/t2_out" >&2
fi

# Verify the error message contains actionable detail (mentions bastion, tunnel, or SSH)
if grep -qiE 'bastion|tunnel|ssh|failbastion' "$TEST_TMPDIR/t2_out" 2>/dev/null; then
  pass "T3: error message contains tunnel/bastion context"
else
  fail "T3: error message lacks tunnel/bastion context — not actionable"
  cat "$TEST_TMPDIR/t2_out" >&2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 4: phase_provision returns non-zero when setup_api_tunnels fails
# Verified by checking that the auto mode code path (if ! setup_api_tunnels; return 1)
# propagates setup_api_tunnels failure to the caller.
# We stub phase_provision directly and check the chain.
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 4: phase_provision propagates non-zero from setup_api_tunnels ---"

# Load phase_provision from install.sh with all required stubs.
# We inject a stub setup_api_tunnels that always fails,
# and stub all other commands to succeed.

T4_BASE="$TEST_TMPDIR/t4"
T4_REPO="$T4_BASE/repo"
T4_MOCK_BIN="$T4_BASE/bin"
create_fake_repo "$T4_REPO" "t4cluster" "10.99.4.4" "t4bastion"
mkdir -p "$T4_REPO/config"
echo 'config: {}' > "$T4_REPO/config/sdi-specs.yaml"
echo 'config: {}' > "$T4_REPO/config/k8s-clusters.yaml"

# mock bin: curl always fails, but we override setup_api_tunnels via stub below
create_mock_bin_fail "$T4_MOCK_BIN"

(
  export PATH="$T4_MOCK_BIN:$PATH"
  common_stubs

  SCALEX_HOME="$T4_BASE/.scalex"
  INSTALLER_DIR="$SCALEX_HOME/installer"
  LOG_DIR="$INSTALLER_DIR/logs"
  LOG_FILE="$LOG_DIR/install.log"
  REPO_DIR="$T4_REPO"
  AUTO_MODE="true"
  REPO_URL="https://example.com/repo.git"
  API_TUNNEL_PIDS=()
  API_TUNNEL_BACKUPS=()
  TUNNEL_CONF_DIR=""
  TUNNEL_STATE_FILE="$SCALEX_HOME/tunnel-state.yaml"
  mkdir -p "$LOG_DIR"
  : > "$LOG_FILE"

  # Stub scalex commands to succeed (not testing those here)
  scalex() { return 0; }

  # Stub setup_api_tunnels to always fail with clear error message
  setup_api_tunnels() {
    log_error "t4cluster: SSH tunnel failed after retries (localhost:16443 → 10.99.4.4:6443)"
    return 1
  }

  # Stub functions called by phase_provision that we don't need to test
  verify_api_tunnels_ready() { return 0; }
  start_tunnel_watchdog()    { :; }
  stop_tunnel_watchdog()     { :; }
  cleanup_api_tunnels()      { :; }
  write_tunnel_config()      { :; }

  eval "$(awk '/^phase_provision\(\)/{found=1; depth=0} found{print; for(i=1;i<=length($0);i++){c=substr($0,i,1); if(c=="{") depth++; if(c=="}") depth--}; if(found && depth==0 && NR>1){exit}}' "$INSTALL_SH")"

  set +e
  phase_provision
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t4_out" 2>&1
T4_RC=$?

if [[ $T4_RC -ne 0 ]]; then
  pass "T4: phase_provision exits non-zero ($T4_RC) when setup_api_tunnels fails"
else
  fail "T4: phase_provision returned exit 0 even though setup_api_tunnels failed (exit propagation broken)"
  cat "$TEST_TMPDIR/t4_out" >&2
fi

if grep -qE '\[ERROR\]|\[ERROR_MSG\]' "$TEST_TMPDIR/t4_out" 2>/dev/null; then
  pass "T4: phase_provision emits error message when tunnel setup fails"
else
  fail "T4: phase_provision did not emit error message on tunnel failure"
  cat "$TEST_TMPDIR/t4_out" >&2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 5: main() --auto exits non-zero when phase_provision fails
# This is the core test for the fix applied to install.sh main().
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 5: main() --auto exits non-zero when phase_provision fails ---"

T5_BASE="$TEST_TMPDIR/t5"
T5_REPO="$T5_BASE/repo"
create_fake_repo "$T5_REPO" "t5cluster" "10.99.5.5" "t5bastion"
mkdir -p "$T5_REPO/config"
echo 'config: {}' > "$T5_REPO/config/sdi-specs.yaml"
echo 'config: {}' > "$T5_REPO/config/k8s-clusters.yaml"

(
  set +u   # main() references many globals (VERSION, BOLD, etc.) — allow unbound
  common_stubs

  SCALEX_HOME="$T5_BASE/.scalex"
  INSTALLER_DIR="$SCALEX_HOME/installer"
  LOG_DIR="$INSTALLER_DIR/logs"
  LOG_FILE="$LOG_DIR/install-test.log"
  TUNNEL_STATE_FILE="$SCALEX_HOME/tunnel-state.yaml"
  REPO_URL="https://example.com/repo.git"
  VERSION="test"
  REPO_DIR=""
  NODE_COUNT=0
  POOL_COUNT=0
  CLUSTER_COUNT=0
  AUTO_MODE="false"
  SUDO_KEEPALIVE_PID=""
  API_TUNNEL_PIDS=()
  API_TUNNEL_BACKUPS=()
  TUNNEL_WATCHDOG_PID=""
  TUNNEL_CONF_DIR=""
  mkdir -p "$LOG_DIR"
  : > "$LOG_FILE"

  # phase_provision: fail immediately — simulates tunnel setup failure
  phase_provision() {
    log_error "Auto mode: API tunnel setup failed — aborting provisioning"
    return 1
  }
  phase_deps() { return 0; }

  eval "$(extract_func 'main')"

  set +e
  SCALEX_REPO_DIR="$T5_REPO" main --auto
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t5_out" 2>&1
T5_RC=$?

if [[ $T5_RC -ne 0 ]]; then
  pass "T5: main() --auto exits non-zero ($T5_RC) when phase_provision fails"
else
  fail "T5: main() --auto returned exit 0 even though phase_provision failed (exit not propagated)"
  cat "$TEST_TMPDIR/t5_out" >&2
fi

if grep -qE '\[ERROR\]|\[ERROR_MSG\]' "$TEST_TMPDIR/t5_out" 2>/dev/null; then
  pass "T5: main() --auto emits error message when provisioning fails"
else
  fail "T5: main() --auto did not emit error message on provisioning failure"
  cat "$TEST_TMPDIR/t5_out" >&2
fi

# The fix adds 'Auto mode: provisioning failed' via error_msg
if grep -qiE 'provisioning failed|Auto mode.*fail' "$TEST_TMPDIR/t5_out" 2>/dev/null; then
  pass "T5: 'Auto mode: provisioning failed' error message appears in output"
else
  fail "T5: 'Auto mode: provisioning failed' message not found"
  cat "$TEST_TMPDIR/t5_out" >&2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Test 6: main() --auto exits 0 when phase_provision succeeds
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "--- Test 6: main() --auto exits 0 when phase_provision succeeds ---"

T6_BASE="$TEST_TMPDIR/t6"
T6_REPO="$T6_BASE/repo"
create_fake_repo "$T6_REPO" "t6cluster" "10.99.6.6" "t6bastion"
mkdir -p "$T6_REPO/config"
echo 'config: {}' > "$T6_REPO/config/sdi-specs.yaml"
echo 'config: {}' > "$T6_REPO/config/k8s-clusters.yaml"

(
  set +u   # main() references many globals (VERSION, BOLD, etc.) — allow unbound
  common_stubs

  SCALEX_HOME="$T6_BASE/.scalex"
  INSTALLER_DIR="$SCALEX_HOME/installer"
  LOG_DIR="$INSTALLER_DIR/logs"
  LOG_FILE="$LOG_DIR/install-test.log"
  TUNNEL_STATE_FILE="$SCALEX_HOME/tunnel-state.yaml"
  REPO_URL="https://example.com/repo.git"
  VERSION="test"
  REPO_DIR=""
  NODE_COUNT=0
  POOL_COUNT=0
  CLUSTER_COUNT=0
  AUTO_MODE="false"
  SUDO_KEEPALIVE_PID=""
  API_TUNNEL_PIDS=()
  API_TUNNEL_BACKUPS=()
  TUNNEL_WATCHDOG_PID=""
  TUNNEL_CONF_DIR=""
  mkdir -p "$LOG_DIR"
  : > "$LOG_FILE"

  # phase_provision: succeed — simulates successful tunnel setup
  phase_provision() { return 0; }
  phase_deps()      { return 0; }

  eval "$(extract_func 'main')"

  set +e
  SCALEX_REPO_DIR="$T6_REPO" main --auto
  RC=$?
  set -e
  exit "$RC"
) > "$TEST_TMPDIR/t6_out" 2>&1
T6_RC=$?

if [[ $T6_RC -eq 0 ]]; then
  pass "T6: main() --auto exits 0 when phase_provision succeeds"
else
  fail "T6: main() --auto exited $T6_RC (expected 0) when phase_provision succeeded"
  cat "$TEST_TMPDIR/t6_out" >&2
fi

# No [ERROR_MSG] on success
if grep -q '\[ERROR_MSG\]' "$TEST_TMPDIR/t6_out" 2>/dev/null; then
  fail "T6: unexpected [ERROR_MSG] on successful auto mode run"
  grep '\[ERROR_MSG\]' "$TEST_TMPDIR/t6_out" >&2
else
  pass "T6: no spurious error messages on success"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ $FAIL -eq 0 ]]
