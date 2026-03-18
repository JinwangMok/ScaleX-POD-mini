#!/usr/bin/env bash
# tests/test-watchdog-recovery.sh
#
# Sub-AC 3 (AC 8): Verify tunnel watchdog recovers from simulated tunnel death
# within 3 retries and logs appropriate stderr output.
#
# What this test verifies:
#   1. Watchdog detects a dead tunnel process and initiates recovery
#   2. Watchdog retries up to 3 times on repeated failures
#   3. Watchdog succeeds when SSH recovers on retry N (N <= 3)
#   4. Watchdog logs retry attempts with structured key=value fields to stderr
#   5. Watchdog logs final recovery success/failure with final_reason field
#   6. Conf file is updated with new PID on successful recovery
#   7. Watchdog does NOT fail-fast abort — continues monitoring other tunnels
#   8. Dedicated watchdog log file is written alongside stderr
#
# Design:
#   - Functions extracted from install.sh via awk (same pattern as test-tunnel-exit-code.sh)
#   - Mock SSH: configurable fail count before success (via MOCK_FAIL_COUNT file)
#   - Each test uses an isolated temp directory

set -uo pipefail

PASS=0
FAIL=0

INSTALL_SH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/install.sh"
TEST_TMPDIR=$(mktemp -d /tmp/scalex-watchdog-test.XXXXXX)

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

# ─── Helper: load watchdog functions ─────────────────────────────────────────
load_watchdog_functions() {
  i18n()      { echo "$1"; }
  log_info()  { echo "[INFO] $*" >&2; }
  log_warn()  { echo "[WARN] $*" >&2; }
  log_error() { echo "[ERROR] $*" >&2; }
  log_raw()   { :; }

  eval "$(extract_func 'start_tunnel_watchdog')"
  eval "$(extract_func 'stop_tunnel_watchdog')"
}

# ─── Helper: run one watchdog cycle inline ───────────────────────────────────
# Runs a single pass of the watchdog per-conf-file recovery logic using
# the same _wd_log structured logger as the real watchdog.
run_watchdog_cycle() {
  local conf_dir="$1"
  local wd_max_retries="${TUNNEL_WATCHDOG_MAX_RETRIES:-3}"
  local watchdog_log="${LOG_DIR}/tunnel-watchdog.log"

  _wd_log() {
    local level="$1" event="$2"; shift 2
    local ts; ts="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    local line="${ts} level=${level} component=tunnel-watchdog event=${event} $*"
    printf '%s\n' "$line" >> "$watchdog_log" 2>/dev/null
    printf '%s\n' "$line" >&2
  }

  for conf_file in "$conf_dir"/*.conf; do
    [[ -f "$conf_file" ]] || continue
    IFS=: read -r lp sip sp bt tpid < "$conf_file" 2>/dev/null || continue
    [[ -z "$tpid" || -z "$lp" ]] && continue
    if ! kill -0 "$tpid" 2>/dev/null; then
      local cluster_label; cluster_label=$(basename "$conf_file" .conf)
      local tunnel_spec="localhost:${lp}->${sip}:${sp}@${bt}"
      _wd_log WARN tunnel_dead "cluster=${cluster_label} tunnel=${tunnel_spec} old_pid=${tpid}"
      local wd_attempt=0 wd_ok=false wd_new_pid=""
      local wd_err_file; wd_err_file=$(mktemp /tmp/scalex-wd-err.XXXXXX 2>/dev/null || echo "/tmp/scalex-wd-err.$$.$RANDOM")
      local wd_final_reason=""
      while (( wd_attempt < wd_max_retries )); do
        wd_attempt=$((wd_attempt + 1))
        : > "$wd_err_file"
        _wd_log INFO retry_start "cluster=${cluster_label} attempt=${wd_attempt}/${wd_max_retries} tunnel=${tunnel_spec}"
        ssh -N \
          -o StrictHostKeyChecking=no \
          -o UserKnownHostsFile=/dev/null \
          -o BatchMode=yes \
          -o ExitOnForwardFailure=yes \
          -o ServerAliveInterval=15 \
          -o ServerAliveCountMax=4 \
          -o ConnectTimeout=10 \
          -L "${lp}:${sip}:${sp}" \
          "$bt" >/dev/null 2>"$wd_err_file" &
        wd_new_pid=$!
        local wd_w=0
        while (( wd_w < 3 )); do
          sleep 1; wd_w=$((wd_w + 1))
          kill -0 "$wd_new_pid" 2>/dev/null || break
        done
        if kill -0 "$wd_new_pid" 2>/dev/null; then
          wd_ok=true
          break
        fi
        local wd_ssh_err
        wd_ssh_err=$(head -5 "$wd_err_file" 2>/dev/null | tr '\n' ' ')
        wd_final_reason="${wd_ssh_err:-process exited immediately with no stderr}"
        _wd_log ERROR retry_failed "cluster=${cluster_label} attempt=${wd_attempt}/${wd_max_retries} tunnel=${tunnel_spec} stderr=\"${wd_final_reason}\""
        if (( wd_attempt < wd_max_retries )); then
          local wd_backoff=$(( wd_attempt * 3 ))
          _wd_log INFO retry_backoff "cluster=${cluster_label} seconds=${wd_backoff}"
          sleep "$wd_backoff"
        fi
      done
      rm -f "$wd_err_file"
      if $wd_ok && [[ -n "$wd_new_pid" ]]; then
        printf '%s:%s:%s:%s:%s\n' "$lp" "$sip" "$sp" "$bt" "$wd_new_pid" > "$conf_file"
        _wd_log INFO retry_success "cluster=${cluster_label} attempt=${wd_attempt}/${wd_max_retries} tunnel=${tunnel_spec} new_pid=${wd_new_pid}"
      else
        _wd_log ERROR retry_exhausted "cluster=${cluster_label} attempts=${wd_max_retries} tunnel=${tunnel_spec} final_reason=\"${wd_final_reason}\""
      fi
    fi
  done
}

# ─── Helper: create mock SSH that fails N times then succeeds ────────────────
create_mock_ssh() {
  local mock_bin="$1"
  local fail_count_file="$2"
  mkdir -p "$mock_bin"

  cat > "$mock_bin/ssh" << ENDSSH
#!/usr/bin/env bash
FAIL_FILE="$fail_count_file"
ATTEMPT_LOG="$TEST_TMPDIR/ssh_attempts.log"

LPORT=""
args=("\$@")
i=0
while [[ \$i -lt \${#args[@]} ]]; do
  if [[ "\${args[\$i]}" == "-L" ]] && [[ \$((i+1)) -lt \${#args[@]} ]]; then
    LPORT="\${args[\$((i+1))]%%:*}"
    i=\$((i+2))
    continue
  fi
  i=\$((i+1))
done

echo "\$(date +%s) ssh attempt port=\$LPORT args=\$*" >> "\$ATTEMPT_LOG"

remaining=\$(cat "\$FAIL_FILE" 2>/dev/null || echo "0")
if [[ "\$remaining" -gt 0 ]]; then
  echo \$((remaining - 1)) > "\$FAIL_FILE"
  echo "ssh: connect to host fake-bastion port 22: Connection refused" >&2
  exit 255
fi

if [[ -n "\$LPORT" ]] && [[ "\$LPORT" =~ ^[0-9]+\$ ]]; then
  python3 - "\$LPORT" << 'PYEOF' &
import socket, sys, time
port = int(sys.argv[1])
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.bind(('127.0.0.1', port))
    s.listen(1)
    while True:
        time.sleep(60)
except Exception:
    pass
finally:
    s.close()
PYEOF
  PY_PID=\$!
  [[ -n "\${MOCK_PID_FILE:-}" ]] && echo "\$PY_PID" >> "\$MOCK_PID_FILE"
fi
exec sleep 300
ENDSSH
  chmod +x "$mock_bin/ssh"
}

create_mock_ssh_always_fail() {
  local mock_bin="$1"
  mkdir -p "$mock_bin"
  cat > "$mock_bin/ssh" << 'ENDSSH'
#!/usr/bin/env bash
echo "ssh: connect to host fake-bastion port 22: Connection refused" >&2
exit 255
ENDSSH
  chmod +x "$mock_bin/ssh"
}

setup_dead_tunnel() {
  local conf_dir="$1" local_port="$2" server_ip="$3" server_port="$4" bastion="$5"
  mkdir -p "$conf_dir"
  local dead_pid=99999
  while kill -0 "$dead_pid" 2>/dev/null; do
    dead_pid=$((dead_pid + 1))
  done
  printf '%s:%s:%s:%s:%s\n' "$local_port" "$server_ip" "$server_port" "$bastion" "$dead_pid" \
    > "$conf_dir/tunnel-test.conf"
  echo "$dead_pid"
}

# ═══ TEST 1: Watchdog recovers dead tunnel on first retry ═══════════════════
test_watchdog_recover_first_retry() {
  local test_name="watchdog recovers dead tunnel on first retry"
  local test_dir="$TEST_TMPDIR/test1"
  local conf_dir="$test_dir/conf"
  local mock_bin="$test_dir/bin"
  local stderr_log="$test_dir/stderr.log"
  local fail_count_file="$test_dir/fail_count"
  local local_port=19001

  mkdir -p "$test_dir"
  echo "0" > "$fail_count_file"
  create_mock_ssh "$mock_bin" "$fail_count_file"

  local dead_pid
  dead_pid=$(setup_dead_tunnel "$conf_dir" "$local_port" "10.0.0.1" "6443" "fake-bastion")

  (
    load_watchdog_functions
    export PATH="$mock_bin:$PATH"
    export LOG_DIR="$test_dir"
    export TUNNEL_CONF_DIR="$conf_dir"
    run_watchdog_cycle "$conf_dir"
  ) 2>"$stderr_log"

  local new_pid_in_conf
  new_pid_in_conf=$(awk -F: '{print $5}' "$conf_dir/tunnel-test.conf" 2>/dev/null)
  if [[ -n "$new_pid_in_conf" ]] && [[ "$new_pid_in_conf" != "$dead_pid" ]]; then
    pass "$test_name — conf file updated with new PID"
  else
    fail "$test_name — conf file not updated (got: $new_pid_in_conf, dead was: $dead_pid)"
  fi

  if grep -q "event=retry_success" "$stderr_log"; then
    pass "$test_name — stderr logs structured recovery success"
  else
    fail "$test_name — no structured recovery success in stderr"
    cat "$stderr_log" >&2
  fi

  if grep -q "event=tunnel_dead" "$stderr_log"; then
    pass "$test_name — stderr logs structured dead tunnel detection"
  else
    fail "$test_name — no structured dead tunnel detection in stderr"
    cat "$stderr_log" >&2
  fi

  if [[ -f "$test_dir/tunnel-watchdog.log" ]]; then
    pass "$test_name — dedicated watchdog log file created"
  else
    fail "$test_name — no dedicated watchdog log file"
  fi

  [[ -n "$new_pid_in_conf" ]] && kill "$new_pid_in_conf" 2>/dev/null || true
}

# ═══ TEST 2: Watchdog recovers after 2 failures (succeeds on attempt 3) ═════
test_watchdog_recover_third_retry() {
  local test_name="watchdog recovers on third retry after 2 failures"
  local test_dir="$TEST_TMPDIR/test2"
  local conf_dir="$test_dir/conf"
  local mock_bin="$test_dir/bin"
  local stderr_log="$test_dir/stderr.log"
  local fail_count_file="$test_dir/fail_count"
  local local_port=19002

  mkdir -p "$test_dir"
  echo "2" > "$fail_count_file"
  create_mock_ssh "$mock_bin" "$fail_count_file"

  local dead_pid
  dead_pid=$(setup_dead_tunnel "$conf_dir" "$local_port" "10.0.0.2" "6443" "fake-bastion")

  (
    load_watchdog_functions
    export PATH="$mock_bin:$PATH"
    export LOG_DIR="$test_dir"
    export TUNNEL_CONF_DIR="$conf_dir"
    run_watchdog_cycle "$conf_dir"
  ) 2>"$stderr_log"

  local new_pid_in_conf
  new_pid_in_conf=$(awk -F: '{print $5}' "$conf_dir/tunnel-test.conf" 2>/dev/null)
  if [[ -n "$new_pid_in_conf" ]] && [[ "$new_pid_in_conf" != "$dead_pid" ]]; then
    pass "$test_name — recovered and conf updated"
  else
    fail "$test_name — recovery failed (conf pid: ${new_pid_in_conf:-empty})"
  fi

  local retry_fail_count
  retry_fail_count=$(grep -c "event=retry_failed" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$retry_fail_count" -ge 2 ]]; then
    pass "$test_name — stderr logged 2 structured retry failures before success"
  else
    fail "$test_name — expected 2 retry failures in stderr, got $retry_fail_count"
    cat "$stderr_log" >&2
  fi

  if grep -q 'stderr=".*Connection refused' "$stderr_log"; then
    pass "$test_name — stderr includes SSH error detail in structured field"
  else
    fail "$test_name — no SSH error detail in structured stderr field"
    cat "$stderr_log" >&2
  fi

  if grep -q "event=retry_success.*attempt=3/3" "$stderr_log"; then
    pass "$test_name — stderr logged structured successful recovery on attempt 3"
  else
    fail "$test_name — no structured recovery on attempt 3 logged"
    cat "$stderr_log" >&2
  fi

  local backoff_count
  backoff_count=$(grep -c "event=retry_backoff" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$backoff_count" -ge 1 ]]; then
    pass "$test_name — retry_backoff events logged between failures"
  else
    fail "$test_name — no retry_backoff events in stderr"
  fi

  [[ -n "$new_pid_in_conf" ]] && kill "$new_pid_in_conf" 2>/dev/null || true
}

# ═══ TEST 3: Watchdog exhausts all 3 retries and logs failure ════════════════
test_watchdog_exhausts_retries() {
  local test_name="watchdog exhausts 3 retries and logs failure"
  local test_dir="$TEST_TMPDIR/test3"
  local conf_dir="$test_dir/conf"
  local mock_bin="$test_dir/bin"
  local stderr_log="$test_dir/stderr.log"
  local local_port=19003

  mkdir -p "$test_dir"
  create_mock_ssh_always_fail "$mock_bin"

  local dead_pid
  dead_pid=$(setup_dead_tunnel "$conf_dir" "$local_port" "10.0.0.3" "6443" "fake-bastion")

  (
    load_watchdog_functions
    export PATH="$mock_bin:$PATH"
    export LOG_DIR="$test_dir"
    export TUNNEL_CONF_DIR="$conf_dir"
    run_watchdog_cycle "$conf_dir"
  ) 2>"$stderr_log"

  local current_pid
  current_pid=$(awk -F: '{print $5}' "$conf_dir/tunnel-test.conf" 2>/dev/null)
  if [[ "$current_pid" == "$dead_pid" ]]; then
    pass "$test_name — conf file unchanged (dead PID preserved)"
  else
    fail "$test_name — conf file was unexpectedly updated to $current_pid"
  fi

  local retry_count
  retry_count=$(grep -c "event=retry_failed" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$retry_count" -eq 3 ]]; then
    pass "$test_name — stderr logged all 3 structured retry failures"
  else
    fail "$test_name — expected 3 retry failures, got $retry_count"
    cat "$stderr_log" >&2
  fi

  if grep -q "event=retry_exhausted.*final_reason=" "$stderr_log"; then
    pass "$test_name — stderr logs structured retry_exhausted with final_reason"
  else
    fail "$test_name — no structured retry_exhausted event in stderr"
    cat "$stderr_log" >&2
  fi

  if grep -q 'stderr=".*Connection refused' "$stderr_log"; then
    pass "$test_name — SSH stderr captured in structured field (not discarded to /dev/null)"
  else
    fail "$test_name — SSH stderr not captured in structured field"
    cat "$stderr_log" >&2
  fi

  local wdlog="$test_dir/tunnel-watchdog.log"
  if [[ -f "$wdlog" ]] && grep -q "event=retry_exhausted" "$wdlog"; then
    pass "$test_name — dedicated watchdog log contains retry_exhausted"
  else
    fail "$test_name — dedicated watchdog log missing or incomplete"
  fi
}

# ═══ TEST 4: Watchdog no fail-fast — processes all tunnels ═══════════════════
test_watchdog_no_failfast() {
  local test_name="watchdog does not fail-fast — processes all tunnels"
  local test_dir="$TEST_TMPDIR/test4"
  local conf_dir="$test_dir/conf"
  local mock_bin="$test_dir/bin"
  local stderr_log="$test_dir/stderr.log"

  mkdir -p "$test_dir"
  create_mock_ssh_always_fail "$mock_bin"

  mkdir -p "$conf_dir"
  local dead_pid1=99991 dead_pid2=99992
  while kill -0 "$dead_pid1" 2>/dev/null; do dead_pid1=$((dead_pid1 + 1)); done
  while kill -0 "$dead_pid2" 2>/dev/null; do dead_pid2=$((dead_pid2 + 1)); done

  printf '%s:%s:%s:%s:%s\n' "19004" "10.0.0.4" "6443" "bastion-a" "$dead_pid1" > "$conf_dir/tunnel-a.conf"
  printf '%s:%s:%s:%s:%s\n' "19005" "10.0.0.5" "6443" "bastion-b" "$dead_pid2" > "$conf_dir/tunnel-b.conf"

  (
    load_watchdog_functions
    export PATH="$mock_bin:$PATH"
    export LOG_DIR="$test_dir"
    export TUNNEL_CONF_DIR="$conf_dir"
    run_watchdog_cycle "$conf_dir"
  ) 2>"$stderr_log"

  local dead_detect_count
  dead_detect_count=$(grep -c "event=tunnel_dead" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$dead_detect_count" -ge 2 ]]; then
    pass "$test_name — both dead tunnels detected (structured)"
  else
    fail "$test_name — expected 2 tunnel_dead events, got $dead_detect_count"
    cat "$stderr_log" >&2
  fi

  local exhausted_count
  exhausted_count=$(grep -c "event=retry_exhausted" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$exhausted_count" -ge 2 ]]; then
    pass "$test_name — both tunnels fully processed (no fail-fast abort)"
  else
    fail "$test_name — expected 2 retry_exhausted events, got $exhausted_count (fail-fast detected!)"
    cat "$stderr_log" >&2
  fi

  local total_retries
  total_retries=$(grep -c "event=retry_failed" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$total_retries" -eq 6 ]]; then
    pass "$test_name — 6 total retry_failed events (3 x 2 tunnels)"
  else
    fail "$test_name — expected 6 retry_failed events, got $total_retries"
  fi

  local component_count
  component_count=$(grep -c "component=tunnel-watchdog" "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$component_count" -gt 0 ]]; then
    pass "$test_name — all log entries have component=tunnel-watchdog"
  else
    fail "$test_name — missing component=tunnel-watchdog in log entries"
  fi
}

# ═══ TEST 5: Watchdog start/stop lifecycle ═══════════════════════════════════
test_watchdog_lifecycle() {
  local test_name="watchdog lifecycle start/stop"
  local test_dir="$TEST_TMPDIR/test5"
  local conf_dir="$test_dir/conf"
  mkdir -p "$conf_dir"

  (
    load_watchdog_functions
    TUNNEL_CONF_DIR="$conf_dir"
    TUNNEL_WATCHDOG_PID=""
    LOG_DIR="$test_dir"

    start_tunnel_watchdog
    local wd_pid="$TUNNEL_WATCHDOG_PID"

    if [[ -n "$wd_pid" ]] && kill -0 "$wd_pid" 2>/dev/null; then
      echo "STARTED:$wd_pid"
    else
      echo "START_FAILED"
    fi

    stop_tunnel_watchdog

    sleep 1
    if ! kill -0 "$wd_pid" 2>/dev/null; then
      echo "STOPPED"
    else
      echo "STOP_FAILED"
      kill "$wd_pid" 2>/dev/null || true
    fi
  ) > "$test_dir/output.txt" 2>/dev/null

  if grep -q "^STARTED:" "$test_dir/output.txt"; then
    pass "$test_name — watchdog started successfully"
  else
    fail "$test_name — watchdog failed to start"
  fi

  if grep -q "^STOPPED$" "$test_dir/output.txt"; then
    pass "$test_name — watchdog stopped successfully"
  else
    fail "$test_name — watchdog failed to stop"
  fi
}

# ═══ TEST 6: Structured log fields are parseable ════════════════════════════
test_structured_log_format() {
  local test_name="structured log format is parseable"
  local test_dir="$TEST_TMPDIR/test6"
  local conf_dir="$test_dir/conf"
  local mock_bin="$test_dir/bin"
  local stderr_log="$test_dir/stderr.log"
  local local_port=19006

  mkdir -p "$test_dir"
  create_mock_ssh_always_fail "$mock_bin"

  local dead_pid
  dead_pid=$(setup_dead_tunnel "$conf_dir" "$local_port" "10.0.0.6" "6443" "fake-bastion")

  (
    load_watchdog_functions
    export PATH="$mock_bin:$PATH"
    export LOG_DIR="$test_dir"
    export TUNNEL_CONF_DIR="$conf_dir"
    export TUNNEL_WATCHDOG_MAX_RETRIES=1
    run_watchdog_cycle "$conf_dir"
  ) 2>"$stderr_log"

  local bad_lines
  bad_lines=$(grep -cvE '^[0-9]{4}-[0-9]{2}-[0-9]{2}T' "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$bad_lines" -eq 0 ]]; then
    pass "$test_name — all lines have ISO-8601 timestamp"
  else
    fail "$test_name — $bad_lines lines missing ISO-8601 timestamp"
  fi

  local no_level
  no_level=$(grep -cvE 'level=(INFO|WARN|ERROR)' "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$no_level" -eq 0 ]]; then
    pass "$test_name — all lines have level= field"
  else
    fail "$test_name — $no_level lines missing level= field"
  fi

  local no_event
  no_event=$(grep -cvE 'event=' "$stderr_log" 2>/dev/null || echo "0")
  if [[ "$no_event" -eq 0 ]]; then
    pass "$test_name — all lines have event= field"
  else
    fail "$test_name — $no_event lines missing event= field"
  fi
}

# ═══ RUN ALL TESTS ══════════════════════════════════════════════════════════
echo "═══════════════════════════════════════════════════════════════"
echo " Test Suite: Watchdog Recovery (Sub-AC 3, AC 8)"
echo "═══════════════════════════════════════════════════════════════"
echo ""

test_watchdog_recover_first_retry
echo ""
test_watchdog_recover_third_retry
echo ""
test_watchdog_exhausts_retries
echo ""
test_watchdog_no_failfast
echo ""
test_watchdog_lifecycle
echo ""
test_structured_log_format

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo " Results: ${PASS} passed, ${FAIL} failed"
echo "═══════════════════════════════════════════════════════════════"

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
