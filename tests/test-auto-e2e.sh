#!/usr/bin/env bash
# Test: install.sh --auto E2E on provisioned environment
# Sub-AC 5c: Verify install.sh --auto completes successfully with:
#   - All pre-flight checks pass (config files, credentials, SSH)
#   - Phase 0 (deps) skipped when already complete
#   - Phase 4 (provision) skipped when already complete
#   - All 3 SSH health checks pass (pre-flight, pre-provision, post-install)
#   - Exit code 0
#
# Requires: all 4 playbox nodes accessible via SSH, phase state files set

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PASS=0
FAIL=0

pass() { printf '  PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

echo ""
echo "=== Test: install.sh --auto E2E on provisioned environment ==="
echo ""

# ── Pre-check: phase state ────────────────────────────────────────────────────
echo "--- Phase state verification ---"
PHASE_FILE="$HOME/.scalex/installer/phase_completed"
PHASE_DONE_DIR="$HOME/.scalex/installer/phases"

if [[ -f "$PHASE_FILE" ]] && [[ "$(cat "$PHASE_FILE")" == "4" ]]; then
  pass "phase_completed=4 (sequential tracker)"
else
  fail "phase_completed not 4 (got: $(cat "$PHASE_FILE" 2>/dev/null || echo 'missing'))"
fi

for n in 0 1 2 3 4; do
  if [[ -f "$PHASE_DONE_DIR/${n}.done" ]]; then
    pass "Phase ${n}.done file exists"
  else
    fail "Phase ${n}.done file missing"
  fi
done

# ── Pre-check: SSH connectivity to all playbox nodes ─────────────────────────
echo ""
echo "--- SSH connectivity pre-check ---"
for host in playbox-0 playbox-1 playbox-2 playbox-3; do
  if ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 -o BatchMode=yes \
       jinwang@"$host" "echo ok" &>/dev/null 2>&1; then
    pass "SSH to $host"
  else
    fail "SSH to $host (unreachable)"
  fi
done

# ── Run install.sh --auto ─────────────────────────────────────────────────────
echo ""
echo "--- install.sh --auto run ---"
LOG_OUT=$(mktemp /tmp/scalex-e2e-XXXXXX.log)
trap 'rm -f "$LOG_OUT"' EXIT

set +e
(cd "$PROJECT_ROOT" && timeout 120 bash install.sh --auto) > "$LOG_OUT" 2>&1
EXIT_CODE=$?
set -e

if [[ $EXIT_CODE -eq 0 ]]; then
  pass "install.sh --auto exit code 0"
else
  fail "install.sh --auto exit code ${EXIT_CODE} (expected 0)"
fi

# ── Verify key events in output ──────────────────────────────────────────────
echo ""
echo "--- Output validation ---"

if grep -q "SSH 상태 확인 통과 \[사전 확인\]: 4/4 노드 OK\|SSH health check PASSED \[pre-flight\]: 4/4 nodes OK" "$LOG_OUT" 2>/dev/null; then
  pass "Pre-flight SSH check: 4/4 nodes passed"
else
  fail "Pre-flight SSH check not found or failed in output"
fi

if grep -q "SSH 상태 확인 통과 \[프로비저닝 전\]: 4/4 노드 OK\|SSH health check PASSED \[pre-provision\]: 4/4 nodes OK" "$LOG_OUT" 2>/dev/null; then
  pass "Pre-provision SSH check: 4/4 nodes passed"
else
  fail "Pre-provision SSH check not found or failed in output"
fi

if grep -q "SSH 상태 확인 통과 \[설치 후 확인\]: 4/4 노드 OK\|SSH health check PASSED \[post-install\]: 4/4 nodes OK" "$LOG_OUT" 2>/dev/null; then
  pass "Post-install SSH check: 4/4 nodes passed"
else
  fail "Post-install SSH check not found or failed in output"
fi

if grep -q "Phase 0 이미 완료 — 의존성 확인 건너뜀\|Phase 0 already complete — skipping" "$LOG_OUT" 2>/dev/null; then
  pass "Phase 0 skipped (already complete)"
else
  fail "Phase 0 skip not detected in output"
fi

if grep -q "Phase 4 이미 완료 — 프로비저닝 건너뜀\|Phase 4 already complete — skipping" "$LOG_OUT" 2>/dev/null; then
  pass "Phase 4 skipped (already complete)"
else
  fail "Phase 4 skip not detected in output"
fi

# The success message is written to the installer log file (log_raw → LOG_FILE), not stdout.
# Find the most recent install log and check it.
INSTALL_LOG=$(ls -t "$HOME/.scalex/installer/logs/install-"*.log 2>/dev/null | head -1)
if [[ -n "$INSTALL_LOG" ]] && grep -q "Installation completed successfully (auto mode)" "$INSTALL_LOG" 2>/dev/null; then
  pass "Installation completed successfully (auto mode) — confirmed in installer log"
elif grep -q "ScaleX 설치 완료\|Installation complete\|All tests passed" "$LOG_OUT" 2>/dev/null; then
  pass "Installation completed successfully (success banner in stdout)"
else
  fail "Success confirmation not found (stdout or installer log)"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "====================================="
printf 'Results: %d passed, %d failed\n' "$PASS" "$FAIL"
echo "====================================="

if [[ $FAIL -eq 0 ]]; then
  echo "E2E: PASSED ✓"
  exit 0
else
  echo "E2E: FAILED ✗"
  echo ""
  echo "--- install.sh --auto output (last 30 lines) ---"
  tail -30 "$LOG_OUT" 2>/dev/null || true
  exit 1
fi
