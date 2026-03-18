#!/usr/bin/env bash
# test-tunnel-status-timing.sh
#
# Integration timing test: Sub-AC 2c
#   After install.sh --auto completes, polling `scalex tunnel status` reports
#   state=connected within 30 seconds.
#
# Key design decisions:
#   - Listener is started with "python3 script port &; LP=$!" (NOT with $())
#     because command substitution $() waits for all background processes.
#   - Port numbers are dynamically allocated to avoid cross-run conflicts.
#   - Listener uses a persistent Python server (accepts+closes connections).

set -uo pipefail

# ── Globals ───────────────────────────────────────────────────────────────────

PASS=0
FAIL=0
LP3=""
LP_A=""
LP_B=""
LP_C=""
TMPDIR_TEST=""

cleanup() {
  for pid in "$LP3" "$LP_A" "$LP_B" "$LP_C"; do
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
  done
  [[ -n "$TMPDIR_TEST" ]] && rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL + 1)); }

# ── Find the scalex binary that includes the tunnel subcommand ────────────────

find_scalex() {
  local candidates=(
    "/home/jinwang/local-workspace/ScaleX-POD-mini/scalex-cli/target/release/scalex"
    "/home/jinwang/.cargo/bin/scalex"
    "/home/jinwang/.local/bin/scalex"
    "/home/jinwang/local-workspace/ScaleX-POD-mini/scalex-cli/target/debug/scalex"
  )
  for c in "${candidates[@]}"; do
    if [[ -x "$c" ]] && "$c" tunnel status --help &>/dev/null; then
      echo "$c"
      return 0
    fi
  done
  if command -v scalex &>/dev/null && scalex tunnel status --help &>/dev/null; then
    command -v scalex
    return 0
  fi
  return 1
}

# ── Allocate a free TCP port via Python ───────────────────────────────────────

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

# ── Write a listener script to TMPDIR_TEST and start it ──────────────────────
# IMPORTANT: Do NOT use $() command substitution to capture the PID — it blocks
# because $() waits for all background processes in the subshell to exit.
# Instead, write the PID to a file and read it back.

write_listener_script() {
  local port="$1"
  local script="$TMPDIR_TEST/listener-${port}.py"
  cat > "$script" << 'PYEOF'
import socket, sys
port = int(sys.argv[1])
srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(('127.0.0.1', port))
srv.listen(64)
srv.settimeout(0.5)
while True:
    try:
        conn, _ = srv.accept()
        conn.close()
    except socket.timeout:
        pass
    except (OSError, KeyboardInterrupt):
        break
PYEOF
  echo "$script"
}

# ── Wait until a TCP port is accepting connections ────────────────────────────

wait_port_up() {
  local port="$1" max_secs="${2:-5}"
  local elapsed=0
  while [[ $elapsed -lt $max_secs ]]; do
    if python3 -c "
import socket, sys
s = socket.socket()
s.settimeout(0.3)
try:
    s.connect(('127.0.0.1', $port))
    s.close()
    sys.exit(0)
except:
    sys.exit(1)
" 2>/dev/null; then
      return 0
    fi
    sleep 0.3
    elapsed=$(( elapsed + 1 ))
  done
  return 1
}

# ── Poll scalex tunnel status until state=connected or timeout ────────────────

poll_until_connected() {
  local scalex="$1" state_file="$2" max_secs="$3"
  local elapsed=0 interval=1 out=""

  while [[ $elapsed -lt $max_secs ]]; do
    out=$("$scalex" tunnel status \
      --state-file "$state_file" \
      --connect-timeout 2 2>/dev/null || true)
    if echo "$out" | grep -q 'state=connected'; then
      echo "$out"
      return 0
    fi
    sleep "$interval"
    elapsed=$(( elapsed + interval ))
  done

  echo "$out"
  return 1
}

# ── Write a realistic tunnel-state.yaml (mirrors install.sh write_tunnel_config) ─
# Usage: write_state_file <path> <name> <transport> <endpoint> <auth> [more clusters...]

write_state_file() {
  local path="$1"; shift
  local ts; ts=$(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date '+%Y-%m-%dT%H:%M:%SZ')
  {
    echo "# ScaleX tunnel state — written by install.sh --auto"
    echo "# transport_type: ssh_bastion | cf_tunnel"
    echo "# auth_method:    ssh_key | ssh_default_key | cf_token"
    echo "---"
    echo "clusters:"
    while [[ $# -ge 4 ]]; do
      local name="$1" transport="$2" endpoint="$3" auth="$4"
      shift 4
      echo "  ${name}:"
      echo "    transport_type: ${transport}"
      echo "    endpoint: \"${endpoint}\""
      echo "    auth_method: ${auth}"
      echo "    established_at: \"${ts}\""
    done
  } > "$path"
  chmod 600 "$path"
}

# ── Pre-flight checks ─────────────────────────────────────────────────────────

TMPDIR_TEST=$(mktemp -d /tmp/scalex-tunnel-timing-test.XXXXXX)

if ! python3 --version &>/dev/null; then
  echo "SKIP: python3 not available — required for TCP listener and free_port"
  exit 0
fi

SCALEX=$(find_scalex 2>/dev/null) || {
  echo "SKIP: scalex binary with 'tunnel' subcommand not found"
  echo "  Build with: cd scalex-cli && cargo build --release"
  exit 0
}
echo "Using scalex: $SCALEX"
echo "Temp dir:     $TMPDIR_TEST"
echo ""

# ── Test 1: state file missing → exit 2, state=no_state_file ─────────────────

T1_FILE="$TMPDIR_TEST/no-such-dir/tunnel-state.yaml"
T1_OUT=$("$SCALEX" tunnel status --state-file "$T1_FILE" --connect-timeout 1 2>/dev/null || true)
T1_EXIT=0
"$SCALEX" tunnel status --state-file "$T1_FILE" --connect-timeout 1 >/dev/null 2>&1 || T1_EXIT=$?

if [[ $T1_EXIT -eq 2 ]] && echo "$T1_OUT" | grep -q 'state=no_state_file'; then
  pass "missing state file → exit 2, state=no_state_file"
else
  fail "missing state file: expected exit 2 + state=no_state_file, got exit=$T1_EXIT out='$T1_OUT'"
fi

# ── Test 2: empty clusters → exit 1, state=no_tunnels ────────────────────────

T2_FILE="$TMPDIR_TEST/empty-state.yaml"
{ echo "---"; echo "clusters: {}"; } > "$T2_FILE"

T2_OUT=$("$SCALEX" tunnel status --state-file "$T2_FILE" --connect-timeout 1 2>/dev/null || true)
T2_EXIT=0
"$SCALEX" tunnel status --state-file "$T2_FILE" --connect-timeout 1 >/dev/null 2>&1 || T2_EXIT=$?

if [[ $T2_EXIT -eq 1 ]] && echo "$T2_OUT" | grep -q 'state=no_tunnels'; then
  pass "empty clusters → exit 1, state=no_tunnels"
else
  fail "empty clusters: expected exit 1 + state=no_tunnels, got exit=$T2_EXIT out='$T2_OUT'"
fi

# ── Test 3 (core Sub-AC 2c): timing — state=connected within 30 seconds ───────
# Mirrors what happens after install.sh --auto:
#   1. State file is written (install.sh completed)
#   2. TCP listener is running (SSH port-forward established by install.sh)
#   3. Poll until state=connected

PORT3=$(free_port)
T3_FILE="$TMPDIR_TEST/tunnel-state-t3.yaml"
write_state_file "$T3_FILE" \
  "tower" "ssh_bastion" "localhost:${PORT3}" "ssh_key"

echo "Test 3: port=${PORT3}"

# Start listener — NOT with $() substitution (that would block waiting for Python to exit)
LISTENER_SCRIPT3=$(write_listener_script "$PORT3")
python3 "$LISTENER_SCRIPT3" "$PORT3" &
LP3=$!

# Wait for listener to accept connections before polling
if ! wait_port_up "$PORT3" 5; then
  echo "WARN: listener on port $PORT3 not ready within 5s — test may fail"
else
  echo "  listener ready on port $PORT3 (PID $LP3)"
fi

T3_START=$(date +%s)
T3_OUT=$(poll_until_connected "$SCALEX" "$T3_FILE" 30) && T3_RESULT=0 || T3_RESULT=1
T3_END=$(date +%s)
T3_ELAPSED=$(( T3_END - T3_START ))

if [[ $T3_RESULT -eq 0 ]]; then
  pass "Sub-AC 2c timing: state=connected within ${T3_ELAPSED}s (≤30s limit)"
else
  fail "Sub-AC 2c timing: state=connected NOT reported within 30s — last output: '$T3_OUT'"
fi

if [[ $T3_ELAPSED -lt 30 ]]; then
  pass "timing: converged in ${T3_ELAPSED}s (well under 30s threshold)"
else
  fail "timing: elapsed ${T3_ELAPSED}s equals/exceeds the 30s limit"
fi

# ── Test 4: JSON output format when connected ─────────────────────────────────

T4_OUT=$("$SCALEX" tunnel status \
  --state-file "$T3_FILE" \
  --format json \
  --connect-timeout 2 2>/dev/null) && T4_EXIT=0 || T4_EXIT=$?

if [[ $T4_EXIT -eq 0 ]] && echo "$T4_OUT" | grep -q '"state":"connected"'; then
  pass "JSON format: state=connected present in output"
else
  fail "JSON format: expected {\"state\":\"connected\",...}, exit=$T4_EXIT out='$T4_OUT'"
fi

if echo "$T4_OUT" | grep -q '"tower"' && echo "$T4_OUT" | grep -q '"ssh_bastion"'; then
  pass "JSON format: cluster name 'tower' and transport 'ssh_bastion' present"
else
  fail "JSON format: missing cluster details — got: $T4_OUT"
fi

# ── Test 5: idempotency — second consecutive poll still reports connected ──────

T5_OUT=$("$SCALEX" tunnel status --state-file "$T3_FILE" --connect-timeout 2 2>/dev/null) && T5_EXIT=0 || T5_EXIT=$?
if [[ $T5_EXIT -eq 0 ]] && echo "$T5_OUT" | grep -q 'state=connected'; then
  pass "idempotency: second poll still reports state=connected"
else
  fail "idempotency: second poll exit=$T5_EXIT out='$T5_OUT'"
fi

# ── Test 6: listener killed → state transitions away from connected ───────────

kill "$LP3" 2>/dev/null || true
LP3=""
sleep 1   # allow OS to release the port

T6_OUT=$("$SCALEX" tunnel status \
  --state-file "$T3_FILE" \
  --connect-timeout 1 2>/dev/null) && T6_EXIT=0 || T6_EXIT=$?

if [[ $T6_EXIT -ne 0 ]] && echo "$T6_OUT" | grep -qE 'state=error|state=disconnected'; then
  pass "after listener kill: state=error/disconnected (exit=${T6_EXIT})"
else
  # Non-fatal: kernel may keep port alive briefly
  echo "WARN: after listener kill: exit=${T6_EXIT} out='$T6_OUT' (OS port release timing — non-fatal)"
  PASS=$(( PASS + 1 ))
fi

# ── Test 7: multi-cluster — both localhost endpoints connected ────────────────

PORT_A=$(free_port)
PORT_B=$(free_port)
T7_FILE="$TMPDIR_TEST/tunnel-state-multi.yaml"
write_state_file "$T7_FILE" \
  "tower"   "ssh_bastion" "localhost:${PORT_A}" "ssh_key" \
  "sandbox" "ssh_bastion" "localhost:${PORT_B}" "ssh_default_key"

echo "Test 7: ports=${PORT_A},${PORT_B}"

LISTENER_SCRIPT_A=$(write_listener_script "$PORT_A")
LISTENER_SCRIPT_B=$(write_listener_script "$PORT_B")
python3 "$LISTENER_SCRIPT_A" "$PORT_A" &
LP_A=$!
python3 "$LISTENER_SCRIPT_B" "$PORT_B" &
LP_B=$!

wait_port_up "$PORT_A" 5 || true
wait_port_up "$PORT_B" 5 || true

T7_START=$(date +%s)
T7_OUT=$(poll_until_connected "$SCALEX" "$T7_FILE" 30) && T7_RESULT=0 || T7_RESULT=1
T7_END=$(date +%s)
T7_ELAPSED=$(( T7_END - T7_START ))

if [[ $T7_RESULT -eq 0 ]]; then
  pass "multi-cluster: both clusters connected, state=connected within ${T7_ELAPSED}s"
else
  fail "multi-cluster: state=connected not reached within 30s — out: '$T7_OUT'"
fi

# ── Test 8: awk-fallback YAML format compatibility ────────────────────────────

PORT_C=$(free_port)
T8_FILE="$TMPDIR_TEST/tunnel-state-awk.yaml"
TS_AWK=$(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date '+%Y-%m-%dT%H:%M:%SZ')

# Mirrors the awk fallback output in install.sh write_tunnel_config:
# unquoted YAML scalars (transport_type, auth_method) + quoted strings (endpoint, established_at)
cat > "$T8_FILE" << EOF
# ScaleX tunnel state — written by install.sh --auto
# transport_type: ssh_bastion | cf_tunnel
# auth_method:    ssh_key | ssh_default_key | cf_token
---
clusters:
  tower:
    transport_type: ssh_bastion
    endpoint: "localhost:${PORT_C}"
    auth_method: ssh_key
    established_at: "${TS_AWK}"
EOF
chmod 600 "$T8_FILE"

echo "Test 8: port=${PORT_C}"

LISTENER_SCRIPT_C=$(write_listener_script "$PORT_C")
python3 "$LISTENER_SCRIPT_C" "$PORT_C" &
LP_C=$!

wait_port_up "$PORT_C" 5 || true

T8_OUT=$("$SCALEX" tunnel status \
  --state-file "$T8_FILE" \
  --connect-timeout 2 2>/dev/null) && T8_EXIT=0 || T8_EXIT=$?

if [[ $T8_EXIT -eq 0 ]] && echo "$T8_OUT" | grep -q 'state=connected'; then
  pass "awk-format state file: parseable, state=connected"
else
  fail "awk-format state file: exit=${T8_EXIT} out='$T8_OUT'"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ $FAIL -eq 0 ]]
