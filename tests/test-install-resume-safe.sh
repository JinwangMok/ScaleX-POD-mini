#!/usr/bin/env bash
# Test: install.sh resume-safe behavior
# Sub-AC 7c: Simulate mid-run interruption after phase N, re-run, and assert
#            phases 0..N are skipped while phases N+1..4 execute successfully.
#
# Two skip-guard mechanisms under test:
#   1.  phase_skip_if_done N  — inner guard inside each phase function
#   2.  if (( completed < N )) — outer guard in the main() orchestrator
#
# No real infrastructure required — all phase functions are mocked.

set -uo pipefail

PASS=0
FAIL=0

# ── assert helper ─────────────────────────────────────────────────────────────
pass() { printf '  PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL: %s\n  expected=[%s]  actual=[%s]\n' "$1" "$2" "$3"; FAIL=$((FAIL + 1)); }
assert() {
  local desc="$1" expected="$2" actual="$3"
  [[ "$actual" == "$expected" ]] && pass "$desc" || fail "$desc" "$expected" "$actual"
}

# ── Common setup (inlined state functions identical to install.sh) ───────────
# State functions are inlined rather than awk-extracted to ensure test
# reliability. Logic verified against install.sh lines 1538-1559.

_setup_env() {
  # Caller must provide PHASE_FILE and STATE_FILE as local paths in a temp dir.
  # Stubs for helpers referenced inside phase functions / skip guard.
  i18n()      { echo "$1"; }
  log_info()  { :; }
  log_phase() { :; }

  state_save_phase() { echo "$1" > "$PHASE_FILE"; }
  state_get_phase()  { [[ -f "$PHASE_FILE" ]] && cat "$PHASE_FILE" || echo "-1"; }

  phase_label() {
    case "$1" in
      0) echo "Dependencies"      ;; 1) echo "Bare-metal & SSH" ;;
      2) echo "SDI Virtualization";; 3) echo "Cluster & GitOps" ;;
      4) echo "Build & Provision" ;; *) echo "Unknown" ;;
    esac
  }

  # install.sh phase_skip_if_done (lines 1597-1608)
  phase_skip_if_done() {
    local phase_num="$1"
    local completed; completed=$(state_get_phase)
    (( completed >= phase_num )) && return 0 || return 1
  }
}

# ── Mock phase functions ──────────────────────────────────────────────────────
# Each mock: (a) early-returns via inner skip guard, (b) records itself in
# CALLED, (c) writes state_save_phase — mirroring real phase function behavior.
_define_mock_phases() {
  CALLED=""
  phase_deps()      { phase_skip_if_done 0 && return 0; CALLED="${CALLED}0 "; state_save_phase 0; }
  phase_baremetal() { phase_skip_if_done 1 && return 0; CALLED="${CALLED}1 "; state_save_phase 1; }
  phase_sdi()       { phase_skip_if_done 2 && return 0; CALLED="${CALLED}2 "; state_save_phase 2; }
  phase_cluster()   { phase_skip_if_done 3 && return 0; CALLED="${CALLED}3 "; state_save_phase 3; }
  phase_provision() { phase_skip_if_done 4 && return 0; CALLED="${CALLED}4 "; state_save_phase 4; }
}

# ── Orchestrator (mirrors install.sh main() lines 3006-3034) ─────────────────
_run_orchestrator() {
  _define_mock_phases
  completed=$(state_get_phase)
  if (( completed < 0 )); then phase_deps;      completed=0; fi
  if (( completed < 1 )); then phase_baremetal; completed=1; fi
  if (( completed < 2 )); then phase_sdi;       completed=2; fi
  if (( completed < 3 )); then phase_cluster;   completed=3; fi
  if (( completed < 4 )); then phase_provision; completed=4; fi
}

# ═══════════════════════════════════════════════════════════════════════════════
# Section 1 — Outer orchestrator: resume after each interruption point
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "=== Section 1: Orchestrator skip-logic for each interruption point ==="

for interrupted_at in -1 0 1 2 3 4; do
  result=$(
    TD=$(mktemp -d); trap 'rm -rf "$TD"' EXIT
    PHASE_FILE="$TD/phase_completed"
    STATE_FILE="$TD/state.env"
    _setup_env
    (( interrupted_at >= 0 )) && state_save_phase "$interrupted_at"
    _run_orchestrator
    printf '%s' "${CALLED% }"
  )

  expected=""
  for p in 0 1 2 3 4; do (( p > interrupted_at )) && expected="${expected}${p} "; done
  expected="${expected% }"

  case "$interrupted_at" in
    -1) desc="fresh start → all 5 phases run" ;;
     4) desc="all done (state=4) → 0 phases run (idempotent)" ;;
     *) desc="interrupted after phase $interrupted_at → phases $((interrupted_at+1))..4 run" ;;
  esac
  assert "$desc" "$expected" "$result"
done

# ═══════════════════════════════════════════════════════════════════════════════
# Section 2 — Inner phase_skip_if_done guard
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "=== Section 2: phase_skip_if_done inner guard (per-phase idempotency) ==="

for completed_at in -1 0 1 2 3 4; do
  for phase in 0 1 2 3 4; do
    result=$(
      TD=$(mktemp -d); trap 'rm -rf "$TD"' EXIT
      PHASE_FILE="$TD/phase_completed"
      STATE_FILE="$TD/state.env"
      _setup_env
      (( completed_at >= 0 )) && state_save_phase "$completed_at"
      phase_skip_if_done "$phase" && echo "skip" || echo "run"
    )
    if (( completed_at >= phase )); then
      assert "guard: state=$completed_at, phase=$phase → skip" "skip" "$result"
    else
      assert "guard: state=$completed_at, phase=$phase → run"  "run"  "$result"
    fi
  done
done

# ═══════════════════════════════════════════════════════════════════════════════
# Section 3 — Multi-run simulation with persistent state (crash + resume)
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "=== Section 3: Multi-run simulation (crash-at-2 → resume → idempotent) ==="

# All 3 "runs" share the same PHASE_FILE so state carries across.
_s3=$(
  TD=$(mktemp -d); trap 'rm -rf "$TD"' EXIT
  PHASE_FILE="$TD/phase_completed"
  STATE_FILE="$TD/state.env"
  _setup_env

  P=0; F=0
  chk() {
    local d="$1" e="$2" a="$3"
    if [[ "$a" == "$e" ]]; then P=$((P+1)); printf '  PASS: %s\n' "$d"
    else F=$((F+1)); printf '  FAIL: %s\n  expected=[%s]  actual=[%s]\n' "$d" "$e" "$a"; fi
  }

  # ── Simulated run 1: phases 0-2 complete, process killed ─────────────────
  # (We write phase 2 directly to simulate the crash after phase_sdi saved it)
  state_save_phase 2

  # ── Run 2: resume from state=2, should complete phases 3 and 4 ───────────
  _run_orchestrator
  chk "run2: only phases 3 and 4 executed" "3 4" "${CALLED% }"
  chk "run2: PHASE_FILE=4 after completion" "4" "$(state_get_phase)"

  # ── Run 3: idempotent — all phases already complete, nothing should run ──
  _run_orchestrator
  chk "run3: no phases run (all done, idempotent)" "" "${CALLED% }"
  chk "run3: PHASE_FILE still 4" "4" "$(state_get_phase)"

  echo "S3_P=$P S3_F=$F"
)
# Print captured output (PASS/FAIL lines) and extract counts
while IFS= read -r _line; do
  if [[ "$_line" =~ S3_P=([0-9]+)[[:space:]]S3_F=([0-9]+) ]]; then
    PASS=$((PASS + ${BASH_REMATCH[1]}))
    FAIL=$((FAIL + ${BASH_REMATCH[2]}))
  else
    echo "$_line"
  fi
done <<< "$_s3"

# ═══════════════════════════════════════════════════════════════════════════════
# Section 4 — PHASE_FILE integrity after full run
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "=== Section 4: PHASE_FILE integrity after complete run ==="

_s4=$(
  TD=$(mktemp -d); trap 'rm -rf "$TD"' EXIT
  PHASE_FILE="$TD/phase_completed"
  STATE_FILE="$TD/state.env"
  _setup_env
  P=0; F=0
  chk() {
    local d="$1" e="$2" a="$3"
    if [[ "$a" == "$e" ]]; then P=$((P+1)); printf '  PASS: %s\n' "$d"
    else F=$((F+1)); printf '  FAIL: %s\n  expected=[%s]  actual=[%s]\n' "$d" "$e" "$a"; fi
  }

  # Fresh full run (no prior state)
  _run_orchestrator
  chk "all 5 phases run on fresh start" "0 1 2 3 4" "${CALLED% }"
  chk "PHASE_FILE=4 after successful full run" "4" "$(state_get_phase)"

  # Verify no prior-state = -1 sentinel
  rm -f "$PHASE_FILE"
  fresh_phase=$(state_get_phase)
  chk "state_get_phase returns -1 when no PHASE_FILE" "-1" "$fresh_phase"

  echo "S4_P=$P S4_F=$F"
)
while IFS= read -r _line; do
  if [[ "$_line" =~ S4_P=([0-9]+)[[:space:]]S4_F=([0-9]+) ]]; then
    PASS=$((PASS + ${BASH_REMATCH[1]}))
    FAIL=$((FAIL + ${BASH_REMATCH[2]}))
  else
    echo "$_line"
  fi
done <<< "$_s4"

# ═══════════════════════════════════════════════════════════════════════════════
# Section 5 — Verify behavior matches actual install.sh state functions
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "=== Section 5: Cross-check state functions against actual install.sh ==="

INSTALL_SH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/install.sh"

if [[ -f "$INSTALL_SH" ]]; then
  # Extract state_save_phase and state_get_phase from actual install.sh
  # and verify they behave identically to our inlined copies.
  _extracted=$(awk '
    /^state_save_phase\(\)/ { print; next }
    /^state_get_phase\(\)/  { print; next }
  ' "$INSTALL_SH")

  _s5=$(
    TD=$(mktemp -d); trap 'rm -rf "$TD"' EXIT
    PHASE_FILE="$TD/phase_completed"
    STATE_FILE="$TD/state.env"
    # Load the REAL functions from install.sh
    eval "$_extracted"
    P=0; F=0
    chk() {
      local d="$1" e="$2" a="$3"
      if [[ "$a" == "$e" ]]; then P=$((P+1)); printf '  PASS: %s\n' "$d"
      else F=$((F+1)); printf '  FAIL: %s\n  expected=[%s]  actual=[%s]\n' "$d" "$e" "$a"; fi
    }
    chk "no PHASE_FILE → state_get_phase returns -1" "-1" "$(state_get_phase)"
    state_save_phase 2
    chk "after state_save_phase 2 → state_get_phase returns 2" "2" "$(state_get_phase)"
    state_save_phase 4
    chk "after state_save_phase 4 → state_get_phase returns 4" "4" "$(state_get_phase)"
    echo "S5_P=$P S5_F=$F"
  )
  while IFS= read -r _line; do
    if [[ "$_line" =~ S5_P=([0-9]+)[[:space:]]S5_F=([0-9]+) ]]; then
      PASS=$((PASS + ${BASH_REMATCH[1]}))
      FAIL=$((FAIL + ${BASH_REMATCH[2]}))
    else
      echo "$_line"
    fi
  done <<< "$_s5"
else
  echo "  SKIP: install.sh not found at $INSTALL_SH"
fi

# ═══════════════════════════════════════════════════════════════════════════════
# Final Summary
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "====================================="
printf 'Results: %d passed, %d failed\n' "$PASS" "$FAIL"
echo "====================================="
[[ $FAIL -eq 0 ]]
