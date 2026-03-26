#!/usr/bin/env bash
# install.sh — ScaleX Interactive TUI Installer
# Usage: bash install.sh              # Interactive TUI mode
#        bash install.sh --auto       # Unattended mode (requires pre-configured config files)
#        SCALEX_REPO_URL=https://github.com/yourfork/ScaleX-POD-mini.git bash install.sh
set -euo pipefail
umask 077

# ============================================================================
# Section 0: Constants & Globals
# ============================================================================
readonly VERSION="1.0"
readonly SCALEX_HOME="$HOME/.scalex"
readonly INSTALLER_DIR="$SCALEX_HOME/installer"
readonly STATE_FILE="$INSTALLER_DIR/state.env"
readonly PHASE_FILE="$INSTALLER_DIR/phase_completed"
readonly PHASE_DONE_DIR="$INSTALLER_DIR/phases"
readonly GEN_DIR="$INSTALLER_DIR/generated"
readonly LOG_DIR="$INSTALLER_DIR/logs"
readonly LOG_FILE="$LOG_DIR/install-$(date +%Y%m%d-%H%M%S).log"
readonly REPO_URL="${SCALEX_REPO_URL:-https://github.com/ScaleX-project/ScaleX-POD-mini.git}"
readonly TUNNEL_STATE_FILE="$SCALEX_HOME/tunnel-state.yaml"

readonly KUBECTL_VERSION="v1.33.1"
readonly HELM_VERSION="v3.17.3"
readonly OPENTOFU_VERSION="1.9.0"
readonly ARGOCD_VERSION="v2.14.0"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

# --- Language detection ---
detect_lang() {
  if [[ -n "${SCALEX_LANG:-}" ]]; then
    echo "$SCALEX_LANG"
  elif [[ "${LANG:-}" == ko* || "${LC_ALL:-}" == ko* ]]; then
    echo "ko"
  else
    echo "en"
  fi
}
SCALEX_LANG="$(detect_lang)"

i18n() {
  if [[ "$SCALEX_LANG" == "ko" ]]; then
    echo "${2:-$1}"
  else
    echo "$1"
  fi
}

TUI=""
REPO_DIR=""
NODE_COUNT=0
POOL_COUNT=0
CLUSTER_COUNT=0
AUTO_MODE="${AUTO_MODE:-false}"
SUDO_KEEPALIVE_PID=""
API_TUNNEL_PIDS=()
API_TUNNEL_BACKUPS=()
TUNNEL_WATCHDOG_PID=""
TUNNEL_CONF_DIR=""

# ============================================================================
# Section 1: Utility Functions
# ============================================================================

init_dirs() {
  mkdir -p "$INSTALLER_DIR" "$GEN_DIR/credentials" "$GEN_DIR/config" "$LOG_DIR" "$PHASE_DONE_DIR"
}

mask_secrets() {
  sed -E 's/(password|secret|pat|token|PASSWORD|SECRET|PAT|TOKEN)([=:]["'"'"' ]*)[^ "'"'"']*/\1\2****/gi'
}

log_raw() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | mask_secrets >> "$LOG_FILE"; }
log_info() { log_raw "INFO: $*"; echo -e "${GREEN}[INFO]${NC} $*" >&2; }
log_warn() { log_raw "WARN: $*"; echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
log_error() { log_raw "ERROR: $*"; echo -e "${RED}[ERROR]${NC} $*" >&2; }
log_phase() { log_raw "PHASE: $*"; echo -e "\n${BOLD}${BLUE}═══ $* ═══${NC}\n"; }

error_msg() {
  local what="$1" why="$2" how="$3"
  log_error "$what"
  echo -e "${RED}${BOLD}What:${NC} $what" >&2
  echo -e "${RED}${BOLD}Why:${NC}  $why" >&2
  echo -e "${RED}${BOLD}How:${NC}  $how" >&2
}

cleanup_handler() {
  local exit_code=$?
  cleanup_api_tunnels
  # Kill sudo keepalive (must be after tunnel cleanup, in case cleanup needs sudo)
  if [[ -n "$SUDO_KEEPALIVE_PID" ]]; then
    kill "$SUDO_KEEPALIVE_PID" 2>/dev/null || true
    SUDO_KEEPALIVE_PID=""
  fi
  if [[ $exit_code -ne 0 ]]; then
    echo ""
    log_warn "$(i18n "Installer was interrupted. Current state has been saved." "인스톨러가 중단되었습니다. 현재 상태가 저장되었습니다.")"
    log_warn "$(i18n "Re-run to resume: bash install.sh" "다시 실행하면 이어서 진행할 수 있습니다: bash install.sh")"
  fi
}
trap cleanup_handler EXIT
trap 'echo ""; log_warn "$(i18n "Ctrl+C detected. Saving state..." "Ctrl+C 감지. 상태를 저장합니다...")"; exit 130' INT

# --- TUI detection ---
detect_tui() {
  if command -v whiptail &>/dev/null; then TUI=whiptail
  elif command -v dialog &>/dev/null; then TUI=dialog
  else TUI=fallback; fi
  log_raw "TUI backend: $TUI"
}

tui_msgbox() {
  local title="$1" msg="$2"
  case "$TUI" in
    whiptail) whiptail --title "$title" --msgbox "$msg" 16 72 3>&1 1>&2 2>&3 || true ;;
    dialog)   dialog --title "$title" --msgbox "$msg" 16 72 3>&1 1>&2 2>&3 || true ;;
    fallback) echo -e "\n${BOLD}[$title]${NC}\n$msg\n"; read -rp "$(i18n "Press Enter to continue..." "Enter를 눌러 계속...")" ;;
  esac
}

tui_input() {
  local title="$1" prompt="$2" default="${3:-}"
  case "$TUI" in
    whiptail) whiptail --title "$title" --inputbox "$prompt" 10 72 "$default" 3>&1 1>&2 2>&3 ;;
    dialog)   dialog --title "$title" --inputbox "$prompt" 10 72 "$default" 3>&1 1>&2 2>&3 ;;
    fallback)
      echo -e "${BOLD}$title${NC}: $prompt"
      [[ -n "$default" ]] && echo -e "  $(i18n "(default: $default)" "(기본값: $default)")"
      local val; read -rp "> " val
      echo "${val:-$default}"
      ;;
  esac
}

tui_password() {
  local title="$1" prompt="$2"
  case "$TUI" in
    whiptail) whiptail --title "$title" --passwordbox "$prompt" 10 72 3>&1 1>&2 2>&3 ;;
    dialog)   dialog --title "$title" --insecure --passwordbox "$prompt" 10 72 3>&1 1>&2 2>&3 ;;
    fallback)
      echo -e "${BOLD}$title${NC}: $prompt"
      local val; read -rsp "> " val; echo
      echo "$val"
      ;;
  esac
}

tui_yesno() {
  [[ "$AUTO_MODE" == "true" ]] && return 0
  local title="$1" prompt="$2"
  case "$TUI" in
    whiptail) whiptail --title "$title" --yesno "$prompt" 10 72 3>&1 1>&2 2>&3; return $? ;;
    dialog)   dialog --title "$title" --yesno "$prompt" 10 72 3>&1 1>&2 2>&3; return $? ;;
    fallback)
      echo -e "${BOLD}$title${NC}: $prompt (y/n)"
      local ans; read -rp "> " ans
      [[ "$ans" =~ ^[Yy] ]]; return $?
      ;;
  esac
}

tui_menu() {
  local title="$1" prompt="$2"; shift 2
  case "$TUI" in
    whiptail) whiptail --title "$title" --menu "$prompt" 20 72 10 "$@" 3>&1 1>&2 2>&3 ;;
    dialog)   dialog --title "$title" --menu "$prompt" 20 72 10 "$@" 3>&1 1>&2 2>&3 ;;
    fallback)
      echo -e "\n${BOLD}$title${NC}: $prompt"
      local i=1
      while [[ $# -ge 2 ]]; do echo "  $1) $2"; shift 2; done
      local val; read -rp "$(i18n "Select> " "선택> ")" val; echo "$val"
      ;;
  esac
}

tui_checklist() {
  local title="$1" prompt="$2"; shift 2
  case "$TUI" in
    whiptail) whiptail --title "$title" --checklist "$prompt" 22 76 14 "$@" 3>&1 1>&2 2>&3 ;;
    dialog)   dialog --title "$title" --checklist "$prompt" 22 76 14 "$@" 3>&1 1>&2 2>&3 ;;
    fallback)
      echo -e "\n${BOLD}$title${NC}: $prompt"
      local results="" i=0
      while [[ $# -ge 3 ]]; do
        local tag="$1" desc="$2" state="$3"; shift 3
        local marker=" "; [[ "$state" == "ON" ]] && marker="*"
        echo "  [$marker] $tag — $desc"
        i=$((i+1))
      done
      echo "$(i18n "Enter items to enable, separated by spaces (e.g. cert-manager kyverno):" "활성화할 항목을 공백으로 구분 입력 (예: cert-manager kyverno):")"
      local val; read -rp "> " val; echo "$val"
      ;;
  esac
}

tui_gauge() {
  local title="$1" pct="$2" msg="$3"
  case "$TUI" in
    whiptail) echo "$pct" | whiptail --title "$title" --gauge "$msg" 7 72 "$pct" 2>/dev/null || true ;;
    dialog)   echo "$pct" | dialog --title "$title" --gauge "$msg" 7 72 "$pct" 2>/dev/null || true ;;
    fallback) printf "\r${BOLD}[%3d%%]${NC} %s" "$pct" "$msg" ;;
  esac
}

# --- Validation ---
validate_ip() {
  local ip="$1"
  [[ "$ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || return 1
  local IFS='.'; read -ra octets <<< "$ip"
  for o in "${octets[@]}"; do (( o <= 255 )) || return 1; done
}

validate_cidr() {
  local cidr="$1"
  [[ "$cidr" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}/[0-9]{1,2}$ ]] || return 1
  local ip="${cidr%/*}" prefix="${cidr#*/}"
  validate_ip "$ip" && (( prefix >= 0 && prefix <= 32 ))
}

validate_not_empty() { [[ -n "${1:-}" ]]; }

# --- Sudo caching (E2E: prompt once at start, keep alive) ---
ensure_sudo() {
  if sudo -n true 2>/dev/null; then
    log_info "$(i18n "sudo: NOPASSWD confirmed" "sudo: NOPASSWD 확인됨")"
    return 0
  fi
  # In auto mode, never prompt — fail immediately with actionable message
  if [[ "$AUTO_MODE" == "true" ]]; then
    error_msg \
      "$(i18n "sudo requires a password but auto mode is non-interactive" "sudo에 비밀번호가 필요하지만 자동 모드는 비대화형입니다")" \
      "$(i18n "sudo -n failed — NOPASSWD not configured for the current user" "sudo -n 실패 — 현재 사용자에 대해 NOPASSWD가 구성되지 않음")" \
      "$(i18n "Add NOPASSWD to sudoers: echo '${USER} ALL=(ALL) NOPASSWD:ALL' | sudo tee /etc/sudoers.d/scalex-auto" \
         "${USER} ALL=(ALL) NOPASSWD:ALL 를 /etc/sudoers.d/scalex-auto에 추가하세요")"
    return 1
  fi
  log_info "$(i18n "sudo access required. Please enter your password (one-time)." "sudo 권한이 필요합니다. 비밀번호를 입력해 주세요 (최초 1회).")"
  sudo -v || { log_error "$(i18n "sudo authentication failed" "sudo 인증 실패")"; return 1; }
  # Keep sudo timestamp alive in background
  ( while true; do sudo -n true 2>/dev/null; sleep 50; kill -0 "$$" 2>/dev/null || exit; done ) &
  SUDO_KEEPALIVE_PID=$!
  log_info "$(i18n "sudo credential cache enabled (PID: $SUDO_KEEPALIVE_PID)" "sudo 인증 캐시 활성화 (PID: $SUDO_KEEPALIVE_PID)")"
}

# --- API tunnel management (E2E: kubectl/helm access to cluster APIs) ---

# _ssh_tunnel_start: Start one SSH port-forward tunnel with retry logic.
# Usage: new_pid=$(_ssh_tunnel_start LOCAL_PORT SERVER_IP SERVER_PORT BASTION)
# Tries up to 3 times with ConnectTimeout=10; waits up to 5s per attempt for process stability.
# Emits PID on stdout on success; exits non-zero after all attempts exhausted.
# SSH stderr is captured per-attempt into a temp file.
# Each retry attempt is logged with structured key=value fields to stderr (via log_warn/log_info)
# and to the installer log file. On final failure, the last captured stderr and a classified
# error reason are recorded.
_ssh_tunnel_start() {
  local local_port="$1" server_ip="$2" server_port="$3" bastion="$4"
  local max_attempts=3 attempt=0 tpid="" stable=false
  local err_file; err_file=$(mktemp /tmp/scalex-ssh-err.XXXXXX 2>/dev/null || echo "/tmp/scalex-ssh-err.$$")
  local last_ssh_err=""
  local tunnel_spec="localhost:${local_port}->${server_ip}:${server_port}@${bastion}"

  log_info "$(i18n "Starting SSH tunnel: ${tunnel_spec} (max_attempts=${max_attempts})" \
    "SSH 터널 시작: ${tunnel_spec} (최대 시도=${max_attempts})")"

  while (( attempt < max_attempts )); do
    attempt=$((attempt + 1))
    : > "$err_file"
    log_info "$(i18n "Tunnel retry_start: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec}" \
      "터널 retry_start: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec}")"
    ssh -N \
      -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null \
      -o BatchMode=yes \
      -o ExitOnForwardFailure=yes \
      -o ServerAliveInterval=15 \
      -o ServerAliveCountMax=4 \
      -o ConnectTimeout=10 \
      -L "${local_port}:${server_ip}:${server_port}" \
      "$bastion" >/dev/null 2>"$err_file" &
    tpid=$!

    # Wait up to 5s, checking each second that the process is still alive.
    # Early exit = ExitOnForwardFailure triggered (port conflict, connect refused, etc.)
    local w=0
    stable=false
    while (( w < 5 )); do
      sleep 1; w=$((w+1))
      if ! kill -0 "$tpid" 2>/dev/null; then
        last_ssh_err=$(head -5 "$err_file" 2>/dev/null | tr '\n' ' ')
        log_warn "$(i18n "Tunnel retry_failed: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec} reason=early_exit stderr=\"${last_ssh_err:-none}\"" \
          "터널 retry_failed: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec} reason=early_exit stderr=\"${last_ssh_err:-none}\"")"
        tpid=""
        break
      fi
    done

    if [[ -n "$tpid" ]] && kill -0 "$tpid" 2>/dev/null; then
      stable=true
      break
    fi

    if (( attempt < max_attempts )); then
      local backoff=$(( attempt * 3 ))
      log_info "$(i18n "Tunnel retry_backoff: seconds=${backoff} tunnel=${tunnel_spec}" \
        "터널 retry_backoff: seconds=${backoff} tunnel=${tunnel_spec}")"
      sleep "$backoff"
    fi
  done

  rm -f "$err_file"

  if $stable && [[ -n "$tpid" ]]; then
    log_info "$(i18n "Tunnel retry_success: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec} pid=${tpid}" \
      "터널 retry_success: attempt=${attempt}/${max_attempts} tunnel=${tunnel_spec} pid=${tpid}")"
    echo "$tpid"
    return 0
  fi

  # Log structured final failure with classified error reason
  local failure_class="unknown"
  if echo "$last_ssh_err" | grep -qi "permission denied\|publickey\|authentication"; then
    failure_class="auth_failure"
  elif echo "$last_ssh_err" | grep -qi "connection refused\|no route\|network unreachable\|timed out"; then
    failure_class="network_failure"
  elif [[ -n "$last_ssh_err" ]]; then
    failure_class="ssh_error"
  fi
  log_error "$(i18n "Tunnel retry_exhausted: attempts=${max_attempts} tunnel=${tunnel_spec} failure_class=${failure_class} final_stderr=\"${last_ssh_err:-none}\"" \
    "터널 retry_exhausted: attempts=${max_attempts} tunnel=${tunnel_spec} failure_class=${failure_class} final_stderr=\"${last_ssh_err:-none}\"")"

  # Emit actionable error based on captured SSH output
  if [[ "$failure_class" == "auth_failure" ]]; then
    error_msg \
      "$(i18n "SSH authentication failed to bastion '${bastion}'" "bastion '${bastion}'에 SSH 인증 실패")" \
      "$(i18n "SSH key not authorized or wrong key path — last error: ${last_ssh_err}" "SSH 키가 승인되지 않았거나 키 경로가 잘못됨 — 마지막 오류: ${last_ssh_err}")" \
      "$(i18n "Check SSH_KEY_PATH in credentials/.env and ensure the key is in ${bastion}:~/.ssh/authorized_keys" \
         "credentials/.env의 SSH_KEY_PATH를 확인하고 키가 ${bastion}:~/.ssh/authorized_keys에 있는지 확인하세요")"
  elif [[ "$failure_class" == "network_failure" ]]; then
    error_msg \
      "$(i18n "Cannot reach bastion '${bastion}' on SSH port" "SSH 포트로 bastion '${bastion}'에 접근할 수 없음")" \
      "$(i18n "Network unreachable or SSH port closed — last error: ${last_ssh_err}" "네트워크에 접근할 수 없거나 SSH 포트가 닫혀 있음 — 마지막 오류: ${last_ssh_err}")" \
      "$(i18n "Verify bastion IP in credentials/.baremetal-init.yaml and check network connectivity" \
         "credentials/.baremetal-init.yaml의 bastion IP를 확인하고 네트워크 연결을 점검하세요")"
  elif [[ "$failure_class" == "ssh_error" ]]; then
    error_msg \
      "$(i18n "SSH tunnel to '${bastion}' failed (localhost:${local_port} → ${server_ip}:${server_port})" \
         "SSH 터널 실패 '${bastion}' (localhost:${local_port} → ${server_ip}:${server_port})")" \
      "$(i18n "SSH error: ${last_ssh_err}" "SSH 오류: ${last_ssh_err}")" \
      "$(i18n "Check ~/.ssh/config, credentials/.env, and SSH access to ${bastion}" \
         "~/.ssh/config, credentials/.env 및 ${bastion}에 대한 SSH 접근을 확인하세요")"
  fi
  return 1
}

# start_tunnel_watchdog: Background watchdog that monitors tunnel processes and auto-restarts
# any that have died. Reads per-tunnel conf files from TUNNEL_CONF_DIR.
# Conf file format: LOCAL_PORT:SERVER_IP:SERVER_PORT:BASTION:PID  (one tunnel per file)
#
# Retry behaviour (configurable via TUNNEL_WATCHDOG_MAX_RETRIES, default 3):
#   - Each dead tunnel gets up to max_retries restart attempts
#   - Exponential backoff between attempts: attempt*3 seconds (3s, 6s, 9s)
#   - SSH stderr is captured per-attempt and recorded in structured log
#   - Each retry attempt and final failure reason are logged with key=value fields
#   - A single tunnel failure does NOT abort the watchdog loop (no fail-fast)
#   - Failed tunnels will be retried again on the next watchdog cycle
#   - Dedicated watchdog log: $LOG_DIR/tunnel-watchdog.log
start_tunnel_watchdog() {
  [[ -z "$TUNNEL_CONF_DIR" || ! -d "$TUNNEL_CONF_DIR" ]] && return 0
  local parent_pid=$$
  local wd_max_retries="${TUNNEL_WATCHDOG_MAX_RETRIES:-3}"
  local watchdog_log="${LOG_DIR}/tunnel-watchdog.log"
  (
    # ── Structured logger for watchdog subprocess ──
    # Writes timestamped key=value entries to both a dedicated log file and stderr.
    # Format: ISO-8601 level=LEVEL component=tunnel-watchdog event=EVENT key=val ...
    _wd_log() {
      local level="$1" event="$2"; shift 2
      local ts; ts="$(date '+%Y-%m-%dT%H:%M:%S%z')"
      local line="${ts} level=${level} component=tunnel-watchdog event=${event} $*"
      printf '%s\n' "$line" >> "$watchdog_log" 2>/dev/null
      printf '%s\n' "$line" >&2
    }
    _wd_log INFO watchdog_started "parent_pid=${parent_pid} conf_dir=${TUNNEL_CONF_DIR} max_retries=${wd_max_retries}"
    while true; do
      sleep 10
      # Exit watchdog when parent installer exits
      if ! kill -0 "$parent_pid" 2>/dev/null; then
        _wd_log INFO watchdog_exit "reason=parent_gone parent_pid=${parent_pid}"
        exit 0
      fi
      for conf_file in "$TUNNEL_CONF_DIR"/*.conf; do
        [[ -f "$conf_file" ]] || continue
        IFS=: read -r lp sip sp bt tpid < "$conf_file" 2>/dev/null || continue
        [[ -z "$tpid" || -z "$lp" ]] && continue
        if ! kill -0 "$tpid" 2>/dev/null; then
          local cluster_label; cluster_label=$(basename "$conf_file" .conf)
          local tunnel_spec="localhost:${lp}->${sip}:${sp}@${bt}"
          _wd_log WARN tunnel_dead "cluster=${cluster_label} tunnel=${tunnel_spec} old_pid=${tpid}"
          # Retry up to max_retries with stderr capture; no fail-fast abort
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
            # Wait up to 3s for tunnel to stabilise; detect early SSH exit
            local wd_w=0
            while (( wd_w < 3 )); do
              sleep 1; wd_w=$((wd_w + 1))
              kill -0 "$wd_new_pid" 2>/dev/null || break
            done
            if kill -0 "$wd_new_pid" 2>/dev/null; then
              wd_ok=true
              break
            fi
            # Capture SSH stderr from this attempt — no fail-fast, continue to next
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
    done
  ) &
  TUNNEL_WATCHDOG_PID=$!
  log_info "$(i18n "Tunnel watchdog started (PID: $TUNNEL_WATCHDOG_PID, max_retries: $wd_max_retries, log: $watchdog_log)" \
    "터널 watchdog 시작 (PID: $TUNNEL_WATCHDOG_PID, 최대 재시도: $wd_max_retries, 로그: $watchdog_log)")"
}

# stop_tunnel_watchdog: Stop the background watchdog and clean up conf files.
stop_tunnel_watchdog() {
  if [[ -n "$TUNNEL_WATCHDOG_PID" ]]; then
    kill "$TUNNEL_WATCHDOG_PID" 2>/dev/null || true
    TUNNEL_WATCHDOG_PID=""
  fi
  if [[ -n "$TUNNEL_CONF_DIR" && -d "$TUNNEL_CONF_DIR" ]]; then
    rm -f "$TUNNEL_CONF_DIR"/*.conf 2>/dev/null || true
  fi
}

# wait_for_tunnel_port: Actively poll until local TCP port is bound (tunnel is listening).
# Returns 0 when port is listening, 1 on timeout or process death.
# Args: port pid cluster_name [max_wait_seconds]
wait_for_tunnel_port() {
  local port="$1" pid="$2" cluster_name="$3" max_wait="${4:-30}"
  local elapsed=0

  log_info "$(i18n "${cluster_name}: waiting for tunnel port ${port} to be ready (up to ${max_wait}s)..." \
    "${cluster_name}: 터널 포트 ${port} 준비 대기 중 (최대 ${max_wait}초)...")"

  while [[ $elapsed -lt $max_wait ]]; do
    # Check if local port is now listening (port check FIRST — more reliable than PID check)
    if nc -z localhost "$port" 2>/dev/null; then
      log_info "$(i18n "${cluster_name}: tunnel port ${port} is listening (${elapsed}s)" \
        "${cluster_name}: 터널 포트 ${port} 수신 준비 완료 (${elapsed}초)")"
      return 0
    elif command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":${port} "; then
      log_info "$(i18n "${cluster_name}: tunnel port ${port} is listening (${elapsed}s)" \
        "${cluster_name}: 터널 포트 ${port} 수신 준비 완료 (${elapsed}초)")"
      return 0
    fi
    # PID check as secondary signal — only fail if process is dead AND port not bound
    if ! kill -0 "$pid" 2>/dev/null; then
      # Double-check port one more time (process may have daemonized with a new PID)
      sleep 1
      if nc -z localhost "$port" 2>/dev/null || \
         { command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":${port} "; }; then
        log_info "$(i18n "${cluster_name}: tunnel port ${port} is listening (process re-parented)" \
          "${cluster_name}: 터널 포트 ${port} 수신 준비 완료 (프로세스 재배치됨)")"
        return 0
      fi
      log_error "$(i18n "${cluster_name}: SSH tunnel process (PID $pid) died before port ${port} became ready" \
        "${cluster_name}: SSH 터널 프로세스 (PID $pid) 포트 ${port} 준비 전 종료됨")"
      return 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done

  log_error "$(i18n "${cluster_name}: tunnel port ${port} not listening after ${max_wait}s — SSH tunnel failed to bind" \
    "${cluster_name}: ${max_wait}초 후에도 터널 포트 ${port} 수신 안됨 — SSH 터널 바인딩 실패")"
  return 1
}

# validate_tunnel_conf: Verify a written tunnel conf file has all required fields
# (LOCAL_PORT, SERVER_IP, SERVER_PORT, BASTION, PID) and that each is non-empty and
# syntactically valid.  Produces an actionable error message on failure.
# Args: conf_file cluster_name
# Returns: 0 if valid, 1 if any field is missing/empty/malformed
validate_tunnel_conf() {
  local conf_file="$1" cluster_name="${2:-unknown}"

  if [[ ! -f "$conf_file" ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf file not found after write: $conf_file" \
      "${cluster_name}: 쓰기 후 터널 설정 파일 없음: $conf_file")"
    return 1
  fi

  local lp sip sp bt tpid
  IFS=: read -r lp sip sp bt tpid < "$conf_file" 2>/dev/null || {
    log_error "$(i18n "${cluster_name}: tunnel conf file is unreadable or malformed: $conf_file" \
      "${cluster_name}: 터널 설정 파일 읽기 불가 또는 형식 오류: $conf_file")"
    return 1
  }

  # Validate LOCAL_PORT: must be a non-empty integer
  if [[ -z "$lp" ]] || ! [[ "$lp" =~ ^[0-9]+$ ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf LOCAL_PORT is missing or non-numeric ('${lp}') in $conf_file — re-run install.sh or check SSH connectivity" \
      "${cluster_name}: 터널 설정 LOCAL_PORT 없거나 숫자 아님 ('${lp}') in $conf_file — install.sh 재실행 또는 SSH 연결 확인")"
    return 1
  fi

  # Validate SERVER_IP: must be non-empty (may be hostname or IP)
  if [[ -z "$sip" ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf SERVER_IP is empty in $conf_file — kubeconfig may lack a valid server URL" \
      "${cluster_name}: 터널 설정 SERVER_IP가 비어 있음 in $conf_file — kubeconfig에 유효한 서버 URL이 없을 수 있습니다")"
    return 1
  fi

  # Validate SERVER_PORT: must be a non-empty integer
  if [[ -z "$sp" ]] || ! [[ "$sp" =~ ^[0-9]+$ ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf SERVER_PORT is missing or non-numeric ('${sp}') in $conf_file" \
      "${cluster_name}: 터널 설정 SERVER_PORT 없거나 숫자 아님 ('${sp}') in $conf_file")"
    return 1
  fi

  # Validate BASTION: must be non-empty
  if [[ -z "$bt" ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf BASTION is empty in $conf_file — credentials/.baremetal-init.yaml must contain at least one node with a name" \
      "${cluster_name}: 터널 설정 BASTION이 비어 있음 in $conf_file — credentials/.baremetal-init.yaml에 name 필드가 있는 노드가 있어야 합니다")"
    return 1
  fi

  # Validate PID: must be a non-empty integer
  if [[ -z "$tpid" ]] || ! [[ "$tpid" =~ ^[0-9]+$ ]]; then
    log_error "$(i18n "${cluster_name}: tunnel conf PID is missing or non-numeric ('${tpid}') in $conf_file — SSH tunnel may have failed to start" \
      "${cluster_name}: 터널 설정 PID 없거나 숫자 아님 ('${tpid}') in $conf_file — SSH 터널이 시작되지 않았을 수 있습니다")"
    return 1
  fi

  log_info "$(i18n "${cluster_name}: tunnel conf validated (port=${lp}, target=${sip}:${sp}, bastion=${bt}, pid=${tpid})" \
    "${cluster_name}: 터널 설정 검증 완료 (port=${lp}, target=${sip}:${sp}, bastion=${bt}, pid=${tpid})")"
  return 0
}

# verify_api_tunnels_ready: Verify all established SSH tunnels are alive, ports bound,
# and API servers reachable. Used in --auto mode before steps requiring kubectl/helm.
# Phase 1: Targeted per-tunnel restart via conf files (avoids full tear-down/rebuild).
# Phase 2: Verifies each cluster API server is reachable through its tunnel endpoint.
# Args: repo_dir [max_api_wait_seconds]
verify_api_tunnels_ready() {
  local repo_dir="$1" max_api_wait="${2:-120}"
  local clusters_dir="$repo_dir/_generated/clusters"

  # Phase 1: Targeted per-tunnel liveness check + restart using conf files.
  # Preferred: conf files available from setup_api_tunnels → restart only dead tunnels.
  # Fallback: full tear-down/rebuild if conf files unavailable.
  if [[ -n "$TUNNEL_CONF_DIR" && -d "$TUNNEL_CONF_DIR" ]] && \
     compgen -G "$TUNNEL_CONF_DIR/*.conf" &>/dev/null; then
    for conf_file in "$TUNNEL_CONF_DIR"/*.conf; do
      [[ -f "$conf_file" ]] || continue
      local lp sip sp bt tpid
      IFS=: read -r lp sip sp bt tpid < "$conf_file" 2>/dev/null || continue
      [[ -z "$tpid" || -z "$lp" ]] && continue
      if ! kill -0 "$tpid" 2>/dev/null; then
        log_warn "$(i18n "Tunnel localhost:${lp}→${sip}:${sp} (PID $tpid) died — restarting" \
          "터널 localhost:${lp}→${sip}:${sp} (PID $tpid) 종료됨 — 재시작")"
        local new_pid
        new_pid=$(_ssh_tunnel_start "$lp" "$sip" "$sp" "$bt") || {
          log_error "$(i18n "Failed to restart tunnel localhost:${lp}→${sip}:${sp}" \
            "터널 재시작 실패: localhost:${lp}→${sip}:${sp}")"
          return 1
        }
        printf '%s:%s:%s:%s:%s\n' "$lp" "$sip" "$sp" "$bt" "$new_pid" > "$conf_file"
        # Update API_TUNNEL_PIDS: replace old PID with new PID
        local i
        for i in "${!API_TUNNEL_PIDS[@]}"; do
          [[ "${API_TUNNEL_PIDS[$i]}" == "$tpid" ]] && API_TUNNEL_PIDS[$i]="$new_pid"
        done
        wait_for_tunnel_port "$lp" "$new_pid" "$(basename "$conf_file" .conf)" 30 || true
        log_info "$(i18n "Tunnel restarted (localhost:${lp}, PID: $new_pid)" \
          "터널 재시작됨 (localhost:${lp}, PID: $new_pid)")"
      fi
    done
  else
    # Fallback: full re-setup when conf files are not available
    local need_re_setup=false
    for tpid in "${API_TUNNEL_PIDS[@]}"; do
      if ! kill -0 "$tpid" 2>/dev/null; then
        log_warn "$(i18n "SSH tunnel (PID $tpid) died — will re-establish all tunnels" \
          "SSH 터널 (PID $tpid) 종료됨 — 모든 터널 재수립 예정")"
        need_re_setup=true
        break
      fi
    done
    if $need_re_setup; then
      log_info "$(i18n "Re-establishing SSH tunnels (full re-setup)..." "SSH 터널 전체 재수립 중...")"
      for kc in "${API_TUNNEL_BACKUPS[@]}"; do
        [[ -f "${kc}.bak" ]] && mv "${kc}.bak" "$kc"
      done
      API_TUNNEL_PIDS=()
      API_TUNNEL_BACKUPS=()
      if ! setup_api_tunnels "$repo_dir"; then
        log_error "$(i18n "Failed to re-establish SSH tunnels — cannot proceed" \
          "SSH 터널 재수립 실패 — 진행 불가")"
        return 1
      fi
    fi
  fi

  # Phase 2: Verify API server reachable through each tunnel
  local all_ready=true
  for kc in "$clusters_dir"/*/kubeconfig.yaml; do
    [[ -f "$kc" ]] || continue
    local cn; cn=$(basename "$(dirname "$kc")")
    local sv; sv=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
    local elapsed=0 interval=5 api_ready=false

    log_info "$(i18n "Verifying ${cn} API reachable through tunnel (${sv})..." \
      "${cn} API 터널 접근 확인 중 (${sv})...")"
    while [[ $elapsed -lt $max_api_wait ]]; do
      if curl -sk --connect-timeout 3 "${sv}/healthz" &>/dev/null; then
        log_info "$(i18n "${cn} API ready (${elapsed}s)" "${cn} API 준비 완료 (${elapsed}초)")"
        api_ready=true
        break
      fi
      sleep $interval
      elapsed=$((elapsed + interval))
      log_info "$(i18n "  ${cn}: still waiting for API (${elapsed}s / ${max_api_wait}s)..." \
        "  ${cn}: API 대기 중 (${elapsed}초 / ${max_api_wait}초)...")"
    done

    if ! $api_ready; then
      log_error "$(i18n "${cn} API not reachable after ${max_api_wait}s — aborting" \
        "${cn} API ${max_api_wait}초 후에도 접근 불가 — 중단")"
      all_ready=false
    fi
  done

  $all_ready || return 1
  log_info "$(i18n "All API tunnels verified ready" "모든 API 터널 준비 완료 확인")"
  return 0
}

# verify_exit_tunnel_connectivity: Final pre-exit connectivity check — verifies all
# established tunnels are reachable BEFORE cleanup_api_tunnels kills SSH processes.
# Designed to be called after bootstrap completes, while SSH tunnels are still alive.
#
# Strategy 1 (preferred): `scalex-pod tunnel status --state-file TUNNEL_STATE_FILE`
#   Reads TUNNEL_STATE_FILE, probes each cluster's endpoint, reports state=connected.
#   Retries up to max_wait seconds (default 30s) with 5s intervals.
#
# Strategy 2 (fallback): Direct port probe via nc/ss against TUNNEL_CONF_DIR entries.
#   Used when scalex-pod binary is not available or lacks the `tunnel status` subcommand.
#
# Non-fatal: logs a warning if connectivity cannot be confirmed, but never aborts the
# install.  By the time this runs, bootstrap has already succeeded — the install is
# functionally complete.
#
# Usage: verify_exit_tunnel_connectivity [max_wait_seconds]
verify_exit_tunnel_connectivity() {
  local max_wait="${1:-30}"

  # Locate scalex-pod binary (prefer release build, fall back to installed copies)
  local scalex_bin=""
  local _candidates=(
    "${REPO_DIR}/scalex-cli/target/release/scalex-pod"
    "${HOME}/.local/bin/scalex-pod"
    "${HOME}/.cargo/bin/scalex-pod"
  )
  for _c in "${_candidates[@]}"; do
    if [[ -x "$_c" ]]; then scalex_bin="$_c"; break; fi
  done
  if [[ -z "$scalex_bin" ]] && command -v scalex-pod &>/dev/null; then
    scalex_bin="$(command -v scalex-pod)"
  fi

  # Nothing to verify if TUNNEL_STATE_FILE does not exist
  if [[ ! -f "$TUNNEL_STATE_FILE" ]]; then
    log_info "$(i18n "verify_exit_tunnel_connectivity: no state file — skip" \
      "verify_exit_tunnel_connectivity: 상태 파일 없음 — 건너뜀")"
    return 0
  fi

  # ── Strategy 1: scalex-pod tunnel status ────────────────────────────────────
  local _has_tunnel_status=false
  if [[ -n "$scalex_bin" ]] && \
     "$scalex_bin" tunnel status --help &>/dev/null 2>&1; then
    _has_tunnel_status=true
  fi

  if $_has_tunnel_status; then
    log_info "$(i18n "Final tunnel connectivity check (scalex-pod tunnel status, up to ${max_wait}s)..." \
      "최종 터널 연결 확인 (scalex-pod tunnel status, 최대 ${max_wait}초)...")"
    local elapsed=0 interval=5 _out=""
    while [[ $elapsed -lt $max_wait ]]; do
      _out=$("$scalex_bin" tunnel status \
        --state-file "$TUNNEL_STATE_FILE" \
        --connect-timeout 2 2>/dev/null) || true
      if echo "$_out" | grep -q 'state=connected'; then
        log_info "$(i18n "Exit tunnel connectivity: PASSED (state=connected, ${elapsed}s)" \
          "종료 터널 연결: 통과 (state=connected, ${elapsed}초)")"
        return 0
      fi
      sleep "$interval"
      elapsed=$(( elapsed + interval ))
      log_info "$(i18n "  Tunnel status: ${_out:-pending} — retrying (${elapsed}s/${max_wait}s)..." \
        "  터널 상태: ${_out:-대기 중} — 재시도 (${elapsed}초/${max_wait}초)...")"
    done
    log_warn "$(i18n "Exit tunnel connectivity: tunnels not yet connected after ${max_wait}s (last: ${_out:-no output}) — scalex-pod CLI will auto-tunnel on next use" \
      "종료 터널 연결: ${max_wait}초 후에도 미연결 (마지막: ${_out:-출력 없음}) — scalex-pod CLI 다음 사용 시 자동 터널")"
    return 0  # Non-fatal
  fi

  # ── Strategy 2: direct port probe ───────────────────────────────────────────
  if [[ -n "$TUNNEL_CONF_DIR" && -d "$TUNNEL_CONF_DIR" ]] && \
     compgen -G "$TUNNEL_CONF_DIR/*.conf" &>/dev/null; then
    log_info "$(i18n "Final tunnel connectivity check (port probe, scalex-pod not available)..." \
      "최종 터널 연결 확인 (포트 프로브, scalex-pod 없음)...")"
    local _all_ok=true
    for _conf in "$TUNNEL_CONF_DIR"/*.conf; do
      [[ -f "$_conf" ]] || continue
      local _lp _sip _sp _bt _tpid
      IFS=: read -r _lp _sip _sp _bt _tpid < "$_conf" 2>/dev/null || continue
      [[ -z "$_lp" || -z "$_tpid" ]] && continue
      # Verify process is alive
      if ! kill -0 "$_tpid" 2>/dev/null; then
        log_warn "$(i18n "Exit check: tunnel PID $_tpid (port $_lp) is not alive" \
          "종료 확인: 터널 PID $_tpid (포트 $_lp) 비활성")"
        _all_ok=false
        continue
      fi
      # Verify port is listening (with retry up to max_wait)
      local _pelapsed=0
      local _port_ok=false
      while [[ $_pelapsed -lt $max_wait ]]; do
        if nc -z localhost "$_lp" 2>/dev/null; then
          _port_ok=true; break
        elif command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":${_lp} "; then
          _port_ok=true; break
        fi
        sleep 5; _pelapsed=$(( _pelapsed + 5 ))
      done
      if $_port_ok; then
        log_info "$(i18n "Exit check: tunnel port $_lp responding (PID $_tpid)" \
          "종료 확인: 터널 포트 $_lp 응답 중 (PID $_tpid)")"
      else
        log_warn "$(i18n "Exit check: tunnel port $_lp not responding after ${max_wait}s (PID $_tpid)" \
          "종료 확인: 터널 포트 $_lp ${max_wait}초 후 무응답 (PID $_tpid)")"
        _all_ok=false
      fi
    done
    if $_all_ok; then
      log_info "$(i18n "Exit tunnel connectivity: PASSED (all ports responding)" \
        "종료 터널 연결: 통과 (모든 포트 응답)")"
    else
      log_warn "$(i18n "Exit tunnel connectivity: some ports not responding — scalex-pod CLI will auto-tunnel on next use" \
        "종료 터널 연결: 일부 포트 무응답 — scalex-pod CLI 다음 사용 시 자동 터널")"
    fi
    return 0  # Non-fatal
  fi

  log_info "$(i18n "verify_exit_tunnel_connectivity: no conf files or scalex-pod binary — skip" \
    "verify_exit_tunnel_connectivity: 설정 파일 또는 scalex-pod 바이너리 없음 — 건너뜀")"
  return 0
}

# validate_tunnel_credentials: Pre-flight check for tunnel credentials in --auto mode.
# Validates SSH key and/or Cloudflare Tunnel credentials exist and are non-placeholder.
# Exits with code 2 if any required credential is missing or contains a placeholder value.
# Usage: validate_tunnel_credentials REPO_DIR
validate_tunnel_credentials() {
  local repo_dir="$1"
  local env_file="$repo_dir/credentials/.env"
  local bm_yaml="$repo_dir/credentials/.baremetal-init.yaml"
  local cf_json="$repo_dir/credentials/cloudflare-tunnel.json"

  # --- SSH key validation ---
  # Determine if any node uses key-based auth (sshAuthMode: key)
  local uses_key_auth=false
  if [[ -f "$bm_yaml" ]]; then
    if grep -q 'sshAuthMode:.*key' "$bm_yaml" 2>/dev/null; then
      uses_key_auth=true
    fi
  fi

  if $uses_key_auth; then
    # Resolve SSH key path from .env
    local ssh_key="$HOME/.ssh/id_ed25519"
    if [[ -f "$env_file" ]]; then
      local key_val; key_val=$(grep '^SSH_KEY_PATH=' "$env_file" 2>/dev/null | cut -d= -f2- | tr -d '"' | tr -d "'")
      # Expand tilde
      [[ -n "$key_val" ]] && ssh_key="${key_val/#\~/$HOME}"
    fi

    if [[ ! -f "$ssh_key" ]]; then
      error_msg \
        "$(i18n "Missing SSH key credential: SSH_KEY_PATH=$ssh_key" "SSH 키 자격 증명 없음: SSH_KEY_PATH=$ssh_key")" \
        "$(i18n "SSH key file does not exist — required for key-based tunnel authentication" "SSH 키 파일이 존재하지 않음 — 키 기반 터널 인증에 필요")" \
        "$(i18n "Generate a key (ssh-keygen -t ed25519) or set SSH_KEY_PATH in credentials/.env to a valid private key path" \
           "키 생성 (ssh-keygen -t ed25519) 또는 credentials/.env의 SSH_KEY_PATH를 유효한 개인 키 경로로 설정하세요")"
      exit 2
    fi

    # Verify the file is a private key (not a placeholder)
    if ! grep -q 'BEGIN.*PRIVATE KEY' "$ssh_key" 2>/dev/null; then
      error_msg \
        "$(i18n "Invalid SSH key credential: SSH_KEY_PATH=$ssh_key" "잘못된 SSH 키 자격 증명: SSH_KEY_PATH=$ssh_key")" \
        "$(i18n "File exists but does not appear to be a valid SSH private key" "파일은 존재하지만 유효한 SSH 개인 키가 아닙니다")" \
        "$(i18n "Provide a valid SSH private key at SSH_KEY_PATH in credentials/.env" \
           "credentials/.env의 SSH_KEY_PATH에 유효한 SSH 개인 키를 제공하세요")"
      exit 2
    fi
    log_info "$(i18n "SSH key credential verified: $ssh_key" "SSH 키 자격 증명 확인: $ssh_key")"

    # Ensure SSH agent is running with the key loaded (required for libvirt provider qemu+ssh)
    if ! ssh-add -l &>/dev/null; then
      if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then
        eval "$(ssh-agent -s)" &>/dev/null
        log_info "$(i18n "Started SSH agent (PID: $SSH_AGENT_PID)" "SSH 에이전트 시작 (PID: $SSH_AGENT_PID)")"
      fi
      ssh-add "$ssh_key" &>/dev/null 2>&1 || true
      log_info "$(i18n "SSH key loaded into agent: $ssh_key" "SSH 키를 에이전트에 로드: $ssh_key")"
    else
      log_info "$(i18n "SSH agent already running with key loaded" "SSH 에이전트 이미 실행 중 (키 로드 완료)")"
    fi
  fi

  # --- Cloudflare Tunnel credential validation ---
  # Only validate if cloudflare-tunnel.json is referenced (credentials_file set in secrets.yaml)
  local secrets_yaml="$repo_dir/credentials/secrets.yaml"
  local cf_enabled=false
  if [[ -f "$secrets_yaml" ]] && grep -q 'credentials_file:' "$secrets_yaml" 2>/dev/null; then
    local cf_path; cf_path=$(grep 'credentials_file:' "$secrets_yaml" | head -1 | awk '{print $2}' | tr -d '"')
    # Non-empty, non-placeholder path means CF Tunnel is configured
    if [[ -n "$cf_path" && "$cf_path" != '""' && "$cf_path" != "''" ]]; then
      cf_enabled=true
    fi
  fi
  # Also treat it as enabled if cloudflare-tunnel.json is present
  [[ -f "$cf_json" ]] && cf_enabled=true

  if $cf_enabled; then
    if [[ ! -f "$cf_json" ]]; then
      error_msg \
        "$(i18n "Missing Cloudflare Tunnel credential: credentials/cloudflare-tunnel.json" \
           "Cloudflare Tunnel 자격 증명 없음: credentials/cloudflare-tunnel.json")" \
        "$(i18n "CF Tunnel is configured (secrets.yaml references it) but the credential file is absent" \
           "CF Tunnel이 구성되었지만 (secrets.yaml 참조) 자격 증명 파일이 없습니다")" \
        "$(i18n "Download cloudflare-tunnel.json from the Cloudflare dashboard and place it at credentials/cloudflare-tunnel.json" \
           "Cloudflare 대시보드에서 cloudflare-tunnel.json을 다운로드하여 credentials/cloudflare-tunnel.json에 배치하세요")"
      exit 2
    fi

    # Validate required fields are present and non-placeholder
    local cf_account cf_secret cf_tunnel_id
    cf_account=$(python3 -c "import json,sys; d=json.load(open('$cf_json')); print(d.get('AccountTag',''))" 2>/dev/null || true)
    cf_secret=$(python3 -c "import json,sys; d=json.load(open('$cf_json')); print(d.get('TunnelSecret',''))" 2>/dev/null || true)
    cf_tunnel_id=$(python3 -c "import json,sys; d=json.load(open('$cf_json')); print(d.get('TunnelID',''))" 2>/dev/null || true)

    local -A _cf_fields
    _cf_fields=( ["AccountTag"]="$cf_account" ["TunnelSecret"]="$cf_secret" ["TunnelID"]="$cf_tunnel_id" )
    local _cf_field_order=( "AccountTag" "TunnelSecret" "TunnelID" )
    local _fn _fv
    for _fn in "${_cf_field_order[@]}"; do
      _fv="${_cf_fields[$_fn]}"
      if [[ -z "$_fv" ]] || echo "$_fv" | grep -qiE '<YOUR_|changeme|placeholder'; then
        error_msg \
          "$(i18n "Missing or invalid Cloudflare Tunnel credential: $_fn in credentials/cloudflare-tunnel.json" \
             "Cloudflare Tunnel 자격 증명 없음/잘못됨: credentials/cloudflare-tunnel.json의 $_fn")" \
          "$(i18n "Field '$_fn' is empty or contains a placeholder value" \
             "'$_fn' 필드가 비어 있거나 플레이스홀더 값을 포함합니다")" \
          "$(i18n "Edit credentials/cloudflare-tunnel.json and set a real value for '$_fn'" \
             "credentials/cloudflare-tunnel.json을 편집하여 '$_fn'에 실제 값을 설정하세요")"
        exit 2
      fi
    done
    log_info "$(i18n "Cloudflare Tunnel credentials verified (TunnelID: $cf_tunnel_id)" \
      "Cloudflare Tunnel 자격 증명 확인 완료 (TunnelID: $cf_tunnel_id)")"
  fi

  return 0
}

# write_tunnel_config: Record tunnel transport details to TUNNEL_STATE_FILE after
# successful tunnel establishment. Idempotent — updates an existing cluster entry or
# appends a new one. Called by setup_api_tunnels (SSH bastion) and cleanup_api_tunnels
# Phase 2 (CF Tunnel domain rewrite).
#
# Args: cluster_name transport_type endpoint auth_method
#   transport_type : "ssh_bastion" | "cf_tunnel"
#   endpoint       : e.g. "localhost:16443" or "https://api.cluster.example.com"
#   auth_method    : "ssh_key" | "ssh_default_key" | "cf_token"
#
# File format (YAML):
#   clusters:
#     <cluster_name>:
#       transport_type: ssh_bastion
#       endpoint: "localhost:16443"
#       auth_method: ssh_key
#       established_at: "2026-03-18T12:00:00Z"
write_tunnel_config() {
  local cluster_name="$1" transport_type="$2" endpoint="$3" auth_method="$4"
  local ts; ts=$(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date '+%Y-%m-%dT%H:%M:%SZ')

  # Validate required args — produce actionable error if missing
  if [[ -z "$cluster_name" || -z "$transport_type" || -z "$endpoint" || -z "$auth_method" ]]; then
    error_msg \
      "$(i18n "write_tunnel_config: missing required argument" "write_tunnel_config: 필수 인수 누락")" \
      "$(i18n "cluster_name, transport_type, endpoint, auth_method are all required" "cluster_name, transport_type, endpoint, auth_method 모두 필수")" \
      "$(i18n "Check install.sh for the caller and ensure all 4 args are supplied" "install.sh 호출부를 확인하고 4개 인수를 모두 제공하세요")"
    return 1
  fi

  # Validate transport_type is one of the explicit allowed values
  case "$transport_type" in
    ssh_bastion|cf_tunnel) ;;
    *)
      error_msg \
        "$(i18n "write_tunnel_config: unknown transport_type '${transport_type}'" "write_tunnel_config: 알 수 없는 transport_type '${transport_type}'")" \
        "$(i18n "transport_type must be 'ssh_bastion' or 'cf_tunnel' — never silently inferred" "transport_type은 'ssh_bastion' 또는 'cf_tunnel'이어야 합니다 — 묵시적 추론 불가")" \
        "$(i18n "Pass the explicit transport_type when calling write_tunnel_config" "write_tunnel_config 호출 시 transport_type을 명시적으로 전달하세요")"
      return 1
      ;;
  esac

  # Ensure parent directory exists and is secure
  mkdir -p "$(dirname "$TUNNEL_STATE_FILE")"
  chmod 700 "$(dirname "$TUNNEL_STATE_FILE")" 2>/dev/null || true

  # Create the file with a header if it does not exist
  if [[ ! -f "$TUNNEL_STATE_FILE" ]]; then
    cat > "$TUNNEL_STATE_FILE" << 'TSEOF'
# ScaleX tunnel state — written by install.sh --auto
# transport_type: ssh_bastion | cf_tunnel
# auth_method:    ssh_key | ssh_default_key | cf_token
---
clusters: {}
TSEOF
    chmod 600 "$TUNNEL_STATE_FILE"
  fi

  # Idempotent YAML update via Python: upsert cluster entry under clusters:
  # Uses PyYAML if available, otherwise falls back to a safe awk-based rewrite.
  if python3 -c "import yaml" 2>/dev/null; then
    python3 - "$TUNNEL_STATE_FILE" "$cluster_name" "$transport_type" "$endpoint" "$auth_method" "$ts" << 'PYEOF'
import sys, yaml, os

state_file, cluster_name, transport_type, endpoint, auth_method, ts = sys.argv[1:]

with open(state_file) as f:
    doc = yaml.safe_load(f) or {}

if 'clusters' not in doc or not isinstance(doc['clusters'], dict):
    doc['clusters'] = {}

doc['clusters'][cluster_name] = {
    'transport_type': transport_type,
    'endpoint': endpoint,
    'auth_method': auth_method,
    'established_at': ts,
}

# Write atomically via a temp file in the same directory
tmp = state_file + '.tmp'
with open(tmp, 'w') as f:
    f.write("# ScaleX tunnel state — written by install.sh --auto\n")
    f.write("# transport_type: ssh_bastion | cf_tunnel\n")
    f.write("# auth_method:    ssh_key | ssh_default_key | cf_token\n")
    f.write("---\n")
    yaml.dump(doc, f, default_flow_style=False, sort_keys=True)
os.chmod(tmp, 0o600)
os.replace(tmp, state_file)
PYEOF
    if [[ $? -ne 0 ]]; then
      error_msg \
        "$(i18n "Failed to write tunnel state for cluster '${cluster_name}'" "클러스터 '${cluster_name}' 터널 상태 쓰기 실패")" \
        "$(i18n "Python YAML write to ${TUNNEL_STATE_FILE} failed" "Python YAML을 ${TUNNEL_STATE_FILE}에 쓰기 실패")" \
        "$(i18n "Check disk space and permissions on $(dirname "$TUNNEL_STATE_FILE")" "$(dirname "$TUNNEL_STATE_FILE") 의 디스크 공간과 권한을 확인하세요")"
      return 1
    fi
  else
    # Fallback: append a new cluster block if the cluster is not already present.
    # This avoids duplicates on re-run while keeping the file parseable.
    if grep -q "^  ${cluster_name}:" "$TUNNEL_STATE_FILE" 2>/dev/null; then
      # Cluster already present — replace its 4 fields using sed (line-by-line safe update)
      # We use a unique marker approach: remove the old block then re-append.
      python3 - "$TUNNEL_STATE_FILE" "$cluster_name" "$transport_type" "$endpoint" "$auth_method" "$ts" << 'SEDEOF' 2>/dev/null || true
import sys, re

state_file, cluster_name, transport_type, endpoint, auth_method, ts = sys.argv[1:]
with open(state_file) as f:
    content = f.read()

# Remove the existing cluster block (4 lines under "  cluster_name:")
block_re = re.compile(
    r'^  ' + re.escape(cluster_name) + r':.*?\n(?:    \S.*?\n){0,6}',
    re.MULTILINE
)
content = block_re.sub('', content)

new_block = (
    f"  {cluster_name}:\n"
    f"    transport_type: {transport_type}\n"
    f"    endpoint: \"{endpoint}\"\n"
    f"    auth_method: {auth_method}\n"
    f"    established_at: \"{ts}\"\n"
)
# Insert before the final newline of the clusters: section
if 'clusters: {}' in content:
    content = content.replace('clusters: {}', f'clusters:\n{new_block}')
else:
    content = content.rstrip('\n') + '\n' + new_block

with open(state_file + '.tmp', 'w') as f:
    f.write(content)
import os; os.chmod(state_file + '.tmp', 0o600); os.replace(state_file + '.tmp', state_file)
SEDEOF
    else
      # Append new cluster block at end of file
      {
        printf '  %s:\n' "$cluster_name"
        printf '    transport_type: %s\n' "$transport_type"
        printf '    endpoint: "%s"\n' "$endpoint"
        printf '    auth_method: %s\n' "$auth_method"
        printf '    established_at: "%s"\n' "$ts"
      } >> "$TUNNEL_STATE_FILE"
      chmod 600 "$TUNNEL_STATE_FILE"
    fi
  fi

  log_info "$(i18n "Tunnel config written: ${cluster_name} transport=${transport_type} endpoint=${endpoint} auth=${auth_method}" \
    "터널 설정 저장됨: ${cluster_name} transport=${transport_type} endpoint=${endpoint} auth=${auth_method}")"
  log_info "$(i18n "  → ${TUNNEL_STATE_FILE}" "  → ${TUNNEL_STATE_FILE}")"
}

# Sets up SSH port-forward tunnels through a bastion node to reach cluster API servers.
# Only needed when installer can't directly reach VM IPs (e.g., remote via Tailscale).
# Transport selection is EXPLICIT:
#   - If api_endpoint configured AND reachable  → CF Tunnel transport (log + skip SSH tunnel)
#   - If api_endpoint configured but unreachable → SSH bastion transport (with warning)
#   - If no api_endpoint configured             → SSH bastion transport
#   - Idempotent: if a live tunnel already exists for a cluster, it is reused
setup_api_tunnels() {
  local repo_dir="$1"
  local clusters_dir="$repo_dir/_generated/clusters"
  local bm_yaml="$repo_dir/credentials/.baremetal-init.yaml"
  local local_port=16443

  [[ -d "$clusters_dir" ]] || { log_warn "$(i18n "Cluster directory not found: $clusters_dir" "클러스터 디렉토리 없음: $clusters_dir")"; return 0; }

  # Find bastion: first node from .baremetal-init.yaml (handles quoted and unquoted YAML)
  local bastion_name="" bastion_target=""
  if [[ -f "$bm_yaml" ]]; then
    bastion_name=$(grep -m1 'name:' "$bm_yaml" | sed "s/.*name:[[:space:]]*['\"]\\{0,1\\}//; s/['\"].*//; s/[[:space:]]*$//")
    # Use SSH config hostname (generated by generate_ssh_config)
    bastion_target="$bastion_name"
  fi

  if [[ -z "$bastion_target" ]]; then
    log_warn "$(i18n "Cannot find bastion node — attempting direct access without tunnel" "bastion 노드를 찾을 수 없음 — 터널 없이 직접 접근 시도")"
    return 0
  fi

  # Determine SSH auth_method: ssh_key (custom path from .env) or ssh_default_key
  local ssh_auth_method="ssh_default_key"
  local env_file="$repo_dir/credentials/.env"
  if [[ -f "$env_file" ]]; then
    local _key_val; _key_val=$(grep '^SSH_KEY_PATH=' "$env_file" 2>/dev/null | cut -d= -f2- | tr -d '"' | tr -d "'")
    [[ -n "$_key_val" ]] && ssh_auth_method="ssh_key"
  fi

  # Set up tunnel conf dir up front so it's available for idempotency checks
  TUNNEL_CONF_DIR="${TUNNEL_CONF_DIR:-$INSTALLER_DIR/tunnels}"
  mkdir -p "$TUNNEL_CONF_DIR"

  # Extract api_endpoint mappings from k8s-clusters.yaml for explicit transport selection
  local api_endpoints_map=""
  local _clusters_yaml="$repo_dir/config/k8s-clusters.yaml"
  if [[ -f "$_clusters_yaml" ]]; then
    api_endpoints_map=$(python3 -c "
import yaml, sys
try:
    with open('$_clusters_yaml') as f:
        data = yaml.safe_load(f)
    for c in data.get('config', {}).get('clusters', []):
        name = c.get('cluster_name', '')
        ep = c.get('api_endpoint', '')
        if name and ep:
            print(f'{name} {ep}')
except: pass
" 2>/dev/null || true)
  fi

  for kc in "$clusters_dir"/*/kubeconfig.yaml; do
    [[ -f "$kc" ]] || continue
    local cluster_name; cluster_name=$(basename "$(dirname "$kc")")

    # --- Idempotency: restore kubeconfig if stuck at localhost URL from crashed run ---
    # If kubeconfig server URL is localhost (i.e., a previous run rewrote it but cleanup
    # didn't run), restore from .bak to get the real CP IP before proceeding.
    local _raw_url; _raw_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
    local _raw_ip; _raw_ip=$(echo "$_raw_url" | sed 's|https://||; s|:.*||')
    if [[ "$_raw_ip" == "127.0.0.1" || "$_raw_ip" == "localhost" ]]; then
      if [[ -f "${kc}.bak" ]]; then
        log_info "$(i18n "${cluster_name}: kubeconfig has stale tunnel URL — restoring from backup for re-run" \
          "${cluster_name}: kubeconfig에 낡은 터널 URL — 재실행을 위해 백업에서 복원")"
        cp "${kc}.bak" "$kc"
        rm -f "${kc}.bak"
      else
        # No backup — try SDI state for real IP
        local _sdi_ip; _sdi_ip=$(python3 -c "
import json, sys
try:
    with open('${repo_dir}/_generated/sdi/sdi-state.json') as f:
        pools = json.load(f)
    if not isinstance(pools, list): pools = [pools]
    for pool in pools:
        for node in pool.get('nodes', []):
            if node.get('node_name','').startswith('${cluster_name}-cp'):
                print(node['ip']); sys.exit(0)
except: pass
" 2>/dev/null || true)
        if [[ -n "$_sdi_ip" ]]; then
          log_info "$(i18n "${cluster_name}: kubeconfig has stale tunnel URL — rewriting to SDI IP ${_sdi_ip}" \
            "${cluster_name}: kubeconfig에 낡은 터널 URL — SDI IP ${_sdi_ip}로 재작성")"
          sed -i "s|${_raw_url}|https://${_sdi_ip}:6443|g" "$kc"
        else
          log_warn "$(i18n "${cluster_name}: kubeconfig has stale tunnel URL (localhost) and no backup — tunnel may target wrong host" \
            "${cluster_name}: kubeconfig에 낡은 터널 URL이 있고 백업도 없음 — 터널이 잘못된 호스트를 대상으로 할 수 있음")"
        fi
      fi
    fi

    # Extract server URL from kubeconfig (now fresh after idempotency restore)
    local server_url; server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
    local server_ip; server_ip=$(echo "$server_url" | sed 's|https://||; s|:.*||')
    local server_port; server_port=$(echo "$server_url" | sed 's|.*:||')
    [[ -z "$server_port" ]] && server_port=6443

    # --- Explicit transport selection ---
    # Look up api_endpoint for this cluster (from k8s-clusters.yaml)
    local cluster_api_endpoint=""
    if [[ -n "$api_endpoints_map" ]]; then
      cluster_api_endpoint=$(echo "$api_endpoints_map" | awk -v cn="$cluster_name" '$1==cn{print $2; exit}')
    fi

    if [[ -n "$cluster_api_endpoint" ]]; then
      # Explicit CF Tunnel transport selected (api_endpoint configured in k8s-clusters.yaml)
      if [[ "$(curl -sk --connect-timeout 5 "${cluster_api_endpoint}/healthz" 2>/dev/null)" == "ok" ]]; then
        log_info "$(i18n "${cluster_name}: TRANSPORT=cf_tunnel (api_endpoint=${cluster_api_endpoint}) — CF Tunnel reachable, no SSH bastion needed" \
          "${cluster_name}: TRANSPORT=cf_tunnel (api_endpoint=${cluster_api_endpoint}) — CF 터널 접근 가능, SSH bastion 불필요")"
        continue
      else
        log_info "$(i18n "${cluster_name}: TRANSPORT=ssh_bastion — api_endpoint ${cluster_api_endpoint} configured but not yet reachable (CF Tunnel not deployed yet); falling back to SSH bastion" \
          "${cluster_name}: TRANSPORT=ssh_bastion — api_endpoint ${cluster_api_endpoint} 설정됐지만 아직 접근 불가 (CF 터널 미배포); SSH bastion으로 폴백")"
      fi
    else
      # Explicit SSH bastion transport (no api_endpoint configured)
      # Check direct connectivity first (same-LAN scenario)
      if curl -sk --connect-timeout 3 "${server_url}/healthz" &>/dev/null; then
        log_info "$(i18n "${cluster_name}: TRANSPORT=direct (API reachable at ${server_url}) — SSH bastion not needed" \
          "${cluster_name}: TRANSPORT=direct (API ${server_url}에서 접근 가능) — SSH bastion 불필요")"
        continue
      fi
      log_info "$(i18n "${cluster_name}: TRANSPORT=ssh_bastion (no api_endpoint configured; direct API unreachable)" \
        "${cluster_name}: TRANSPORT=ssh_bastion (api_endpoint 미설정; 직접 API 접근 불가)")"
    fi

    # --- Idempotency: reuse existing live tunnel if available ---
    local _conf_file="$TUNNEL_CONF_DIR/${cluster_name}.conf"
    if [[ -f "$_conf_file" ]]; then
      local _elp _esip _esp _ebt _epid
      IFS=: read -r _elp _esip _esp _ebt _epid < "$_conf_file" 2>/dev/null || true
      if [[ -n "$_epid" ]] && kill -0 "$_epid" 2>/dev/null && \
         [[ "$_esip" == "$server_ip" && "$_esp" == "$server_port" ]]; then
        log_info "$(i18n "${cluster_name}: SSH tunnel already running (PID=${_epid}, localhost:${_elp} → ${_esip}:${_esp}) — reusing" \
          "${cluster_name}: SSH 터널 이미 실행 중 (PID=${_epid}, localhost:${_elp} → ${_esip}:${_esp}) — 재사용")"
        API_TUNNEL_PIDS+=("$_epid")
        local_port="$_elp"
        # Ensure kubeconfig points to the running tunnel
        if ! grep -q "localhost:${_elp}" "$kc" 2>/dev/null; then
          cp "$kc" "${kc}.bak"
          API_TUNNEL_BACKUPS+=("$kc")
          sed -i "s|${server_url}|https://localhost:${_elp}|g" "$kc"
        else
          # Kubeconfig already points to tunnel — still track for cleanup
          [[ ! -f "${kc}.bak" ]] && cp "$kc" "${kc}.bak"
          API_TUNNEL_BACKUPS+=("$kc")
        fi
        write_tunnel_config "$cluster_name" "ssh_bastion" "localhost:${_elp}" "$ssh_auth_method"
        local_port=$(( _elp + 1 ))
        continue
      fi
    fi

    # --- Find next available port (skip ports already in use) ---
    while nc -z localhost "$local_port" 2>/dev/null || \
          (command -v ss &>/dev/null && ss -tlnp 2>/dev/null | grep -q ":${local_port} "); do
      log_info "$(i18n "Port ${local_port} already in use — trying $((local_port + 1))" \
        "포트 ${local_port} 이미 사용 중 — $((local_port + 1)) 시도")"
      local_port=$((local_port + 1))
    done

    # Set up SSH tunnel: localhost:<port> → <vm-ip>:6443 via bastion
    # Transport is EXPLICIT: ssh_bastion (logged above before this point)
    log_info "$(i18n "${cluster_name}: establishing SSH bastion tunnel (localhost:${local_port} → ${server_ip}:${server_port} via ${bastion_target})" \
      "${cluster_name}: SSH bastion 터널 설정 중 (localhost:${local_port} → ${server_ip}:${server_port} via ${bastion_target})")"
    local tpid
    tpid=$(_ssh_tunnel_start "$local_port" "$server_ip" "$server_port" "$bastion_target") || {
      log_error "$(i18n "${cluster_name}: SSH tunnel failed after retries (localhost:${local_port} → ${server_ip}:${server_port})" \
        "${cluster_name}: SSH 터널 재시도 후 실패 (localhost:${local_port} → ${server_ip}:${server_port})")"
      return 1
    }

    # Wait for tunnel port to be bound (more reliable than static sleep)
    if ! wait_for_tunnel_port "$local_port" "$tpid" "$cluster_name" 30; then
      log_error "$(i18n "${cluster_name}: SSH tunnel failed to bind port ${local_port}" \
        "${cluster_name}: SSH 터널 포트 ${local_port} 바인딩 실패")"
      kill "$tpid" 2>/dev/null || true
      return 1
    fi
    API_TUNNEL_PIDS+=("$tpid")

    # Persist tunnel config for watchdog (LOCAL_PORT:SERVER_IP:SERVER_PORT:BASTION:PID)
    # Watchdog reads these files to detect and restart dead tunnels automatically.
    printf '%s:%s:%s:%s:%s\n' "$local_port" "$server_ip" "$server_port" "$bastion_target" "$tpid" \
      > "$TUNNEL_CONF_DIR/${cluster_name}.conf"
    # Validate the written conf file immediately — catch partial writes or empty fields early
    validate_tunnel_conf "$TUNNEL_CONF_DIR/${cluster_name}.conf" "$cluster_name" || {
      log_error "$(i18n "${cluster_name}: tunnel conf validation failed — aborting tunnel setup for this cluster" \
        "${cluster_name}: 터널 설정 검증 실패 — 이 클러스터의 터널 설정 중단")"
      kill "$tpid" 2>/dev/null || true
      return 1
    }

    # Backup kubeconfig and rewrite server URL to tunnel
    cp "$kc" "${kc}.bak"
    API_TUNNEL_BACKUPS+=("$kc")
    sed -i "s|${server_url}|https://localhost:${local_port}|g" "$kc"

    # Verify API is reachable through tunnel (wait up to 60s, restart tunnel on death)
    local api_ok=false
    for _try in $(seq 1 12); do
      if curl -sk --connect-timeout 3 "https://localhost:${local_port}/healthz" &>/dev/null; then
        api_ok=true
        break
      fi
      # Check tunnel is still alive; use _ssh_tunnel_start() for reliable restart
      if ! kill -0 "$tpid" 2>/dev/null; then
        log_warn "$(i18n "${cluster_name}: tunnel died during API verify — restarting..." "${cluster_name}: API 확인 중 터널 종료 — 재시작...")"
        tpid=$(_ssh_tunnel_start "$local_port" "$server_ip" "$server_port" "$bastion_target") || {
          log_error "$(i18n "${cluster_name}: tunnel restart failed" "${cluster_name}: 터널 재시작 실패")"
          break
        }
        API_TUNNEL_PIDS[-1]="$tpid"
        # Update conf file with new PID
        printf '%s:%s:%s:%s:%s\n' "$local_port" "$server_ip" "$server_port" "$bastion_target" "$tpid" \
          > "$TUNNEL_CONF_DIR/${cluster_name}.conf"
        # Validate updated conf — warn on failure but do not abort (api_ok=false will surface it)
        validate_tunnel_conf "$TUNNEL_CONF_DIR/${cluster_name}.conf" "$cluster_name" || \
          log_warn "$(i18n "${cluster_name}: restarted tunnel conf validation failed — watchdog may not be able to manage this tunnel" \
            "${cluster_name}: 재시작된 터널 설정 검증 실패 — watchdog이 이 터널을 관리하지 못할 수 있습니다")"
        # Log if port doesn't bind but don't abort here — api_ok=false will surface the failure
        wait_for_tunnel_port "$local_port" "$tpid" "$cluster_name" 20 || \
          log_warn "$(i18n "${cluster_name}: restarted tunnel port ${local_port} not bound — API check will likely fail" \
            "${cluster_name}: 재시작된 터널 포트 ${local_port} 바인딩 안됨 — API 확인 실패 예상")"
      fi
      sleep 3
    done
    if $api_ok; then
      log_info "${cluster_name}: kubeconfig → localhost:${local_port} (API verified)"
    else
      log_warn "$(i18n "${cluster_name}: API not reachable through tunnel — continuing anyway" "${cluster_name}: 터널을 통한 API 접근 불가 — 계속 진행")"
      log_info "${cluster_name}: kubeconfig → localhost:${local_port}"
    fi

    # Record established SSH bastion tunnel to persistent state file.
    # scalex-pod dash --headless reads this file to discover active tunnels.
    # Called after port is bound (not on direct-access skip above) so the
    # entry is only written when a tunnel was actually established.
    write_tunnel_config \
      "$cluster_name" \
      "ssh_bastion" \
      "localhost:${local_port}" \
      "$ssh_auth_method"

    local_port=$((local_port + 1))
  done

  # Start background watchdog to keep tunnels alive during long operations
  # (cluster init ~25min, bootstrap ~15min — tunnels may die without watchdog)
  if [[ ${#API_TUNNEL_PIDS[@]} -gt 0 ]]; then
    start_tunnel_watchdog
  fi
}

cleanup_api_tunnels() {
  # Stop watchdog first (before killing tunnel PIDs it monitors)
  stop_tunnel_watchdog

  # Kill tunnel processes (bootstrap is done, scalex-pod CLI will auto-tunnel when needed)
  for pid in "${API_TUNNEL_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  if [[ ${#API_TUNNEL_PIDS[@]} -gt 0 ]]; then
    log_info "$(i18n "SSH API tunnel cleanup complete (${#API_TUNNEL_PIDS[@]} tunnels)" "SSH API 터널 ${#API_TUNNEL_PIDS[@]}개 정리 완료")"
  fi
  API_TUNNEL_PIDS=()

  # Permanently rewrite kubeconfigs with actual CP IPs (NOT restore 127.0.0.1 backups).
  # This ensures kubeconfigs are usable from the bastion via scalex-pod CLI auto-tunneling.
  for kc in "${API_TUNNEL_BACKUPS[@]}"; do
    if [[ -f "${kc}.bak" ]]; then
      # Extract the original CP IP from the backup (which has the real server URL)
      local orig_url; orig_url=$(grep 'server:' "${kc}.bak" | head -1 | awk '{print $2}' | tr -d '"')
      local cp_ip; cp_ip=$(echo "$orig_url" | sed 's|https://||; s|:.*||')
      local cp_port; cp_port=$(echo "$orig_url" | sed 's|.*:||')
      [[ -z "$cp_port" ]] && cp_port=6443

      if [[ "$cp_ip" == "127.0.0.1" ]]; then
        # Backup still has localhost — look up actual CP IP from SDI state
        local cluster_name; cluster_name=$(basename "$(dirname "$kc")")
        local _sdi_state_path="${REPO_DIR:-.}/_generated/sdi/sdi-state.json"
        local sdi_ip; sdi_ip=$(python3 -c "
import json, sys
try:
    with open('${_sdi_state_path}') as f:
        pools = json.load(f)
    if not isinstance(pools, list): pools = [pools]
    for pool in pools:
        for node in pool.get('nodes', []):
            if node.get('node_name','').startswith('${cluster_name}-cp'):
                print(node['ip']); sys.exit(0)
except: pass
" 2>/dev/null)
        if [[ -n "$sdi_ip" ]]; then
          cp_ip="$sdi_ip"
        fi
      fi

      # Write kubeconfig with the actual CP IP (reachable via SSH tunnel by scalex-pod CLI)
      if [[ "$cp_ip" != "127.0.0.1" ]]; then
        # Get the current (tunnel) URL from the modified kubeconfig
        local tunnel_url; tunnel_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
        sed -i "s|${tunnel_url}|https://${cp_ip}:${cp_port}|g" "$kc"
        log_info "kubeconfig $(basename "$(dirname "$kc")"): server → ${cp_ip}:${cp_port}"
      else
        # Fallback: restore backup as-is if IP lookup failed
        mv "${kc}.bak" "$kc"
      fi
      rm -f "${kc}.bak"
    fi
  done
  API_TUNNEL_BACKUPS=()

  # --- Phase 2: Rewrite kubeconfigs with domain URLs (if api_endpoint configured) ---
  # When Cloudflare Tunnel provides stable domain URLs, kubeconfigs should use them
  # so they work from any network without SSH tunnels.
  local _repo_root="${REPO_DIR:-.}"
  local clusters_yaml="${_repo_root}/config/k8s-clusters.yaml"
  if [[ -f "$clusters_yaml" ]]; then
    local clusters_dir="${_repo_root}/_generated/clusters"
    # Extract cluster_name + api_endpoint pairs using Python (safe YAML parsing)
    local pairs; pairs=$(python3 -c "
import yaml, sys
try:
    with open('$clusters_yaml') as f:
        data = yaml.safe_load(f)
    for c in data.get('config', {}).get('clusters', []):
        name = c.get('cluster_name', '')
        ep = c.get('api_endpoint', '')
        if name and ep:
            print(f'{name} {ep}')
except Exception as e:
    print(f'ERROR: {e}', file=sys.stderr)
" 2>/dev/null)
    while IFS=' ' read -r current_cluster current_endpoint; do
      [[ -z "$current_cluster" || -z "$current_endpoint" ]] && continue
      local kc_path="${clusters_dir}/${current_cluster}/kubeconfig.yaml"
      if [[ -f "$kc_path" ]]; then
        # Save original kubeconfig with VM IP for fallback (only if not already saved)
        if [[ ! -f "${kc_path}.original" ]]; then
          cp "$kc_path" "${kc_path}.original"
        fi

        # Probe domain URL — CF Tunnel needs time for ArgoCD to sync the cloudflared deployment.
        # Default: 180s (36 retries × 5s). Override with SCALEX_CF_TUNNEL_WAIT env var.
        local _cf_wait="${SCALEX_CF_TUNNEL_WAIT:-180}"
        local _cf_retries=$(( (_cf_wait + 4) / 5 ))  # ceil(wait/5)
        log_info "$(i18n "Probing domain endpoint for ${current_cluster}: ${current_endpoint} (up to ${_cf_wait}s)" \
          "${current_cluster} 도메인 엔드포인트 확인 중: ${current_endpoint} (최대 ${_cf_wait}초)")"
        # Determine CF Tunnel auth_method: cf_token (credentials.json present) or cf_token_missing
        local cf_auth_method="cf_token"
        if [[ ! -f "credentials/cloudflare-tunnel.json" ]]; then
          cf_auth_method="cf_token_missing"
          error_msg \
            "$(i18n "CF Tunnel credentials not found for ${current_cluster}" "CF 터널 자격증명 없음: ${current_cluster}")" \
            "$(i18n "credentials/cloudflare-tunnel.json is required for CF Tunnel auth" "CF 터널 인증에 credentials/cloudflare-tunnel.json이 필요합니다")" \
            "$(i18n "Run: scalex-pod secrets apply or copy credentials/cloudflare-tunnel.json from your CF dashboard" "실행: scalex-pod secrets apply 또는 CF 대시보드에서 credentials/cloudflare-tunnel.json 복사")"
        fi

        if [[ "$(curl -sk --connect-timeout 5 --retry "$_cf_retries" --retry-delay 5 \
          --retry-max-time "$_cf_wait" \
          "${current_endpoint}/healthz" 2>/dev/null)" == "ok" ]]; then
          # Rewrite kubeconfig server field to domain URL
          local current_url; current_url=$(grep 'server:' "$kc_path" | head -1 | awk '{print $2}' | tr -d '"')
          sed -i "s|${current_url}|${current_endpoint}|g" "$kc_path"
          log_info "$(i18n "kubeconfig ${current_cluster}: server → ${current_endpoint}" \
            "kubeconfig ${current_cluster}: server → ${current_endpoint}")"

          # Record CF Tunnel state for scalex-pod CLI (overrides any prior ssh_bastion entry
          # for this cluster — CF Tunnel is the preferred persistent transport).
          write_tunnel_config \
            "$current_cluster" \
            "cf_tunnel" \
            "$current_endpoint" \
            "$cf_auth_method"
        else
          log_warn "$(i18n "Domain endpoint unreachable for ${current_cluster} — keeping CP IP" \
            "${current_cluster} 도메인 엔드포인트 접근 불가 — CP IP 유지")"
        fi
      fi
    done <<< "$pairs"
  fi
}

# --- SSH config generation ---
# Generates ~/.ssh/config entries from .baremetal-init.yaml for libvirt qemu+ssh:// access.
# Reads node topology and creates ProxyJump entries for non-direct nodes.
generate_ssh_config() {
  local repo_dir="${1:-.}"
  local yaml_file="$repo_dir/credentials/.baremetal-init.yaml"
  local env_file="$repo_dir/credentials/.env"
  local ssh_config="$HOME/.ssh/config"
  local marker="# --- ScaleX managed ---"
  local end_marker="# --- End ScaleX ---"

  [[ -f "$yaml_file" ]] || return 0

  # Skip if already configured (check both auto-generated and manual markers)
  if [[ -f "$ssh_config" ]] && grep -qE "ScaleX managed|ScaleX-POD-mini" "$ssh_config"; then
    log_info "$(i18n "SSH config already configured — skipping" "SSH config 이미 설정됨 — 건너뜀")"
    return 0
  fi

  # Resolve SSH key path from .env
  local ssh_key="$HOME/.ssh/id_ed25519"
  if [[ -f "$env_file" ]]; then
    local key_val; key_val=$(grep '^SSH_KEY_PATH=' "$env_file" 2>/dev/null | cut -d= -f2- | tr -d '"' | tr -d "'")
    [[ -n "$key_val" ]] && ssh_key="$key_val"
  fi

  mkdir -p "$HOME/.ssh"
  local config_block=""
  config_block+="$marker"$'\n'

  # Parse nodes from YAML (simple line-based parser)
  local name="" ip="" reachable_ip="" reachable_port="" user="" auth_mode="" via=""
  local in_node=false

  while IFS= read -r line; do
    # Detect new node entry
    if [[ "$line" =~ ^[[:space:]]*-[[:space:]]*name:[[:space:]]*\"?([^\"]+)\"? ]]; then
      # Flush previous node
      if [[ -n "$name" ]]; then
        config_block+="Host $name"$'\n'
        if [[ -n "$reachable_ip" ]]; then
          config_block+="    HostName $reachable_ip"$'\n'
        else
          config_block+="    HostName $ip"$'\n'
        fi
        [[ -n "$reachable_port" ]] && config_block+="    Port $reachable_port"$'\n'
        config_block+="    User $user"$'\n'
        [[ "$auth_mode" == "key" ]] && config_block+="    IdentityFile $ssh_key"$'\n'
        [[ -n "$via" ]] && config_block+="    ProxyJump $via"$'\n'
        config_block+="    StrictHostKeyChecking no"$'\n'
        config_block+="    ServerAliveInterval 30"$'\n'
        config_block+="    ServerAliveCountMax 10"$'\n'$'\n'
      fi
      name="${BASH_REMATCH[1]}"
      ip="" reachable_ip="" reachable_port="" user="" auth_mode="" via=""
      in_node=true
    elif $in_node; then
      if [[ "$line" =~ reachable_node_ip:[[:space:]]*\"?([^\"]+)\"? ]]; then
        reachable_ip="${BASH_REMATCH[1]}"
      elif [[ "$line" =~ reachable_node_port:[[:space:]]*\"?([0-9]+)\"? ]]; then
        reachable_port="${BASH_REMATCH[1]}"
      elif [[ "$line" =~ node_ip:[[:space:]]*\"?([^\"]+)\"? ]]; then
        ip="${BASH_REMATCH[1]}"
      elif [[ "$line" =~ adminUser:[[:space:]]*\"?([^\"]+)\"? ]]; then
        user="${BASH_REMATCH[1]}"
      elif [[ "$line" =~ sshAuthMode:[[:space:]]*\"?([^\"]+)\"? ]]; then
        auth_mode="${BASH_REMATCH[1]}"
      elif [[ "$line" =~ reachable_via:.*\"([^\"]+)\" ]]; then
        via="${BASH_REMATCH[1]}"
      fi
    fi
  done < "$yaml_file"

  # Flush last node
  if [[ -n "$name" ]]; then
    config_block+="Host $name"$'\n'
    if [[ -n "$reachable_ip" ]]; then
      config_block+="    HostName $reachable_ip"$'\n'
    else
      config_block+="    HostName $ip"$'\n'
    fi
    [[ -n "$reachable_port" ]] && config_block+="    Port $reachable_port"$'\n'
    config_block+="    User $user"$'\n'
    [[ "$auth_mode" == "key" ]] && config_block+="    IdentityFile $ssh_key"$'\n'
    [[ -n "$via" ]] && config_block+="    ProxyJump $via"$'\n'
    config_block+="    StrictHostKeyChecking no"$'\n'
    config_block+="    ServerAliveInterval 30"$'\n'
    config_block+="    ServerAliveCountMax 10"$'\n'$'\n'
    # Add IP-based entry for libvirt provider (uses IP directly, needs ProxyJump via SSH config)
    if [[ -n "$via" && -n "$ip" ]]; then
      config_block+="Host $ip"$'\n'
      config_block+="    User $user"$'\n'
      [[ "$auth_mode" == "key" ]] && config_block+="    IdentityFile $ssh_key"$'\n'
      config_block+="    ProxyJump $via"$'\n'
      config_block+="    StrictHostKeyChecking no"$'\n'
      config_block+="    ServerAliveInterval 30"$'\n'
      config_block+="    ServerAliveCountMax 10"$'\n'$'\n'
    fi
  fi

  config_block+="$end_marker"$'\n'

  # Append to existing config or create new
  echo "$config_block" >> "$ssh_config"
  chmod 600 "$ssh_config"
  log_info "$(i18n "SSH config generated (~/.ssh/config)" "SSH config 생성 완료 (~/.ssh/config)")"
}

# --- State management ---
state_set() {
  local key="$1" val="$2"
  touch "$STATE_FILE"
  if grep -q "^${key}=" "$STATE_FILE" 2>/dev/null; then
    sed -i "s|^${key}=.*|${key}=${val}|" "$STATE_FILE"
  else
    echo "${key}=${val}" >> "$STATE_FILE"
  fi
}

state_get() {
  local key="$1" default="${2:-}"
  if [[ -f "$STATE_FILE" ]]; then
    local val; val=$(grep "^${key}=" "$STATE_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
    echo "${val:-$default}"
  else
    echo "$default"
  fi
}

state_save_phase() { echo "$1" > "$PHASE_FILE"; }
state_get_phase() { [[ -f "$PHASE_FILE" ]] && cat "$PHASE_FILE" || echo "-1"; }

# phase_mark_done N — write a per-phase completion marker and advance the
# sequential PHASE_FILE if N is greater than the current recorded phase.
# Idempotent: safe to call multiple times for the same phase.
phase_mark_done() {
  local n="$1"
  mkdir -p "$PHASE_DONE_DIR"
  touch "$PHASE_DONE_DIR/${n}.done"
  local cur; cur=$(state_get_phase)
  if (( n > cur )); then
    state_save_phase "$n"
  fi
  log_info "$(i18n "Phase ${n} completion marker written (${PHASE_DONE_DIR}/${n}.done)" \
    "Phase ${n} 완료 마커 저장됨 (${PHASE_DONE_DIR}/${n}.done)")"
}

# phase_is_done N — return 0 if phase N has previously completed successfully.
# Checks per-phase marker file first; falls back to the sequential tracker.
phase_is_done() {
  local n="$1"
  [[ -f "$PHASE_DONE_DIR/${n}.done" ]] && return 0
  local cur; cur=$(state_get_phase)
  (( cur >= n ))
}

phase_label() {
  case "$1" in
    0) echo "Dependencies" ;; 1) echo "Bare-metal & SSH" ;;
    2) echo "SDI Virtualization" ;; 3) echo "Cluster & GitOps" ;;
    4) echo "Build & Provision" ;; *) echo "Unknown" ;;
  esac
}

# phase_skip_if_done PHASE_NUM — Returns 0 (caller should skip) if the given
# phase is already recorded as complete in either the per-phase .done file or
# the sequential PHASE_FILE marker.  Delegates to phase_is_done so both
# tracking sources are always consulted (fixes AC4/AC7 integration gap).
# Usage inside a phase function:  phase_skip_if_done 2 && return 0
phase_skip_if_done() {
  local phase_num="$1"
  if phase_is_done "$phase_num"; then
    local label; label=$(phase_label "$phase_num")
    log_info "$(i18n \
      "Phase ${phase_num} (${label}) already complete — skipping (marker: ${PHASE_DONE_DIR}/${phase_num}.done)" \
      "Phase ${phase_num} (${label}) 이미 완료 — 건너뜀 (마커: ${PHASE_DONE_DIR}/${phase_num}.done)")"
    # Synchronize sequential PHASE_FILE tracker when only the .done file is present
    # (handles the case where .done file exists but PHASE_FILE lags behind).
    local cur; cur=$(state_get_phase)
    if (( phase_num > cur )); then
      state_save_phase "$phase_num"
    fi
    return 0
  fi
  return 1
}

# =============================================================================
# Phase 4 sub-step tracking (auto mode only)
#
# Purpose: Allow resume of a partially-completed Phase 4 provisioning run
# without re-executing non-idempotent infrastructure steps (sdi init,
# cluster init).  Each tracked sub-step writes a sentinel file that is
# checked at the start of the step on subsequent runs.
#
# Sub-step IDs:
#   2 — scalex-pod sdi init     (VM/libvirt provisioning, NOT idempotent)
#   3 — scalex-pod cluster init (k8s bootstrap via Kubespray, slow + risky to redo)
#
# Marker files: $PHASE_DONE_DIR/4s{N}.done
# Cleared by: resume_check reset (phase<=4), resume_check fresh (all *.done)
# =============================================================================

# phase4_step_is_done N — return 0 if phase 4 auto-mode sub-step N is recorded done.
phase4_step_is_done() { [[ -f "$PHASE_DONE_DIR/4s${1}.done" ]]; }

# phase4_step_mark_done N — write completion marker for phase 4 sub-step N.
phase4_step_mark_done() {
  mkdir -p "$PHASE_DONE_DIR"
  touch "$PHASE_DONE_DIR/4s${1}.done"
  log_info "$(i18n "Phase 4 sub-step ${1} marked done (${PHASE_DONE_DIR}/4s${1}.done)" \
    "Phase 4 하위 단계 ${1} 완료 마커 저장됨 (${PHASE_DONE_DIR}/4s${1}.done)")"
}

# phase4_clear_steps — remove all phase 4 sub-step markers (used by reset/fresh).
phase4_clear_steps() {
  rm -f "$PHASE_DONE_DIR"/4s*.done 2>/dev/null || true
  log_info "$(i18n "Phase 4 sub-step markers cleared" "Phase 4 하위 단계 마커 초기화됨")"
}

# =============================================================================
# check_nodes_ssh_health — Parallel SSH health-check gate for cluster nodes.
#
# Purpose:
#   Verify every listed node is SSH-reachable and can execute a command
#   (real login + `echo ok`), NOT a mere TCP port-22 check.
#   Must be called at inter-phase boundaries to satisfy the
#   feedback_network_safety_critical requirement: verify SSH/network BEFORE
#   and AFTER every remote operation.
#
# Usage:
#   check_nodes_ssh_health LABEL [USER@]HOST [[USER@]HOST ...]
#   check_nodes_ssh_health "pre-SDI" root@10.0.0.10 root@10.0.0.11
#
#   LABEL  — human-readable context label (logged & shown in summary)
#   HOST   — bare IP/hostname (default user applies) or user@host
#
# Optional environment overrides:
#   SSH_HEALTH_TIMEOUT         — per-node ceiling seconds   (default 300 = 5 min)
#   SSH_HEALTH_USER            — SSH user for bare hostname  (default: $USER)
#   SSH_HEALTH_CONNECT_TIMEOUT — per-attempt ConnectTimeout  (default 15)
#
# Behaviour:
#   • All nodes probed in parallel (one background job per node)
#   • SSH check = real login + `echo ok` — never just TCP/port-22
#   • Per-node retry with exponential backoff: 5 s → 10 s → 20 s → 40 s
#     (4 gaps → 5 total attempts per node within the timeout ceiling)
#   • Retry progress logged every attempt: node, attempt/max, backoff seconds
#   • Fail-fast: the first node that exhausts all retries kills all sibling
#     probes and aborts with a descriptive error_msg()
#   • Compact per-node summary table (PASS/FAIL) printed after completion
#   • Returns 0 if ALL nodes pass; returns 1 on any failure (caller aborts)
# =============================================================================

# _ssh_node_probe: per-node worker, always run as a background job.
# Writes "PASS" or "FAIL:<reason>" to RESULT_FILE then exits.
# Args: NODE USER RESULT_FILE CONNECT_TIMEOUT LABEL
_ssh_node_probe() {
  local node="$1" user="$2" result_file="$3"
  local connect_timeout="${4:-15}" label="${5:-ssh-health}"
  # Backoff sequence between consecutive attempts (seconds)
  local -a backoffs=(5 10 20 40)
  local max_attempts=$(( ${#backoffs[@]} + 1 ))   # 5 total attempts
  local attempt=0 last_err=""

  while (( attempt < max_attempts )); do
    local err_tmp; err_tmp=$(mktemp /tmp/scalex-ssh-probe.XXXXXX 2>/dev/null \
                              || echo "/tmp/scalex-ssh-probe.$$.${RANDOM}")
    : > "$err_tmp"

    local ssh_out
    ssh_out=$(ssh \
      -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null \
      -o BatchMode=yes \
      -o ConnectTimeout="$connect_timeout" \
      -o ServerAliveInterval=5 \
      -o ServerAliveCountMax=1 \
      "${user}@${node}" "echo ok" 2>"$err_tmp") || true

    if [[ "$ssh_out" == *ok* ]]; then
      rm -f "$err_tmp"
      echo "PASS" > "$result_file"
      return 0
    fi

    last_err=$(head -3 "$err_tmp" 2>/dev/null | tr '\n' ' ')
    rm -f "$err_tmp"

    attempt=$(( attempt + 1 ))

    if (( attempt < max_attempts )); then
      local wait_sec=${backoffs[$(( attempt - 1 ))]}
      # Structured retry log — visible in install log (written via stderr → log_raw)
      printf '%s\n' \
        "[$(date '+%Y-%m-%d %H:%M:%S')] WARN: ssh_health: label=${label}" \
        "  node=${node} attempt=${attempt}/${max_attempts}" \
        "  backoff=${wait_sec}s stderr=\"${last_err:-none}\"" >&2
      sleep "$wait_sec"
    fi
  done

  # All attempts exhausted — record failure
  local reason="${last_err:-connection refused or timed out after ${max_attempts} attempts}"
  printf '%s\n' \
    "[$(date '+%Y-%m-%d %H:%M:%S')] ERROR: ssh_health: label=${label}" \
    "  node=${node} FAILED after ${max_attempts} attempts" \
    "  last_error=\"${last_err:-none}\"" >&2
  echo "FAIL:${reason}" > "$result_file"
  return 1
}

check_nodes_ssh_health() {
  local label="${1:?check_nodes_ssh_health requires a LABEL as first argument}"
  shift
  local -a nodes=("$@")

  if [[ ${#nodes[@]} -eq 0 ]]; then
    log_warn "$(i18n "check_nodes_ssh_health(${label}): no nodes — skipping" \
      "check_nodes_ssh_health(${label}): 노드 없음 — 건너뜀")"
    return 0
  fi

  local timeout_ceil="${SSH_HEALTH_TIMEOUT:-300}"
  local default_user="${SSH_HEALTH_USER:-${USER:-root}}"
  local connect_timeout="${SSH_HEALTH_CONNECT_TIMEOUT:-15}"

  # Temp directory for per-node result files
  local result_dir
  result_dir=$(mktemp -d /tmp/scalex-ssh-health.XXXXXX 2>/dev/null) || {
    result_dir="/tmp/scalex-ssh-health.$$"
    mkdir -p "$result_dir"
  }
  chmod 700 "$result_dir"

  log_info "$(i18n "SSH health check starting: label=${label} nodes=${#nodes[@]} timeout=${timeout_ceil}s connect_timeout=${connect_timeout}s" \
    "SSH 상태 확인 시작: label=${label} nodes=${#nodes[@]} timeout=${timeout_ceil}초 connect_timeout=${connect_timeout}초")"

  # --- Launch one background probe per node ---
  # Keep ordered list (bash assoc arrays don't preserve order)
  local -a node_list=()       # ordered hostnames (post-parse)
  local -A node_users=()      # node → ssh user
  local -A node_result_files=() # node → result file path
  local -A node_pids=()       # node → background PID

  local entry
  for entry in "${nodes[@]}"; do
    local u h
    if [[ "$entry" == *@* ]]; then
      u="${entry%%@*}"
      h="${entry#*@}"
    else
      u="$default_user"
      h="$entry"
    fi

    # Sanitise hostname → safe filename key (dots/colons/slashes → underscores)
    local h_safe; h_safe=$(printf '%s' "$h" | tr '.:/@' '____')
    local rf="${result_dir}/${h_safe}.result"
    : > "$rf"

    node_list+=("$h")
    node_users["$h"]="$u"
    node_result_files["$h"]="$rf"

    log_info "$(i18n "  SSH probe launch: ${u}@${h} (label=${label})" \
      "  SSH 프로브 시작: ${u}@${h} (label=${label})")"

    _ssh_node_probe "$h" "$u" "$rf" "$connect_timeout" "$label" &
    node_pids["$h"]=$!
  done

  # --- Fail-fast polling loop ---
  # Poll every second; abort as soon as any probe reports FAIL.
  # Honour the overall deadline (timeout_ceil).
  local deadline=$(( $(date +%s) + timeout_ceil ))
  local failed_node=""

  while true; do
    local all_done=true
    local n

    for n in "${node_list[@]}"; do
      local pid=${node_pids[$n]}
      local rf=${node_result_files[$n]}

      if kill -0 "$pid" 2>/dev/null; then
        # Probe still running
        all_done=false
      else
        # Probe exited — check result
        local res; res=$(cat "$rf" 2>/dev/null || echo "FAIL:result file missing")
        if [[ "$res" != "PASS" ]]; then
          failed_node="$n"
          break 2   # Break out of the for loop AND the while loop
        fi
      fi
    done

    # All probes finished cleanly (no break triggered above)
    $all_done && break

    # Check global deadline
    if (( $(date +%s) >= deadline )); then
      log_warn "$(i18n "SSH health check: global timeout ${timeout_ceil}s reached — aborting remaining probes" \
        "SSH 상태 확인: 전체 시간 초과 ${timeout_ceil}초 — 나머지 프로브 중단")"
      for n in "${node_list[@]}"; do
        if kill -0 "${node_pids[$n]}" 2>/dev/null; then
          kill "${node_pids[$n]}" 2>/dev/null || true
          echo "FAIL:global timeout after ${timeout_ceil}s" > "${node_result_files[$n]}"
          [[ -z "$failed_node" ]] && failed_node="$n"
        fi
      done
      break
    fi

    sleep 1
  done

  # Kill any sibling probes still running (fail-fast cleanup)
  for n in "${node_list[@]}"; do
    kill "${node_pids[$n]}" 2>/dev/null || true
  done
  # Reap all background jobs
  for n in "${node_list[@]}"; do
    wait "${node_pids[$n]}" 2>/dev/null || true
  done

  # --- Print per-node summary ---
  echo ""
  echo -e "${BOLD}SSH Health Check — ${label}${NC}"
  echo -e "${BOLD}──────────────────────────────────────────────────${NC}"
  local pass_count=0 fail_count=0
  for n in "${node_list[@]}"; do
    local rf=${node_result_files[$n]}
    local res; res=$(cat "$rf" 2>/dev/null || echo "FAIL:no result")
    local u=${node_users[$n]}
    if [[ "$res" == "PASS" ]]; then
      echo -e "  ${GREEN}PASS${NC}  ${u}@${n}"
      pass_count=$(( pass_count + 1 ))
    else
      local reason; reason=$(printf '%s' "$res" | sed 's/^FAIL://')
      echo -e "  ${RED}FAIL${NC}  ${u}@${n}  — ${reason}"
      fail_count=$(( fail_count + 1 ))
    fi
  done
  echo -e "${BOLD}──────────────────────────────────────────────────${NC}"
  echo -e "  Result: ${pass_count} passed, ${fail_count} failed"
  echo ""

  # Capture failure reason BEFORE cleaning up result_dir
  local fail_reason=""
  if [[ -n "$failed_node" ]]; then
    local fail_res; fail_res=$(cat "${node_result_files[$failed_node]}" 2>/dev/null \
                                || echo "FAIL:unknown")
    fail_reason=$(printf '%s' "$fail_res" | sed 's/^FAIL://')
  fi

  rm -rf "$result_dir"

  if [[ -n "$failed_node" ]]; then
    local fail_user="${node_users[$failed_node]:-${default_user}}"
    error_msg \
      "$(i18n "SSH health check FAILED [${label}]: node '${failed_node}' unreachable" \
         "SSH 상태 확인 실패 [${label}]: 노드 '${failed_node}' 접근 불가")" \
      "$(i18n "SSH login + echo ok failed after all retries — reason: ${fail_reason}" \
         "SSH 로그인 + echo ok 모든 재시도 후 실패 — 이유: ${fail_reason}")" \
      "$(i18n "Fix SSH connectivity then re-run. Test manually: ssh ${fail_user}@${failed_node} echo ok" \
         "SSH 연결을 수정한 후 재실행. 수동 테스트: ssh ${fail_user}@${failed_node} echo ok")"
    log_error "$(i18n "install.sh aborting due to SSH health check failure (label=${label}, node=${failed_node})" \
      "SSH 상태 확인 실패로 install.sh 중단 (label=${label}, node=${failed_node})")"
    return 1
  fi

  log_info "$(i18n "SSH health check PASSED [${label}]: ${pass_count}/${#nodes[@]} nodes OK" \
    "SSH 상태 확인 통과 [${label}]: ${pass_count}/${#nodes[@]} 노드 OK")"
  return 0
}

# =============================================================================
# check_ssh_health — Public SSH health-check API.
#
# Primary entry point for verifying SSH connectivity to a set of nodes before
# or after any remote operation (feedback_network_safety_critical compliance).
#
# All nodes are probed IN PARALLEL (one background job per node).
# SSH check = real login + `echo ok` — NOT a mere TCP/port-22 check.
#
# Per-node retry with exponential backoff:
#   attempt 1 → 2 (wait 5 s) → 3 (wait 10 s) → 4 (wait 20 s) → 5 (wait 40 s)
# First SSH failure after all retries triggers abort; all sibling probes are killed.
#
# Configurable via environment variables:
#   SSH_HEALTH_TIMEOUT         — overall per-node ceiling in seconds (default 300 = 5 min)
#   SSH_HEALTH_CONNECT_TIMEOUT — per-attempt ConnectTimeout           (default 15 s)
#   SSH_HEALTH_USER            — SSH user for bare hostnames           (default: $USER)
#
# Usage:
#   check_ssh_health LABEL [USER@]HOST [[USER@]HOST ...]
#   check_ssh_health "pre-SDI"   root@10.0.0.10 root@10.0.0.11
#   check_ssh_health "post-install"  admin@node1 admin@node2 admin@node3
#
# Returns:
#   0  — ALL nodes passed SSH login + echo-ok within the timeout
#   1  — ANY node failed after all retries (caller should abort or log warning)
# =============================================================================
check_ssh_health() {
  local label="${1:?check_ssh_health requires a LABEL as first argument}"
  shift
  # Delegate to the core parallel-probe orchestrator.
  # check_nodes_ssh_health handles: parallel launch, fail-fast polling,
  # per-node summary table, and structured error_msg on failure.
  check_nodes_ssh_health "$label" "$@"
}

# disable_nic_offload — Disable TSO/GSO/GRO on all bare-metal nodes' eno1 NIC.
#
# Prevents Intel e1000e Hardware Unit Hang under heavy traffic.
# Reads node names from .baremetal-init.yaml; persists via udev rule.
# Non-fatal: logs warn and continues if a node is unreachable.
#
# Usage: disable_nic_offload [REPO_DIR]
disable_nic_offload() {
  local repo_dir="${1:-${REPO_DIR:-$(pwd)}}"
  local bm_yaml=""
  if [[ -f "$repo_dir/credentials/.baremetal-init.yaml" ]]; then
    bm_yaml="$repo_dir/credentials/.baremetal-init.yaml"
  elif [[ -f "$GEN_DIR/credentials/.baremetal-init.yaml" ]]; then
    bm_yaml="$GEN_DIR/credentials/.baremetal-init.yaml"
  fi

  if [[ -z "$bm_yaml" ]]; then
    log_warn "$(i18n "disable_nic_offload: no .baremetal-init.yaml found — skipping" \
      "disable_nic_offload: .baremetal-init.yaml 없음 — 건너뜀")"
    return 0
  fi

  log_info "$(i18n "Disabling NIC offload (TSO/GSO/GRO) on all nodes (e1000e hang workaround)..." \
    "모든 노드에서 NIC offload(TSO/GSO/GRO) 비활성화 중 (e1000e 하드웨어 행 방지)...")"

  local _node_name=""
  while IFS= read -r _line; do
    if [[ "$_line" =~ ^[[:space:]]*-[[:space:]]*name:[[:space:]]*\"?([^\"[:space:]]+)\"? ]]; then
      _node_name="${BASH_REMATCH[1]}"
      if [[ -n "$_node_name" ]]; then
        ssh -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=no "$_node_name" \
          "sudo ethtool -K eno1 tso off gso off gro off 2>/dev/null; \
           echo 'ACTION==\"add\", SUBSYSTEM==\"net\", KERNEL==\"eno1\", RUN+=\"/sbin/ethtool -K eno1 tso off gso off gro off\"' \
           | sudo tee /etc/udev/rules.d/99-disable-offload.rules >/dev/null" \
          && log_info "  $_node_name: offload disabled" \
          || log_warn "  $_node_name: failed to disable offload (continuing)"
      fi
    fi
  done < "$bm_yaml"
}

# phase_ssh_check — Inter-phase SSH health-check wrapper.
#
# Reads bare-metal nodes from .baremetal-init.yaml, ensures ~/.ssh/config has
# ProxyJump/IdentityFile entries (via generate_ssh_config — idempotent), then
# calls check_nodes_ssh_health with the parsed USER@HOST list.
#
# In --auto mode  : returns 1 on any failure; caller MUST propagate with || return 1
# Interactive mode: returns 1 on any failure; caller should use || true (non-fatal)
# Graceful skip   : returns 0 when no .baremetal-init.yaml or no nodes found
#
# Usage: phase_ssh_check PHASE_LABEL [REPO_DIR]
#   PHASE_LABEL — e.g. "pre-flight", "pre-sdi-init", "post-install"
#   REPO_DIR    — defaults to $REPO_DIR global then $(pwd)
phase_ssh_check() {
  local phase_label="$1"
  local repo_dir="${2:-${REPO_DIR:-$(pwd)}}"

  # Locate .baremetal-init.yaml: check repo_dir first, then installer generated dir
  local bm_yaml="" bm_source_dir="$repo_dir"
  if [[ -f "$repo_dir/credentials/.baremetal-init.yaml" ]]; then
    bm_yaml="$repo_dir/credentials/.baremetal-init.yaml"
  elif [[ -f "$GEN_DIR/credentials/.baremetal-init.yaml" ]]; then
    bm_yaml="$GEN_DIR/credentials/.baremetal-init.yaml"
    bm_source_dir="$GEN_DIR"
  fi

  if [[ -z "$bm_yaml" ]]; then
    log_info "$(i18n \
      "SSH health check [${phase_label}]: no .baremetal-init.yaml found — skipping" \
      "SSH 상태 확인 [${phase_label}]: .baremetal-init.yaml 없음 — 건너뜀")"
    return 0
  fi

  # Ensure ~/.ssh/config has ProxyJump/IdentityFile entries (idempotent)
  generate_ssh_config "$bm_source_dir"

  # Parse user@hostname pairs from .baremetal-init.yaml (simple line-based parser)
  local node_args=()
  local _psc_name="" _psc_user=""
  while IFS= read -r _psc_line; do
    if [[ "$_psc_line" =~ ^[[:space:]]*-[[:space:]]*name:[[:space:]]*\"?([^\"[:space:]]+)\"? ]]; then
      [[ -n "$_psc_name" ]] && node_args+=("${_psc_user:-root}@${_psc_name}")
      _psc_name="${BASH_REMATCH[1]}"
      _psc_user=""
    elif [[ "$_psc_line" =~ adminUser:[[:space:]]*\"?([^\"[:space:]]+)\"? ]]; then
      _psc_user="${BASH_REMATCH[1]}"
    fi
  done < "$bm_yaml"
  [[ -n "$_psc_name" ]] && node_args+=("${_psc_user:-root}@${_psc_name}")

  if [[ ${#node_args[@]} -eq 0 ]]; then
    log_info "$(i18n \
      "SSH health check [${phase_label}]: no nodes in config — skipping" \
      "SSH 상태 확인 [${phase_label}]: 설정에 노드 없음 — 건너뜀")"
    return 0
  fi

  # Run parallel SSH health check (retries, backoff, summary all handled inside)
  if ! check_nodes_ssh_health "$phase_label" "${node_args[@]}"; then
    if [[ "$AUTO_MODE" == "true" ]]; then
      log_error "$(i18n \
        "install.sh: SSH health check [${phase_label}] FAILED in --auto mode — aborting install" \
        "install.sh: SSH 상태 확인 [${phase_label}] --auto 모드 실패 — 설치 중단")"
      return 1
    fi
    # Interactive mode: check_nodes_ssh_health already printed per-node summary + error_msg
    log_warn "$(i18n \
      "SSH health check [${phase_label}]: failure — continuing in interactive mode" \
      "SSH 상태 확인 [${phase_label}]: 실패 — 대화형 모드에서 계속 진행")"
    return 1
  fi
  return 0
}

# ============================================================================
# Section 2: Dependency Management
# ============================================================================

detect_os() {
  if [[ -f /etc/os-release ]]; then
    # Read ID from os-release without sourcing (avoids readonly VERSION conflict)
    local _os_id
    _os_id=$(grep -oP '^ID=\K.*' /etc/os-release | tr -d '"')
    case "$_os_id" in
      ubuntu|debian|linuxmint|pop) echo "debian" ;;
      centos|rhel|rocky|alma|fedora) echo "rhel" ;;
      arch|manjaro|endeavouros) echo "arch" ;;
      *) echo "unknown" ;;
    esac
  elif [[ "$(uname)" == "Darwin" ]]; then echo "macos"
  else echo "unknown"; fi
}

check_dep() {
  local name="$1" cmd="${2:-$1}"
  command -v "$cmd" &>/dev/null
}

install_dep() {
  local name="$1" os="$2"
  log_info "$(i18n "Installing $name..." "$name 설치 중...")"
  case "$name" in
    git)
      case "$os" in
        debian) sudo apt-get update -qq && sudo apt-get install -yqq git ;;
        rhel) sudo dnf install -y git ;;
        arch) sudo pacman -S --noconfirm git ;;
        macos) xcode-select --install 2>/dev/null || true ;;
      esac ;;
    rust)
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
      # shellcheck disable=SC1091
      source "$HOME/.cargo/env" 2>/dev/null || true ;;
    ansible)
      if command -v pip3 &>/dev/null; then pip3 install --user ansible
      elif command -v pipx &>/dev/null; then pipx install ansible
      else
        case "$os" in
          debian) sudo apt-get install -yqq ansible ;;
          rhel) sudo dnf install -y ansible ;;
          arch) sudo pacman -S --noconfirm ansible ;;
          macos) brew install ansible ;;
        esac
      fi ;;
    python3)
      case "$os" in
        debian) sudo apt-get install -yqq python3 python3-pip ;;
        rhel) sudo dnf install -y python3 python3-pip ;;
        arch) sudo pacman -S --noconfirm python python-pip ;;
        macos) brew install python3 ;;
      esac ;;
    opentofu)
      local arch; arch=$(uname -m)
      [[ "$arch" == "x86_64" ]] && arch="amd64"
      [[ "$arch" == "aarch64" ]] && arch="arm64"
      local url="https://github.com/opentofu/opentofu/releases/download/v${OPENTOFU_VERSION}/tofu_${OPENTOFU_VERSION}_linux_${arch}.tar.gz"
      local tmp; tmp=$(mktemp -d)
      curl -fsSL "$url" | tar xz -C "$tmp"
      install -m 755 "$tmp/tofu" "$HOME/.local/bin/tofu"
      rm -rf "$tmp" ;;
    kubectl)
      local arch; arch=$(uname -m)
      [[ "$arch" == "x86_64" ]] && arch="amd64"
      [[ "$arch" == "aarch64" ]] && arch="arm64"
      local os_name; os_name=$(uname -s | tr '[:upper:]' '[:lower:]')
      curl -fsSLo "$HOME/.local/bin/kubectl" \
        "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/${os_name}/${arch}/kubectl"
      chmod +x "$HOME/.local/bin/kubectl" ;;
    helm)
      curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | \
        HELM_INSTALL_DIR="$HOME/.local/bin" USE_SUDO=false bash ;;
    argocd)
      local arch; arch=$(uname -m)
      [[ "$arch" == "x86_64" ]] && arch="amd64"
      [[ "$arch" == "aarch64" ]] && arch="arm64"
      local os_name; os_name=$(uname -s | tr '[:upper:]' '[:lower:]')
      curl -fsSLo "$HOME/.local/bin/argocd" \
        "https://github.com/argoproj/argo-cd/releases/download/${ARGOCD_VERSION}/argocd-${os_name}-${arch}"
      chmod +x "$HOME/.local/bin/argocd" ;;
    sshpass)
      case "$os" in
        debian) sudo apt-get install -yqq sshpass ;;
        rhel) sudo dnf install -y sshpass ;;
        arch) sudo pacman -S --noconfirm sshpass ;;
        macos) brew install hudochenkov/sshpass/sshpass 2>/dev/null || log_warn "$(i18n "sshpass requires manual installation on macOS" "sshpass는 macOS에서 별도 설치 필요")" ;;
      esac ;;
    whiptail)
      case "$os" in
        debian) sudo apt-get install -yqq whiptail ;;
        rhel) sudo dnf install -y newt ;;
        arch) sudo pacman -S --noconfirm libnewt ;;
      esac ;;
  esac
}

phase_deps() {
  phase_skip_if_done 0 && return 0
  log_phase "$(i18n "Phase 0: Checking dependencies" "Phase 0: 의존성 확인")"
  local os; os=$(detect_os)
  log_info "$(i18n "OS: $os" "운영체제: $os")"
  mkdir -p "$HOME/.local/bin"
  export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

  local deps=("git:git" "rust:cargo" "ansible:ansible" "python3:python3"
               "opentofu:tofu" "kubectl:kubectl" "helm:helm" "argocd:argocd")
  local optional_deps=("sshpass:sshpass")
  local missing=()
  for entry in "${deps[@]}"; do
    local name="${entry%%:*}" cmd="${entry##*:}"
    if ! check_dep "$name" "$cmd"; then missing+=("$name"); fi
  done

  # Check optional deps (warn only, don't block)
  local opt_missing=()
  for entry in "${optional_deps[@]}"; do
    local name="${entry%%:*}" cmd="${entry##*:}"
    if ! check_dep "$name" "$cmd"; then opt_missing+=("$name"); fi
  done
  if [[ ${#opt_missing[@]} -gt 0 ]]; then
    log_warn "$(i18n "Optional tools missing (needed for password SSH auth): ${opt_missing[*]}" "선택적 도구 누락 (password SSH 인증 시 필요): ${opt_missing[*]}")"
  fi

  if [[ ${#missing[@]} -eq 0 ]]; then
    log_info "$(i18n "All required dependencies are installed." "모든 필수 의존성이 설치되어 있습니다.")"
    phase_mark_done 0
    return 0
  fi

  log_warn "$(i18n "Missing tools: ${missing[*]}" "누락된 도구: ${missing[*]}")"
  if tui_yesno "$(i18n "Install dependencies" "의존성 설치")" "$(i18n "Install the following tools?\n\n${missing[*]}" "다음 도구를 설치하시겠습니까?\n\n${missing[*]}")"; then
    for name in "${missing[@]}"; do
      install_dep "$name" "$os" || {
        error_msg "$(i18n "$name installation failed" "$name 설치 실패")" "$(i18n "Package manager error or network issue" "패키지 매니저 오류 또는 네트워크 문제")" "$(i18n "Install manually and re-run" "수동 설치 후 다시 실행하세요")"
        return 1
      }
    done
    # Install optional deps (best-effort, don't fail)
    for name in "${opt_missing[@]}"; do
      install_dep "$name" "$os" || log_warn "$(i18n "$name installation failed — can be ignored if not using password SSH" "$name 설치 실패 — password SSH 미사용 시 무시 가능")"
    done
    # Re-source cargo env if rust was installed
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env" 2>/dev/null || true
    log_info "$(i18n "Dependency installation complete" "의존성 설치 완료")"
  else
    log_warn "$(i18n "Skipping dependencies. Some features may not work." "의존성을 건너뜁니다. 일부 기능이 작동하지 않을 수 있습니다.")"
  fi
  phase_mark_done 0
}

# ============================================================================
# Section 3: Phase 1 — Bare-metal & SSH
# ============================================================================

phase_baremetal() {
  phase_skip_if_done 1 && return 0
  log_phase "$(i18n "Phase 1: Bare-metal nodes & SSH setup" "Phase 1: 베어메탈 노드 & SSH 설정")"

  # Network defaults
  local bridge cidr gateway
  bridge=$(tui_input "$(i18n "Network defaults" "네트워크 기본값")" "$(i18n "Management bridge interface:" "관리 브릿지 인터페이스:")" "$(state_get NET_BRIDGE br0)")
  cidr=$(tui_input "$(i18n "Network defaults" "네트워크 기본값")" "$(i18n "Management network CIDR:" "관리 네트워크 CIDR:")" "$(state_get NET_CIDR 10.0.0.0/24)")
  while ! validate_cidr "$cidr"; do
    log_error "$(i18n "Invalid CIDR: $cidr" "유효하지 않은 CIDR: $cidr")"
    cidr=$(tui_input "$(i18n "Network defaults" "네트워크 기본값")" "$(i18n "Management network CIDR (e.g. 10.0.0.0/24):" "관리 네트워크 CIDR (예: 10.0.0.0/24):")" "10.0.0.0/24")
  done
  gateway=$(tui_input "$(i18n "Network defaults" "네트워크 기본값")" "$(i18n "Gateway IP:" "게이트웨이 IP:")" "$(state_get NET_GW 10.0.0.1)")
  while ! validate_ip "$gateway"; do
    log_error "$(i18n "Invalid IP: $gateway" "유효하지 않은 IP: $gateway")"
    gateway=$(tui_input "$(i18n "Network defaults" "네트워크 기본값")" "$(i18n "Gateway IP:" "게이트웨이 IP:")" "10.0.0.1")
  done
  state_set NET_BRIDGE "$bridge"
  state_set NET_CIDR "$cidr"
  state_set NET_GW "$gateway"

  # Node collection
  > "$INSTALLER_DIR/nodes.txt"
  local env_lines=""
  local yaml_nodes=""
  local adding=true
  NODE_COUNT=0

  while $adding; do
    NODE_COUNT=$((NODE_COUNT + 1))
    log_info "$(i18n "Configuring node #${NODE_COUNT}" "노드 #${NODE_COUNT} 설정")"

    local name; name=$(tui_input "$(i18n "Node #${NODE_COUNT}" "노드 #${NODE_COUNT}")" "$(i18n "Node name (e.g. node-0):" "노드 이름 (예: node-0):")" "")
    while ! validate_not_empty "$name"; do
      name=$(tui_input "$(i18n "Node #${NODE_COUNT}" "노드 #${NODE_COUNT}")" "$(i18n "Node name (cannot be empty):" "노드 이름 (비어있을 수 없음):")" "")
    done

    local access; access=$(tui_menu "$(i18n "Node access method" "노드 접근 방식")" "$(i18n "Select SSH access method:" "SSH 접근 방식 선택:")" \
      "direct"  "$(i18n "Direct access (same LAN)" "직접 접근 (같은 LAN)")" \
      "external" "$(i18n "External IP (e.g. Tailscale)" "외부 IP (예: Tailscale)")" \
      "proxy"   "$(i18n "ProxyJump (via another node)" "ProxyJump (다른 노드 경유)")")

    local node_ip; node_ip=$(tui_input "$(i18n "Node IP" "노드 IP")" "$(i18n "Node LAN IP:" "노드 LAN IP:")" "")
    while ! validate_ip "$node_ip"; do
      log_error "$(i18n "Invalid IP" "유효하지 않은 IP")"
      node_ip=$(tui_input "$(i18n "Node IP" "노드 IP")" "$(i18n "Node LAN IP (e.g. 10.0.0.10):" "노드 LAN IP (예: 10.0.0.10):")" "")
    done

    local admin_user; admin_user=$(tui_input "$(i18n "SSH user" "SSH 사용자")" "$(i18n "SSH user:" "SSH 사용자:")" "${USER:-$(id -un 2>/dev/null || echo root)}")

    local auth_mode; auth_mode=$(tui_menu "$(i18n "Auth method" "인증 방식")" "$(i18n "SSH auth method:" "SSH 인증 방식:")" \
      "password" "$(i18n "Password" "비밀번호")" \
      "key"      "$(i18n "SSH key" "SSH 키")")

    local var_upper; var_upper=$(echo "${name}" | tr '[:lower:]-' '[:upper:]_')

    # Build YAML node entry
    local node_yaml=""
    node_yaml+="  - name: \"${name}\"\n"

    case "$access" in
      direct)
        node_yaml+="    direct_reachable: true\n"
        node_yaml+="    node_ip: \"${node_ip}\"\n"
        ;;
      external)
        local ext_ip; ext_ip=$(tui_input "$(i18n "External IP" "외부 IP")" "$(i18n "External access IP (e.g. Tailscale IP):" "외부 접근 IP (예: Tailscale IP):")" "")
        while ! validate_ip "$ext_ip"; do
          ext_ip=$(tui_input "$(i18n "External IP" "외부 IP")" "$(i18n "Enter a valid IP:" "유효한 IP를 입력하세요:")" "")
        done
        node_yaml+="    direct_reachable: false\n"
        node_yaml+="    reachable_node_ip: \"${ext_ip}\"\n"
        node_yaml+="    node_ip: \"${node_ip}\"\n"
        ;;
      proxy)
        local proxy_node; proxy_node=$(tui_input "ProxyJump" "$(i18n "Proxy node name:" "경유할 노드 이름:")" "")
        node_yaml+="    direct_reachable: false\n"
        node_yaml+="    reachable_via: [\"${proxy_node}\"]\n"
        node_yaml+="    node_ip: \"${node_ip}\"\n"
        ;;
    esac

    node_yaml+="    adminUser: \"${admin_user}\"\n"
    node_yaml+="    sshAuthMode: \"${auth_mode}\"\n"

    if [[ "$auth_mode" == "password" ]]; then
      local pw; pw=$(tui_password "$(i18n "SSH password" "SSH 비밀번호")" "$(i18n "SSH password for ${name}:" "${name} 의 SSH 비밀번호:")")
      env_lines+="${var_upper}_PASSWORD=\"${pw}\"\n"
      node_yaml+="    sshPassword: \"${var_upper}_PASSWORD\"\n"
    else
      local kp; kp=$(tui_input "$(i18n "SSH key path" "SSH 키 경로")" "$(i18n "SSH key path:" "SSH 키 경로:")" "~/.ssh/id_ed25519")
      env_lines+="SSH_KEY_PATH=\"${kp}\"\n"
      node_yaml+="    sshKeyPath: \"SSH_KEY_PATH\"\n"
    fi

    yaml_nodes+="${node_yaml}\n"
    echo "${name}|${access}|${node_ip}|${admin_user}|${auth_mode}" >> "$INSTALLER_DIR/nodes.txt"

    if ! tui_yesno "$(i18n "Add node" "노드 추가")" "$(i18n "Add another node?" "다른 노드를 추가하시겠습니까?")"; then
      adding=false
    fi
  done

  # Generate .baremetal-init.yaml
  cat > "$GEN_DIR/credentials/.baremetal-init.yaml" << BEOF
networkDefaults:
  managementBridge: "${bridge}"
  managementCidr: "${cidr}"
  gateway: "${gateway}"

targetNodes:
$(echo -e "$yaml_nodes" | sed '/^$/d')
BEOF
  chmod 600 "$GEN_DIR/credentials/.baremetal-init.yaml"

  # Generate .env
  {
    echo "# credentials/.env — generated by ScaleX installer"
    echo -e "$env_lines" | sed '/^$/d'
  } > "$GEN_DIR/credentials/.env"
  chmod 600 "$GEN_DIR/credentials/.env"

  # Summary
  log_info "$(i18n "${NODE_COUNT} node(s) configured" "노드 ${NODE_COUNT}개 구성 완료")"

  # SSH test option
  if tui_yesno "$(i18n "SSH test" "SSH 테스트")" "$(i18n "Test SSH connection to configured nodes?" "구성된 노드에 SSH 연결을 테스트하시겠습니까?")"; then
    while IFS='|' read -r n _ ip user auth; do
      echo -n "  ${n} (${ip})... "
      if ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no -o BatchMode=yes "${user}@${ip}" "echo ok" 2>/dev/null; then
        echo -e "${GREEN}OK${NC}"
      else
        echo -e "${YELLOW}FAIL ($(i18n "manual verification required" "수동 확인 필요"))${NC}"
      fi
    done < "$INSTALLER_DIR/nodes.txt"
  fi

  state_set NODE_COUNT "$NODE_COUNT"
  phase_mark_done 1
  log_info "$(i18n "Phase 1 complete" "Phase 1 완료")"
}

# ============================================================================
# Section 4: Phase 2 — SDI Virtualization
# ============================================================================

collect_vm_specs() {
  local pool_name="$1" purpose="$2" pool_idx="$3"
  local vm_yaml="" vm_count=0 adding_vm=true

  # Placement
  local place_mode; place_mode=$(tui_menu "$(i18n "Placement ($pool_name)" "배치 방식 ($pool_name)")" "$(i18n "VM placement method:" "VM 배치 방식:")" \
    "hosts" "$(i18n "Specify hosts" "특정 호스트 지정")" \
    "spread" "$(i18n "Spread across all hosts" "호스트 전체에 분산")")

  local place_yaml=""
  if [[ "$place_mode" == "spread" ]]; then
    place_yaml="      placement:\n        spread: true"
  else
    local hosts_str; hosts_str=$(tui_input "$(i18n "Specify hosts" "호스트 지정")" "$(i18n "Host names (comma-separated):" "호스트 이름 (쉼표 구분):")" "")
    local hosts_list; hosts_list=$(echo "$hosts_str" | sed 's/,/, /g; s/^/[/; s/$/]/')
    place_yaml="      placement:\n        hosts: ${hosts_list}"
  fi

  while $adding_vm; do
    vm_count=$((vm_count + 1))
    log_info "  VM #${vm_count} (${pool_name})"

    local vm_name; vm_name=$(tui_input "$(i18n "VM name" "VM 이름")" "$(i18n "VM name:" "VM 이름:")" "${pool_name}-cp-$((vm_count-1))")
    local vm_ip; vm_ip=$(tui_input "VM IP" "VM IP:" "")
    while ! validate_ip "$vm_ip"; do
      vm_ip=$(tui_input "VM IP" "$(i18n "Enter a valid IP:" "유효한 IP를 입력하세요:")" "")
    done
    local vm_cpu; vm_cpu=$(tui_input "CPU" "$(i18n "CPU cores:" "CPU 코어 수:")" "4")
    local vm_mem; vm_mem=$(tui_input "$(i18n "Memory" "메모리")" "$(i18n "Memory (GB):" "메모리 (GB):")" "8")
    local vm_disk; vm_disk=$(tui_input "$(i18n "Disk" "디스크")" "$(i18n "Disk (GB):" "디스크 (GB):")" "60")

    local vm_host=""
    if [[ "$place_mode" == "spread" ]]; then
      vm_host=$(tui_input "$(i18n "Host" "호스트")" "$(i18n "Host for this VM (leave empty for auto):" "이 VM의 호스트 (비워두면 자동):")" "")
    fi

    local roles_str; roles_str=$(tui_checklist "$(i18n "Select roles" "역할 선택")" "$(i18n "Select VM roles:" "VM 역할 선택:")" \
      "control-plane" "$(i18n "Control plane" "컨트롤 플레인")" "OFF" \
      "etcd"          "etcd"           "OFF" \
      "worker"        "$(i18n "Worker node" "워커 노드")"      "ON")
    # Normalize checklist output
    roles_str=$(echo "$roles_str" | tr -d '"' | tr ' ' ', ')
    [[ -z "$roles_str" ]] && roles_str="worker"
    local roles_yaml; roles_yaml=$(echo "$roles_str" | sed 's/,/, /g; s/^/[/; s/$/]/')

    local gpu_line=""
    if tui_yesno "GPU" "$(i18n "Enable GPU passthrough?" "GPU 패스스루를 활성화하시겠습니까?")"; then
      gpu_line="\n          devices:\n            gpu_passthrough: true"
    fi

    vm_yaml+="        - node_name: \"${vm_name}\"\n"
    vm_yaml+="          ip: \"${vm_ip}\"\n"
    vm_yaml+="          cpu: ${vm_cpu}\n"
    vm_yaml+="          mem_gb: ${vm_mem}\n"
    vm_yaml+="          disk_gb: ${vm_disk}\n"
    [[ -n "$vm_host" ]] && vm_yaml+="          host: \"${vm_host}\"\n"
    vm_yaml+="          roles: ${roles_yaml}${gpu_line}\n"

    if ! tui_yesno "$(i18n "Add VM" "VM 추가")" "$(i18n "Add more VMs to this pool?" "이 풀에 VM을 더 추가하시겠습니까?")"; then
      adding_vm=false
    fi
  done

  # Return pool YAML block via stdout
  echo -e "    - pool_name: \"${pool_name}\""
  echo -e "      purpose: \"${purpose}\""
  echo -e "${place_yaml}"
  echo -e "      node_specs:"
  echo -e "${vm_yaml}" | sed '/^$/d'
}

phase_sdi() {
  phase_skip_if_done 2 && return 0
  log_phase "$(i18n "Phase 2: SDI virtualization setup" "Phase 2: SDI 가상화 설정")"

  local bridge; bridge=$(state_get NET_BRIDGE "br0")
  local cidr; cidr=$(state_get NET_CIDR "10.0.0.0/24")
  local gateway; gateway=$(state_get NET_GW "10.0.0.1")

  # Resource pool
  local pool_rp_name; pool_rp_name=$(tui_input "$(i18n "Resource pool" "리소스 풀")" "$(i18n "Resource pool name:" "리소스 풀 이름:")" "default-pool")
  local dns_str; dns_str=$(tui_input "DNS" "$(i18n "DNS servers (comma-separated):" "DNS 서버 (쉼표 구분):")" "8.8.8.8,8.8.4.4")
  local dns_yaml; dns_yaml=$(echo "$dns_str" | sed 's/ *//g; s/,/", "/g; s/^/["/; s/$/"]/')

  # OS image
  local os_url; os_url=$(tui_input "$(i18n "OS image" "OS 이미지")" "$(i18n "Cloud image URL:" "클라우드 이미지 URL:")" \
    "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img")
  local os_fmt; os_fmt=$(tui_input "$(i18n "Image format" "이미지 포맷")" "$(i18n "Image format:" "이미지 포맷:")" "qcow2")

  # Cloud init
  local ssh_pubkey; ssh_pubkey=$(tui_input "$(i18n "SSH public key" "SSH 공개키")" "$(i18n "SSH public key path:" "SSH 공개키 경로:")" "~/.ssh/id_ed25519.pub")
  local pkg_str; pkg_str=$(tui_input "$(i18n "Packages" "패키지")" "$(i18n "Packages to install (comma-separated):" "설치할 패키지 (쉼표 구분):")" \
    "curl,apt-transport-https,nfs-common,open-iscsi")
  local pkg_yaml; pkg_yaml=$(echo "$pkg_str" | sed 's/ *//g; s/,/, /g; s/^/[/; s/$/]/')

  # Tower pool (required)
  log_info "$(i18n "Tower pool setup (required — for management cluster)" "Tower 풀 설정 (필수 — 관리 클러스터용)")"
  local tower_pool_yaml; tower_pool_yaml=$(collect_vm_specs "tower" "management" 0)

  # Sandbox pool (required)
  log_info "$(i18n "Workload pool setup (required — for first workload cluster)" "워크로드 풀 설정 (필수 — 첫 번째 워크로드 클러스터용)")"
  local sandbox_name; sandbox_name=$(tui_input "$(i18n "Workload pool" "워크로드 풀")" "$(i18n "Workload pool name:" "워크로드 풀 이름:")" "sandbox")
  local sandbox_pool_yaml; sandbox_pool_yaml=$(collect_vm_specs "$sandbox_name" "workload" 1)

  local extra_pools_yaml=""
  POOL_COUNT=2

  while tui_yesno "$(i18n "Additional pool" "추가 풀")" "$(i18n "Create additional SDI pool?" "추가 SDI 풀을 만드시겠습니까?")"; do
    POOL_COUNT=$((POOL_COUNT + 1))
    local ep_name; ep_name=$(tui_input "$(i18n "Pool name" "풀 이름")" "$(i18n "Pool name:" "풀 이름:")" "pool-${POOL_COUNT}")
    local ep_purpose; ep_purpose=$(tui_menu "$(i18n "Purpose" "용도")" "$(i18n "Pool purpose:" "풀 용도:")" \
      "workload" "$(i18n "Workload" "워크로드")" "storage" "$(i18n "Storage" "스토리지")" "monitoring" "$(i18n "Monitoring" "모니터링")")
    local ep_yaml; ep_yaml=$(collect_vm_specs "$ep_name" "$ep_purpose" "$POOL_COUNT")
    extra_pools_yaml+="\n${ep_yaml}"
  done

  # Generate sdi-specs.yaml
  cat > "$GEN_DIR/config/sdi-specs.yaml" << SEOF
resource_pool:
  name: "${pool_rp_name}"
  network:
    management_bridge: "${bridge}"
    management_cidr: "${cidr}"
    gateway: "${gateway}"
    nameservers: ${dns_yaml}

os_image:
  source: "${os_url}"
  format: "${os_fmt}"

cloud_init:
  ssh_authorized_keys_file: "${ssh_pubkey}"
  packages: ${pkg_yaml}

spec:
  sdi_pools:
${tower_pool_yaml}
${sandbox_pool_yaml}$(echo -e "$extra_pools_yaml")
SEOF

  state_set POOL_COUNT "$POOL_COUNT"
  state_set SANDBOX_POOL_NAME "$sandbox_name"
  phase_mark_done 2
  log_info "$(i18n "Phase 2 complete — ${POOL_COUNT} pool(s) configured" "Phase 2 완료 — ${POOL_COUNT}개 풀 구성됨")"
}

# ============================================================================
# Section 5: Phase 3 — Cluster & GitOps
# ============================================================================

collect_cluster() {
  local cname="$1" crole="$2" cid="$3" pool_name="${4:-}"
  local cluster_yaml=""

  if [[ -z "$pool_name" ]]; then
    pool_name=$(tui_input "$(i18n "SDI pool" "SDI 풀")" "$(i18n "SDI pool name for ${cname} cluster:" "${cname} 클러스터의 SDI 풀 이름:")" "$cname")
  fi

  local mode; mode=$(tui_menu "$(i18n "Cluster mode" "클러스터 모드")" "$(i18n "Cluster mode:" "클러스터 모드:")" \
    "sdi" "$(i18n "Use SDI VM pool" "SDI VM 풀 사용")" "baremetal" "$(i18n "Use bare-metal directly" "베어메탈 직접 사용")")

  local ssh_user; ssh_user=$(tui_input "$(i18n "SSH user" "SSH 사용자")" "$(i18n "Cluster SSH user:" "클러스터 SSH 사용자:")" "${USER:-$(id -un 2>/dev/null || echo root)}")

  # Network
  local pod_cidr service_cidr dns_domain
  if [[ "$crole" == "management" ]]; then
    pod_cidr=$(tui_input "$(i18n "Network" "네트워크")" "Pod CIDR:" "10.244.0.0/20")
    service_cidr=$(tui_input "$(i18n "Network" "네트워크")" "Service CIDR:" "10.96.0.0/20")
    dns_domain=$(tui_input "$(i18n "Network" "네트워크")" "$(i18n "DNS domain:" "DNS 도메인:")" "tower.local")
  else
    pod_cidr=$(tui_input "$(i18n "Network" "네트워크")" "Pod CIDR:" "10.233.0.0/17")
    service_cidr=$(tui_input "$(i18n "Network" "네트워크")" "Service CIDR:" "10.233.128.0/18")
    dns_domain=$(tui_input "$(i18n "Network" "네트워크")" "$(i18n "DNS domain:" "DNS 도메인:")" "${cname}.local")
  fi

  cluster_yaml+="    - cluster_name: \"${cname}\"\n"
  [[ "$mode" == "baremetal" ]] && cluster_yaml+="      cluster_mode: \"baremetal\"\n"
  cluster_yaml+="      cluster_sdi_resource_pool: \"${pool_name}\"\n"
  cluster_yaml+="      cluster_role: \"${crole}\"\n"
  cluster_yaml+="      ssh_user: \"${ssh_user}\"\n"
  cluster_yaml+="      network:\n"
  cluster_yaml+="        pod_cidr: \"${pod_cidr}\"\n"
  cluster_yaml+="        service_cidr: \"${service_cidr}\"\n"
  cluster_yaml+="        dns_domain: \"${dns_domain}\"\n"

  if [[ "$crole" == "workload" ]]; then
    local nrc; nrc=$(tui_input "$(i18n "Network" "네트워크")" "$(i18n "Native routing CIDR (leave empty to skip):" "Native routing CIDR (비워두면 생략):")" "")
    [[ -n "$nrc" ]] && cluster_yaml+="        native_routing_cidr: \"${nrc}\"\n"
  fi

  cluster_yaml+="      cilium:\n"
  cluster_yaml+="        cluster_id: ${cid}\n"
  cluster_yaml+="        cluster_name: \"${cname}\"\n"

  # OIDC (workload only)
  if [[ "$crole" == "workload" ]]; then
    if tui_yesno "OIDC" "$(i18n "Enable OIDC authentication?" "OIDC 인증을 활성화하시겠습니까?")"; then
      local oidc_client; oidc_client=$(tui_input "OIDC" "Client ID:" "kubernetes")
      local oidc_issuer; oidc_issuer=$(tui_input "OIDC" "Issuer URL:" "https://auth.example.com/realms/kubernetes")
      cluster_yaml+="      oidc:\n"
      cluster_yaml+="        enabled: true\n"
      cluster_yaml+="        client_id: \"${oidc_client}\"\n"
      cluster_yaml+="        issuer_url: \"${oidc_issuer}\"\n"
      cluster_yaml+="        username_claim: \"preferred_username\"\n"
      cluster_yaml+="        username_prefix: \"oidc:\"\n"
      cluster_yaml+="        groups_claim: \"groups\"\n"
      cluster_yaml+="        groups_prefix: \"oidc:\"\n"
    fi
  fi

  echo -e "$cluster_yaml"
}

phase_cluster() {
  phase_skip_if_done 3 && return 0
  log_phase "$(i18n "Phase 3: Cluster & GitOps setup" "Phase 3: 클러스터 & GitOps 설정")"

  # Common K8s settings
  local k8s_ver; k8s_ver=$(tui_input "Kubernetes" "$(i18n "Kubernetes version:" "Kubernetes 버전:")" "1.33.1")
  local ks_ver; ks_ver=$(tui_input "Kubespray" "$(i18n "Kubespray version:" "Kubespray 버전:")" "v2.30.0")
  local cilium_ver; cilium_ver=$(tui_input "Cilium" "$(i18n "Cilium version:" "Cilium 버전:")" "1.17.5")

  local advanced_common=""
  local kube_proxy_remove="true" cgroup_driver="systemd" helm_enabled="true"
  local gw_api_enabled="true" gw_api_ver="1.3.0" nodelocaldns="true"
  local node_prefix="24" ntp="true" etcd_type="host" dns_mode="coredns"
  local graceful_shutdown="true" graceful_sec="120"

  if tui_yesno "$(i18n "Advanced settings" "고급 설정")" "$(i18n "Modify common Kubernetes advanced settings?" "공통 Kubernetes 고급 설정을 변경하시겠습니까?")"; then
    kube_proxy_remove=$(tui_menu "kube-proxy" "$(i18n "Remove kube-proxy (replaced by Cilium):" "kube-proxy 제거 (Cilium 대체):")" "true" "$(i18n "Yes" "예")" "false" "$(i18n "No" "아니오")")
    cgroup_driver=$(tui_menu "cgroup" "$(i18n "cgroup driver:" "cgroup 드라이버:")" "systemd" "$(i18n "systemd (recommended)" "systemd (권장)")" "cgroupfs" "cgroupfs")
    gw_api_ver=$(tui_input "Gateway API" "$(i18n "Gateway API version:" "Gateway API 버전:")" "$gw_api_ver")
    node_prefix=$(tui_input "$(i18n "Node prefix" "노드 프리픽스")" "$(i18n "Pod network node prefix (/N):" "Pod 네트워크 노드 프리픽스 (/N):")" "$node_prefix")
    etcd_type=$(tui_menu "etcd" "$(i18n "etcd deployment method:" "etcd 배포 방식:")" "host" "$(i18n "Host (recommended)" "Host (권장)")" "kubeadm" "Kubeadm")
  fi

  # Tower cluster (required)
  log_info "$(i18n "Tower cluster setup (required — management cluster)" "Tower 클러스터 설정 (필수 — 관리 클러스터)")"
  local tower_yaml; tower_yaml=$(collect_cluster "tower" "management" 1 "tower")

  # Sandbox cluster (required)
  local sandbox_pool; sandbox_pool=$(state_get SANDBOX_POOL_NAME "sandbox")
  log_info "$(i18n "Workload cluster setup (required)" "워크로드 클러스터 설정 (필수)")"
  local sandbox_name; sandbox_name=$(tui_input "$(i18n "Cluster" "클러스터")" "$(i18n "Workload cluster name:" "워크로드 클러스터 이름:")" "sandbox")
  local sandbox_yaml; sandbox_yaml=$(collect_cluster "$sandbox_name" "workload" 2 "$sandbox_pool")

  local extra_clusters_yaml=""
  CLUSTER_COUNT=2
  local managed_list="\"${sandbox_name}\""

  while tui_yesno "$(i18n "Additional cluster" "추가 클러스터")" "$(i18n "Add more clusters?" "클러스터를 더 추가하시겠습니까?")"; do
    CLUSTER_COUNT=$((CLUSTER_COUNT + 1))
    local ec_name; ec_name=$(tui_input "$(i18n "Cluster name" "클러스터 이름")" "$(i18n "Cluster name:" "클러스터 이름:")" "cluster-${CLUSTER_COUNT}")
    local ec_yaml; ec_yaml=$(collect_cluster "$ec_name" "workload" "$CLUSTER_COUNT" "")
    extra_clusters_yaml+="\n${ec_yaml}"
    managed_list+=", \"${ec_name}\""
  done

  # ArgoCD settings
  local argo_ns; argo_ns=$(tui_input "ArgoCD" "$(i18n "Namespace:" "네임스페이스:")" "argocd")
  local argo_repo; argo_repo=$(tui_input "ArgoCD" "$(i18n "Git repo URL:" "Git 리포 URL:")" "$REPO_URL")
  local argo_branch; argo_branch=$(tui_input "ArgoCD" "$(i18n "Branch:" "브랜치:")" "main")

  # Domains
  local dom_auth; dom_auth=$(tui_input "$(i18n "Domain" "도메인")" "$(i18n "Auth domain (Keycloak):" "Auth 도메인 (Keycloak):")" "auth.example.com")
  local dom_argo; dom_argo=$(tui_input "$(i18n "Domain" "도메인")" "$(i18n "ArgoCD domain:" "ArgoCD 도메인:")" "cd.example.com")
  local dom_api; dom_api=$(tui_input "$(i18n "Domain" "도메인")" "$(i18n "K8s API domain:" "K8s API 도메인:")" "api.k8s.example.com")

  # Secrets
  log_info "$(i18n "Secrets setup" "시크릿 설정")"
  local kc_admin_pw; kc_admin_pw=$(tui_password "Keycloak" "$(i18n "Keycloak admin password:" "Keycloak Admin 비밀번호:")")
  local kc_db_pw; kc_db_pw=$(tui_password "Keycloak" "$(i18n "Keycloak DB password:" "Keycloak DB 비밀번호:")")
  local argo_pat; argo_pat=$(tui_password "ArgoCD" "$(i18n "GitHub PAT (leave empty if not a private repo):" "GitHub PAT (비공개 리포가 아니면 비워두세요):")")

  # Cloudflare
  local cf_enabled=false cf_account="" cf_secret="" cf_tunnel_id=""
  if tui_yesno "Cloudflare Tunnel" "$(i18n "Use Cloudflare Tunnel?" "Cloudflare Tunnel을 사용하시겠습니까?")"; then
    cf_enabled=true
    cf_account=$(tui_input "Cloudflare" "Account Tag:" "")
    cf_secret=$(tui_password "Cloudflare" "Tunnel Secret:")
    cf_tunnel_id=$(tui_input "Cloudflare" "Tunnel ID:" "")
  fi

  # App selection
  log_info "$(i18n "App selection" "앱 선택")"
  local common_apps; common_apps=$(tui_checklist "$(i18n "Common apps" "공통 앱")" "$(i18n "Apps to install on all clusters:" "모든 클러스터에 설치할 앱:")" \
    "cilium-resources"  "$(i18n "Cilium resources (manifest)" "Cilium 리소스 (manifest)")" "ON" \
    "cert-manager"      "cert-manager v1.18.2"     "ON" \
    "kyverno"           "Kyverno 3.3.7"            "ON" \
    "kyverno-policies"  "$(i18n "Kyverno policies (manifest)" "Kyverno 정책 (manifest)")"   "ON")
  echo "$common_apps" > "$INSTALLER_DIR/apps_selected.txt"

  local tower_apps; tower_apps=$(tui_checklist "$(i18n "Tower apps" "Tower 앱")" "$(i18n "Tower cluster apps:" "Tower 클러스터 앱:")" \
    "cilium"            "Cilium ${cilium_ver}"       "ON" \
    "argocd"            "ArgoCD 8.1.1"               "ON" \
    "cluster-config"    "$(i18n "Cluster config (manifest)" "클러스터 설정 (manifest)")"    "ON" \
    "cert-issuers"      "$(i18n "Cert issuers (manifest)" "인증서 발급자 (manifest)")"    "ON" \
    "keycloak"          "Keycloak 25.1.2"            "ON" \
    "cloudflared-tunnel" "Cloudflare Tunnel 2.1.2"   "$(${cf_enabled} && echo ON || echo OFF)" \
)
  echo "$tower_apps" >> "$INSTALLER_DIR/apps_selected.txt"

  local sandbox_apps; sandbox_apps=$(tui_checklist "$(i18n "Sandbox apps" "Sandbox 앱")" "$(i18n "${sandbox_name} cluster apps:" "${sandbox_name} 클러스터 앱:")" \
    "cilium"                  "Cilium ${cilium_ver}"                "ON" \
    "cluster-config"          "$(i18n "Cluster config (manifest)" "클러스터 설정 (manifest)")"             "ON" \
    "local-path-provisioner"  "Local Path Provisioner v0.0.32"     "ON" \
    "rbac"                    "RBAC (manifest)"                    "ON" \
    "test-resources"          "$(i18n "Test resources (manifest)" "테스트 리소스 (manifest)")"             "OFF")
  echo "$sandbox_apps" >> "$INSTALLER_DIR/apps_selected.txt"

  # Generate k8s-clusters.yaml
  cat > "$GEN_DIR/config/k8s-clusters.yaml" << KEOF
config:
  common:
    kubernetes_version: "${k8s_ver}"
    kubespray_version: "${ks_ver}"
    container_runtime: "containerd"
    cni: "cilium"
    cilium_version: "${cilium_ver}"
    kube_proxy_remove: ${kube_proxy_remove}
    cgroup_driver: "${cgroup_driver}"
    helm_enabled: ${helm_enabled}
    kube_apiserver_admission_plugins:
      - NodeRestriction
      - PodTolerationRestriction
    firewalld_enabled: false
    kube_vip_enabled: false
    graceful_node_shutdown: ${graceful_shutdown}
    graceful_node_shutdown_sec: ${graceful_sec}
    kubelet_custom_flags:
      - "--node-ip={{ ip }}"
    gateway_api_enabled: ${gw_api_enabled}
    gateway_api_version: "${gw_api_ver}"
    kubeconfig_localhost: true
    kubectl_localhost: true
    enable_nodelocaldns: ${nodelocaldns}
    kube_network_node_prefix: ${node_prefix}
    ntp_enabled: ${ntp}
    etcd_deployment_type: "${etcd_type}"
    dns_mode: "${dns_mode}"

  clusters:
$(echo -e "${tower_yaml}")
$(echo -e "${sandbox_yaml}")$(echo -e "${extra_clusters_yaml}")

  argocd:
    namespace: "${argo_ns}"
    repo_url: "${argo_repo}"
    repo_branch: "${argo_branch}"
    tower_manages: [${managed_list}]

  domains:
    auth: "${dom_auth}"
    argocd: "${dom_argo}"
    k8s_api: "${dom_api}"
KEOF

  # Generate secrets.yaml
  cat > "$GEN_DIR/credentials/secrets.yaml" << SECEOF
keycloak:
  admin_password: "${kc_admin_pw}"
  db_password: "${kc_db_pw}"

argocd:
  repo_pat: "${argo_pat}"

cloudflare:
  credentials_file: "credentials/cloudflare-tunnel.json"
  cert_file: ""
SECEOF
  chmod 600 "$GEN_DIR/credentials/secrets.yaml"

  # Generate cloudflare-tunnel.json if enabled
  if $cf_enabled; then
    cat > "$GEN_DIR/credentials/cloudflare-tunnel.json" << CFEOF
{
  "AccountTag": "${cf_account}",
  "TunnelSecret": "${cf_secret}",
  "TunnelID": "${cf_tunnel_id}"
}
CFEOF
    chmod 600 "$GEN_DIR/credentials/cloudflare-tunnel.json"
  fi

  state_set CLUSTER_COUNT "$CLUSTER_COUNT"
  state_set SANDBOX_NAME "$sandbox_name"
  state_set REPO_URL_USER "$argo_repo"
  phase_mark_done 3
  log_info "$(i18n "Phase 3 complete — ${CLUSTER_COUNT} cluster(s) configured" "Phase 3 완료 — ${CLUSTER_COUNT}개 클러스터 구성됨")"
}

# ============================================================================
# Section 6: Phase 4 — Clone, Build, Provision
# ============================================================================

run_step() {
  local step_num="$1" total="$2" desc="$3"; shift 3
  echo -e "${CYAN}[Phase 4/4] [Step ${step_num}/${total}]${NC} ${desc}"
  log_raw "STEP ${step_num}/${total}: ${desc}"
  if "$@"; then
    echo -e "  ${GREEN}OK${NC}"
    return 0
  else
    echo -e "  ${RED}FAIL${NC}"
    log_error "Step ${step_num} failed: $desc"
    if [[ "$AUTO_MODE" == "true" ]]; then return 1; fi
    if tui_yesno "$(i18n "Retry" "재시도")" "$(i18n "${desc} failed. Retry?" "${desc} 실패. 재시도하시겠습니까?")"; then
      "$@"
    else
      return 1
    fi
  fi
}

phase_provision() {
  phase_skip_if_done 4 && return 0
  log_phase "$(i18n "Phase 4: Build & provisioning" "Phase 4: 빌드 & 프로비저닝")"
  local total_steps=6
  local repo_url; repo_url=$(state_get REPO_URL_USER "$REPO_URL")

  # Step 1: Clone or locate repo (priority: cwd > env var > $HOME/ScaleX-POD-mini > clone)
  echo -e "${CYAN}[Phase 4/4] [Step 1/${total_steps}]${NC} $(i18n "Preparing repository..." "리포지토리 준비...")"
  if [[ -d "$(pwd)/.git" && -f "$(pwd)/install.sh" ]]; then
    REPO_DIR="$(pwd)"
    log_info "$(i18n "Using current directory as repo: $REPO_DIR" "현재 디렉토리를 리포로 사용: $REPO_DIR")"
  elif [[ -n "${SCALEX_REPO_DIR:-}" && -d "$SCALEX_REPO_DIR/.git" ]]; then
    REPO_DIR="$SCALEX_REPO_DIR"
    log_info "$(i18n "Using existing repo: $REPO_DIR" "기존 리포 사용: $REPO_DIR")"
  elif [[ -d "$HOME/ScaleX-POD-mini/.git" ]]; then
    REPO_DIR="$HOME/ScaleX-POD-mini"
    log_info "$(i18n "Local repo found: $REPO_DIR" "로컬 리포 발견: $REPO_DIR")"
    if tui_yesno "$(i18n "Repository" "리포지토리")" "$(i18n "Use existing repo?\n${REPO_DIR}" "기존 리포를 사용하시겠습니까?\n${REPO_DIR}")"; then
      log_info "$(i18n "Using existing repo" "기존 리포 사용")"
    else
      REPO_DIR="$HOME/ScaleX-POD-mini"
      git clone "$repo_url" "$REPO_DIR" 2>&1 | tail -1
    fi
  else
    REPO_DIR="$HOME/ScaleX-POD-mini"
    log_info "$(i18n "Cloning repo: $repo_url" "리포 클론 중: $repo_url")"
    git clone "$repo_url" "$REPO_DIR" 2>&1 | tail -1
  fi
  echo -e "  ${GREEN}OK${NC} — $REPO_DIR"
  state_set REPO_DIR "$REPO_DIR"

  # Step 2: Copy generated config files (skip in auto mode — repo already has correct config)
  echo -e "${CYAN}[Phase 4/4] [Step 2/${total_steps}]${NC} $(i18n "Copying config files..." "구성 파일 복사...")"
  if [[ "$AUTO_MODE" == "true" ]]; then
    log_info "$(i18n "Auto mode: keeping existing config files (skipping overwrite)" "자동 모드: 기존 구성 파일 유지 (덮어쓰기 건너뜀)")"
  else
    mkdir -p "$REPO_DIR/credentials" "$REPO_DIR/config"
    if [[ -f "$GEN_DIR/credentials/.baremetal-init.yaml" ]]; then
      cp "$GEN_DIR/credentials/.baremetal-init.yaml" "$REPO_DIR/credentials/"
      chmod 600 "$REPO_DIR/credentials/.baremetal-init.yaml"
    fi
    if [[ -f "$GEN_DIR/credentials/.env" ]]; then
      cp "$GEN_DIR/credentials/.env" "$REPO_DIR/credentials/"
      chmod 600 "$REPO_DIR/credentials/.env"
    fi
    if [[ -f "$GEN_DIR/credentials/secrets.yaml" ]]; then
      cp "$GEN_DIR/credentials/secrets.yaml" "$REPO_DIR/credentials/"
      chmod 600 "$REPO_DIR/credentials/secrets.yaml"
    fi
    if [[ -f "$GEN_DIR/credentials/cloudflare-tunnel.json" ]]; then
      cp "$GEN_DIR/credentials/cloudflare-tunnel.json" "$REPO_DIR/credentials/"
      chmod 600 "$REPO_DIR/credentials/cloudflare-tunnel.json"
    fi
    if [[ -f "$GEN_DIR/config/sdi-specs.yaml" ]]; then
      cp "$GEN_DIR/config/sdi-specs.yaml" "$REPO_DIR/config/"
    fi
    if [[ -f "$GEN_DIR/config/k8s-clusters.yaml" ]]; then
      cp "$GEN_DIR/config/k8s-clusters.yaml" "$REPO_DIR/config/"
    fi
  fi
  echo -e "  ${GREEN}OK${NC}"

  # Step 3: Build scalex CLI
  echo -e "${CYAN}[Phase 4/4] [Step 3/${total_steps}]${NC} $(i18n "Building scalex CLI..." "scalex CLI 빌드...")"
  if [[ -d "$REPO_DIR/scalex-cli" ]]; then
    (cd "$REPO_DIR/scalex-cli" && cargo build --release 2>&1 | tail -3) || {
      error_msg "$(i18n "scalex CLI build failed" "scalex CLI 빌드 실패")" "$(i18n "Rust compilation error" "Rust 컴파일 오류")" "$(i18n "Check cargo build logs" "cargo build 로그를 확인하세요")"
      # In auto mode: scalex CLI is required for all provisioning steps — fail fast with non-zero exit
      if [[ "$AUTO_MODE" == "true" ]]; then
        log_error "$(i18n "Auto mode: scalex CLI build is required for provisioning — aborting" "자동 모드: 프로비저닝에 scalex CLI 빌드가 필요함 — 중단")"
        return 1
      fi
      if ! tui_yesno "$(i18n "Continue" "계속")" "$(i18n "Build failed. Continue anyway?" "빌드 실패. 그래도 계속하시겠습니까?")"; then return 1; fi
    }
    mkdir -p "$HOME/.local/bin"
    cp "$REPO_DIR/scalex-cli/target/release/scalex-pod" "$HOME/.local/bin/scalex-pod" 2>/dev/null || true
    chmod +x "$HOME/.local/bin/scalex-pod" 2>/dev/null || true
    echo -e "  ${GREEN}OK${NC} — ~/.local/bin/scalex-pod"
  else
    log_warn "$(i18n "scalex-cli directory not found. Skipping." "scalex-cli 디렉토리를 찾을 수 없습니다. 건너뜁니다.")"
  fi

  # Step 4: Validate configs
  echo -e "${CYAN}[Phase 4/4] [Step 4/${total_steps}]${NC} $(i18n "Validating config..." "구성 검증...")"
  if command -v scalex-pod &>/dev/null; then
    (cd "$REPO_DIR" && scalex-pod get config-files 2>&1) || log_warn "$(i18n "Config validation warnings detected" "구성 검증 경고 발생")"
  else
    log_warn "$(i18n "scalex-pod CLI not in PATH. Skipping validation." "scalex-pod CLI가 PATH에 없습니다. 검증을 건너뜁니다.")"
  fi
  echo -e "  ${GREEN}OK${NC}"

  # Step 5: Auto-provisioning
  echo -e "${CYAN}[Phase 4/4] [Step 5/${total_steps}]${NC} $(i18n "Auto-provisioning..." "자동 프로비저닝...")"
  if tui_yesno "$(i18n "Provisioning" "프로비저닝")" "$(i18n "Start auto-provisioning?\n\nThe following tasks will run in order:\n1. facts --all\n2. sdi init\n3. cluster init\n4. API tunnel setup\n5. secrets apply (management + workload)\n6. bootstrap" "자동 프로비저닝을 시작하시겠습니까?\n\n다음 작업이 순서대로 실행됩니다:\n1. facts --all\n2. sdi init\n3. cluster init\n4. API 터널 설정\n5. secrets apply (management + workload)\n6. bootstrap")"; then

    if [[ "$AUTO_MODE" == "true" ]]; then
      # --- Auto mode: explicit step ordering with API tunnel lifecycle ---
      local ps_total=9

      echo -e "  ${CYAN}[1/${ps_total}]${NC} scalex-pod facts --all..."
      if ! (cd "$REPO_DIR" && scalex-pod facts --all 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: facts failed — aborting provisioning" "자동 모드: facts 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      # SSH health check: pre-sdi-init — verify nodes still reachable before SDI/VM deployment
      phase_ssh_check "$(i18n "pre-sdi-init" "SDI 초기화 전")" "$REPO_DIR" || return 1

      # Disable NIC offload on bare-metal nodes (e1000e hang prevention)
      disable_nic_offload "$REPO_DIR"

      if phase4_step_is_done 2; then
        echo -e "  ${CYAN}[2/${ps_total}]${NC} scalex-pod sdi init — $(i18n "already done, skipping" "이미 완료됨, 건너뜀")"
      else
        echo -e "  ${CYAN}[2/${ps_total}]${NC} scalex-pod sdi init..."
        if ! (cd "$REPO_DIR" && scalex-pod sdi init config/sdi-specs.yaml 2>&1 | tee -a "$LOG_FILE" | tail -5); then
          log_error "$(i18n "Auto mode: SDI init failed — aborting provisioning" "자동 모드: SDI 초기화 실패 — 프로비저닝 중단")"
          return 1
        fi
        phase4_step_mark_done 2
        echo -e "  ${GREEN}OK${NC}"
      fi

      # SSH health check: pre-cluster-init — verify nodes reachable before k8s bootstrap
      phase_ssh_check "$(i18n "pre-cluster-init" "클러스터 초기화 전")" "$REPO_DIR" || return 1

      if phase4_step_is_done 3; then
        echo -e "  ${CYAN}[3/${ps_total}]${NC} scalex-pod cluster init — $(i18n "already done, skipping" "이미 완료됨, 건너뜀")"
      else
        echo -e "  ${CYAN}[3/${ps_total}]${NC} scalex-pod cluster init..."
        # Pre-fix /opt/cni/bin permissions on all VMs BEFORE Kubespray (Kubespray sets kube:root 755
        # during CNI plugin install, but Cilium init container needs write access immediately after)
        local _sdi_state_pre="${REPO_DIR}/_generated/sdi/sdi-state.json"
        if [[ -f "$_sdi_state_pre" ]]; then
          log_info "$(i18n "Pre-Kubespray: ensuring /opt/cni/bin permissions on all VMs..." "Kubespray 전: 모든 VM에 /opt/cni/bin 권한 설정...")"
          local _pre_ips; _pre_ips=$(python3 -c "
import json
with open('${_sdi_state_pre}') as f:
    pools = json.load(f)
if not isinstance(pools, list): pools = [pools]
for pool in pools:
    for node in pool.get('nodes', []):
        print(node.get('ip', ''))
" 2>/dev/null)
          while IFS= read -r _pre_ip; do
            [[ -z "$_pre_ip" ]] && continue
            ssh -i ~/.ssh/id_ed25519 -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=no \
              -J playbox-0 "ubuntu@${_pre_ip}" 'sudo mkdir -p /opt/cni/bin && sudo chmod 777 /opt/cni/bin' 2>/dev/null || true
          done <<< "$_pre_ips"
        fi
        if ! (cd "$REPO_DIR" && scalex-pod cluster init config/k8s-clusters.yaml 2>&1 | tee -a "$LOG_FILE" | tail -5); then
          log_error "$(i18n "Auto mode: cluster init failed — aborting provisioning" "자동 모드: 클러스터 초기화 실패 — 프로비저닝 중단")"
          return 1
        fi
        # Fix /opt/cni/bin permissions on all VMs after Kubespray (it sets kube:root 755,
        # but Cilium init container needs write access to copy cilium-mount binary)
        log_info "$(i18n "Fixing /opt/cni/bin permissions on all VMs..." "/opt/cni/bin 권한 수정 중...")"
        local _sdi_state="${REPO_DIR}/_generated/sdi/sdi-state.json"
        if [[ -f "$_sdi_state" ]]; then
          local _vm_ips; _vm_ips=$(python3 -c "
import json
with open('${_sdi_state}') as f:
    pools = json.load(f)
if not isinstance(pools, list): pools = [pools]
for pool in pools:
    for node in pool.get('nodes', []):
        print(node.get('ip', ''))
" 2>/dev/null)
          while IFS= read -r _vm_ip; do
            [[ -z "$_vm_ip" ]] && continue
            ssh -i ~/.ssh/id_ed25519 -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=no \
              -J playbox-0 "ubuntu@${_vm_ip}" 'sudo chmod 777 /opt/cni/bin' 2>/dev/null || true
          done <<< "$_vm_ips"
          log_info "$(i18n "/opt/cni/bin permissions fixed on all VMs" "/opt/cni/bin 권한 수정 완료")"
        fi
        phase4_step_mark_done 3
        echo -e "  ${GREEN}OK${NC}"
      fi

      # Set up API tunnels AFTER cluster init creates kubeconfigs
      echo -e "  ${CYAN}[4/${ps_total}]${NC} $(i18n "API access tunnel setup..." "API 접근 터널 설정...")"
      if ! setup_api_tunnels "$REPO_DIR"; then
        log_error "$(i18n "Auto mode: API tunnel setup failed — aborting provisioning" "자동 모드: API 터널 설정 실패 — 프로비저닝 중단")"
        return 1
      fi
      # Tunnel readiness health-check: verify all tunnels are live and accepting connections
      # before proceeding to any step requiring kubectl/API access.
      echo -e "  ${CYAN}[5/${ps_total}]${NC} $(i18n "Tunnel readiness check — verifying tunnels live and API servers reachable..." \
        "터널 준비 상태 확인 — 터널 활성 및 API 서버 접근 가능 여부 확인...")"
      if ! verify_api_tunnels_ready "$REPO_DIR" 120; then
        log_error "$(i18n "Auto mode: tunnel readiness check failed — aborting provisioning" \
          "자동 모드: 터널 준비 상태 확인 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[6/${ps_total}]${NC} scalex-pod secrets apply (management)..."
      if ! (cd "$REPO_DIR" && scalex-pod secrets apply --role management 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: management cluster secrets apply failed — aborting provisioning" "자동 모드: 관리 클러스터 시크릿 적용 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[7/${ps_total}]${NC} scalex-pod secrets apply (workload)..."
      if ! (cd "$REPO_DIR" && scalex-pod secrets apply --role workload 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_warn "$(i18n "Workload secrets apply failed — continuing" "워크로드 시크릿 적용 실패 — 계속 진행")"
      fi
      echo -e "  ${GREEN}OK${NC}"

      # Verify API tunnels are alive + API servers ready before bootstrap (second gate)
      echo -e "  ${CYAN}[8/${ps_total}]${NC} $(i18n "Pre-bootstrap tunnel gate + scalex-pod bootstrap..." \
        "부트스트랩 전 터널 게이트 확인 + scalex-pod bootstrap...")"
      if ! verify_api_tunnels_ready "$REPO_DIR" 120; then
        log_error "$(i18n "Auto mode: pre-bootstrap tunnel readiness check failed — aborting" \
          "자동 모드: 부트스트랩 전 터널 준비 상태 확인 실패 — 중단")"
        return 1
      fi
      # SSH health check: pre-bootstrap — verify nodes reachable before ArgoCD/app deployment
      phase_ssh_check "$(i18n "pre-bootstrap" "부트스트랩 전")" "$REPO_DIR" || return 1
      if ! (cd "$REPO_DIR" && scalex-pod bootstrap 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: bootstrap failed — aborting provisioning" "자동 모드: 부트스트랩 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      # Step 9: Final pre-exit tunnel connectivity verification.
      # Called BEFORE cleanup_api_tunnels so SSH tunnel processes are still alive.
      # Uses scalex-pod tunnel status (preferred) or direct port probe (fallback).
      # Non-fatal: warns if connectivity cannot be confirmed but does not abort.
      echo -e "  ${CYAN}[9/${ps_total}]${NC} $(i18n "Verifying tunnel connectivity before exit..." \
        "종료 전 터널 연결 확인...")"
      verify_exit_tunnel_connectivity 30
      echo -e "  ${GREEN}OK${NC}"

      # Clean up API tunnels — restore original kubeconfigs with real CP IPs
      cleanup_api_tunnels

    else
      # --- Interactive mode: explicit steps with tunnel support ---
      local prov_pre_tunnel=("scalex-pod facts --all"
                             "scalex-pod sdi init config/sdi-specs.yaml"
                             "scalex-pod cluster init config/k8s-clusters.yaml")
      local prov_post_tunnel=("scalex-pod secrets apply --role management"
                              "scalex-pod secrets apply --role workload"
                              "scalex-pod bootstrap")
      local ps_i=0 ps_total=$(( ${#prov_pre_tunnel[@]} + 1 + ${#prov_post_tunnel[@]} ))

      # Pre-tunnel steps (no kubectl needed)
      for cmd in "${prov_pre_tunnel[@]}"; do
        ps_i=$((ps_i + 1))
        echo -e "  ${CYAN}[${ps_i}/${ps_total}]${NC} ${cmd}..."
        if (cd "$REPO_DIR" && eval "$cmd" 2>&1 | tee -a "$LOG_FILE" | tail -5); then
          echo -e "  ${GREEN}OK${NC}"
        else
          log_error "$(i18n "Provisioning failed: $cmd" "프로비저닝 실패: $cmd")"
          if ! tui_yesno "$(i18n "Continue" "계속")" "$(i18n "${cmd} failed. Continue to next step?" "${cmd} 실패. 다음 단계로 계속하시겠습니까?")"; then
            break
          fi
        fi
      done

      # Set up API tunnels (after cluster init creates kubeconfigs)
      ps_i=$((ps_i + 1))
      echo -e "  ${CYAN}[${ps_i}/${ps_total}]${NC} $(i18n "API access tunnel setup..." "API 접근 터널 설정...")"
      setup_api_tunnels "$REPO_DIR" || log_warn "$(i18n "API tunnel setup failed — attempting direct access" "API 터널 설정 실패 — 직접 접근으로 시도")"
      echo -e "  ${GREEN}OK${NC}"

      # Post-tunnel steps (kubectl/helm needed)
      for cmd in "${prov_post_tunnel[@]}"; do
        ps_i=$((ps_i + 1))
        echo -e "  ${CYAN}[${ps_i}/${ps_total}]${NC} ${cmd}..."
        if (cd "$REPO_DIR" && eval "$cmd" 2>&1 | tee -a "$LOG_FILE" | tail -5); then
          echo -e "  ${GREEN}OK${NC}"
        else
          log_error "$(i18n "Provisioning failed: $cmd" "프로비저닝 실패: $cmd")"
          if ! tui_yesno "$(i18n "Continue" "계속")" "$(i18n "${cmd} failed. Continue to next step?" "${cmd} 실패. 다음 단계로 계속하시겠습니까?")"; then
            break
          fi
        fi
      done

      # Clean up API tunnels
      cleanup_api_tunnels
    fi

  else
    log_info "$(i18n "Skipping provisioning. Run manually." "프로비저닝을 건너뜁니다. 수동으로 실행하세요.")"
  fi

  # Step 6: Save kubeconfigs
  echo -e "${CYAN}[Phase 4/4] [Step 6/${total_steps}]${NC} $(i18n "Saving kubeconfig..." "kubeconfig 저장...")"
  mkdir -p "$SCALEX_HOME/credentials"
  if [[ -d "$REPO_DIR/_generated" ]]; then
    find "$REPO_DIR/_generated" -name "kubeconfig.yaml" -o -name "admin.conf" -o -name "config" 2>/dev/null | while read -r kc; do
      local cluster_dir; cluster_dir=$(basename "$(dirname "$kc")")
      mkdir -p "$SCALEX_HOME/credentials/${cluster_dir}"
      cp "$kc" "$SCALEX_HOME/credentials/${cluster_dir}/config"
      chmod 600 "$SCALEX_HOME/credentials/${cluster_dir}/config"
      log_info "$(i18n "kubeconfig saved: ${cluster_dir}" "kubeconfig 저장: ${cluster_dir}")"
    done
  fi
  echo -e "  ${GREEN}OK${NC}"

  phase_mark_done 4
  log_info "$(i18n "Phase 4 complete" "Phase 4 완료")"
}

# ============================================================================
# Section 7: Orchestrator
# ============================================================================

show_dashboard() {
  local completed; completed=$(state_get_phase)
  local lines=""
  local i
  for i in 0 1 2 3 4; do
    local label; label=$(phase_label "$i")
    local status icon
    if phase_is_done "$i"; then
      icon="\\xE2\\x9C\\x93"; status="Done"  # checkmark (per-phase marker)
    elif (( i == completed + 1 )); then
      icon="\\xE2\\x96\\xB6"; status="Active"  # play arrow
    else
      icon=" "; status="Pending"
    fi
    lines+="$(printf "  [%b] Phase %d: %-24s %s\n" "$icon" "$i" "$label" "$status")"$'\n'
  done

  echo ""
  echo -e "${BOLD}${BLUE}+------------------------------------------------------+${NC}"
  echo -e "${BOLD}${BLUE}|  ScaleX Installer                             v${VERSION}  |${NC}"
  echo -e "${BOLD}${BLUE}+------------------------------------------------------+${NC}"
  echo -e "$lines"
  echo -e "${BOLD}${BLUE}+------------------------------------------------------+${NC}"
  echo ""
}

resume_check() {
  local completed; completed=$(state_get_phase)
  if (( completed >= 0 )); then
    echo ""
    log_info "$(i18n "Previous install state found (Phase ${completed} completed)" "이전 설치 상태가 발견되었습니다 (Phase ${completed} 완료)")"
    show_dashboard
    local choice; choice=$(tui_menu "$(i18n "Resume" "재개")" "$(i18n "How would you like to proceed?" "어떻게 진행하시겠습니까?")" \
      "continue" "$(i18n "Continue from where left off" "이어서 진행")" \
      "reset"    "$(i18n "Restart from a specific phase" "특정 Phase부터 재시작")" \
      "fresh"    "$(i18n "Start from scratch" "처음부터 시작")")
    case "$choice" in
      continue) return 0 ;;
      reset)
        local phase; phase=$(tui_input "$(i18n "Phase selection" "Phase 선택")" "$(i18n "Phase number to restart from (0-4):" "재시작할 Phase 번호 (0-4):")" "")
        if [[ "$phase" =~ ^[0-4]$ ]]; then
          local prev=$((phase - 1))
          (( prev < 0 )) && prev=-1
          state_save_phase "$prev"
          # Remove per-phase markers for phases that will be re-run
          local rp
          for rp in 0 1 2 3 4; do
            (( rp >= phase )) && rm -f "$PHASE_DONE_DIR/${rp}.done" 2>/dev/null || true
          done
          # Also clear phase 4 sub-step markers when resetting to/before phase 4
          # (4s2.done = sdi init, 4s3.done = cluster init)
          (( 4 >= phase )) && phase4_clear_steps || true
        fi
        ;;
      fresh)
        if tui_yesno "$(i18n "Confirm" "확인")" "$(i18n "Reset all state?" "모든 상태를 초기화하시겠습니까?")"; then
          rm -f "$STATE_FILE" "$PHASE_FILE"
          rm -f "$INSTALLER_DIR/nodes.txt" "$INSTALLER_DIR/pools.txt"
          rm -f "$INSTALLER_DIR/clusters.txt" "$INSTALLER_DIR/apps_selected.txt"
          rm -f "$PHASE_DONE_DIR"/*.done 2>/dev/null || true
          rm -rf "$GEN_DIR"
          mkdir -p "$GEN_DIR/credentials" "$GEN_DIR/config" "$PHASE_DONE_DIR"
          state_save_phase -1
        fi
        ;;
    esac
  fi
}

post_install_summary() {
  local repo_dir; repo_dir=$(state_get REPO_DIR "$HOME/ScaleX-POD-mini")

  echo ""
  echo -e "${BOLD}${GREEN}============================================================${NC}"
  echo -e "${BOLD}${GREEN}  $(i18n "ScaleX installation complete!" "ScaleX 설치 완료!")${NC}"
  echo -e "${BOLD}${GREEN}============================================================${NC}"
  echo ""
  echo -e "  ${BOLD}$(i18n "Repository:" "리포지토리:")${NC}  ${repo_dir}"
  echo -e "  ${BOLD}CLI:${NC}         ~/.local/bin/scalex-pod"
  echo -e "  ${BOLD}$(i18n "State:" "상태:")${NC}        ${INSTALLER_DIR}"
  echo -e "  ${BOLD}$(i18n "Logs:" "로그:")${NC}        ${LOG_FILE}"
  echo ""

  if [[ -d "$SCALEX_HOME/credentials" ]]; then
    echo -e "  ${BOLD}Kubeconfigs:${NC}"
    find "$SCALEX_HOME/credentials" -name "config" 2>/dev/null | while read -r kc; do
      local cname; cname=$(basename "$(dirname "$kc")")
      echo -e "    - ${cname}: ${kc}"
    done
    echo ""
  fi

  echo -e "  ${BOLD}$(i18n "Next steps:" "다음 단계:")${NC}"
  echo -e "    export PATH=\"\$HOME/.local/bin:\$PATH\""
  echo -e "    cd ${repo_dir}"
  echo -e "    scalex-pod get config-files    # $(i18n "Check config" "구성 확인")"
  echo -e "    scalex-pod status              # $(i18n "Cluster status" "클러스터 상태")"
  echo ""
  echo -e "  ${BOLD}$(i18n "ArgoCD dashboard:" "ArgoCD 대시보드:")${NC}"
  echo -e "    https://$(grep -o 'argocd:.*' "$GEN_DIR/config/k8s-clusters.yaml" 2>/dev/null | head -1 | awk '{print $2}' | tr -d '"' || echo "cd.example.com")"
  echo ""
}

parse_args() {
  for arg in "$@"; do
    case "$arg" in
      --auto) AUTO_MODE=true ;;
    esac
  done
  # Auto-enable when stdin is not a terminal (curl|bash, wget|bash)
  [[ ! -t 0 ]] && AUTO_MODE=true
}

main() {
  init_dirs
  parse_args "$@"

  echo -e "\n${BOLD}${BLUE}"
  echo "  ____            _       __  __"
  echo " / ___|  ___ __ _| | ___\ \/ /"
  echo " \___ \ / __/ _\` | |/ _ \\\\  / "
  echo "  ___) | (_| (_| | |  __//  \\ "
  echo " |____/ \\___\\__,_|_|\\___/_/\\_\\"
  echo -e "${NC}"
  echo -e "  ${BOLD}ScaleX Interactive Installer v${VERSION}${NC}"
  echo ""

  detect_tui
  log_raw "=== ScaleX Installer v${VERSION} started ==="
  log_raw "OS: $(uname -srm), TUI: ${TUI}"

  # --- Auto mode: skip TUI phases, run deps + provision directly ---
  if [[ "$AUTO_MODE" == "true" ]]; then
    log_info "$(i18n "Auto mode enabled — skipping Phases 1-3" "자동 모드 활성화 — Phases 1-3 건너뜀")"

    # Locate repo: current dir > env var > well-known path
    local repo=""
    if [[ -d "$(pwd)/.git" && -f "$(pwd)/install.sh" ]]; then
      repo="$(pwd)"
    elif [[ -n "${SCALEX_REPO_DIR:-}" && -d "${SCALEX_REPO_DIR}/.git" ]]; then
      repo="$SCALEX_REPO_DIR"
    elif [[ -d "$HOME/ScaleX-POD-mini/.git" ]]; then
      repo="$HOME/ScaleX-POD-mini"
    fi

    # Pre-flight: check repo was found
    if [[ -z "$repo" ]]; then
      error_msg "$(i18n "Repository not found" "리포지토리를 찾을 수 없음")" \
        "$(i18n "No ScaleX repo detected in cwd, SCALEX_REPO_DIR, or \$HOME/ScaleX-POD-mini" "현재 디렉토리, SCALEX_REPO_DIR, \$HOME/ScaleX-POD-mini에서 리포를 찾을 수 없습니다")" \
        "$(i18n "Clone the repo first or set SCALEX_REPO_DIR" "먼저 리포를 클론하거나 SCALEX_REPO_DIR을 설정하세요")"
      return 1
    fi
    log_info "$(i18n "Using repo: $repo" "리포 사용: $repo")"

    # Pre-flight: validate required config files
    local required_files=(
      "credentials/.baremetal-init.yaml"
      "credentials/.env"
      "config/sdi-specs.yaml"
      "config/k8s-clusters.yaml"
    )
    for f in "${required_files[@]}"; do
      local check_path="${repo:-.}/$f"
      if [[ ! -f "$check_path" ]]; then
        error_msg "$(i18n "Required file missing: $f" "필수 파일 없음: $f")" \
          "$(i18n "Auto mode requires pre-configured config files" "자동 모드는 사전 구성된 config 파일이 필요합니다")" \
          "$(i18n "Run bash install.sh (interactive mode) to set up first" "bash install.sh (대화형 모드)로 먼저 설정하세요")"
        return 1
      fi
    done
    log_info "$(i18n "Required config files verified" "필수 구성 파일 확인 완료")"

    # Pre-flight: validate tunnel credentials (SSH key + CF Tunnel)
    # Exits with code 2 if any required credential is missing or invalid.
    validate_tunnel_credentials "$repo"

    # Generate ~/.ssh/config for ProxyJump nodes (required by libvirt qemu+ssh://)
    generate_ssh_config "$repo"

    # SSH health check: pre-flight — verify ALL nodes reachable before any provisioning.
    # Must run after generate_ssh_config so ProxyJump entries are in ~/.ssh/config.
    # In --auto mode: first SSH failure aborts install.sh immediately (return 1).
    phase_ssh_check "$(i18n "pre-flight" "사전 확인")" "$repo" || return 1

    # Clone kubespray if not present (not a submodule — runtime dependency)
    if [[ -n "$repo" && ! -f "$repo/kubespray/kubespray/cluster.yml" ]]; then
      log_info "$(i18n "Cloning Kubespray v2.30.0..." "Kubespray v2.30.0 클론 중...")"
      if ! git clone --branch v2.30.0 --depth 1 \
        https://github.com/kubernetes-sigs/kubespray.git \
        "$repo/kubespray/kubespray" 2>&1 | tail -3; then
        error_msg "$(i18n "Kubespray clone failed" "Kubespray 클론 실패")" \
          "$(i18n "git clone error — check network connectivity and GitHub availability" "git clone 오류 — 네트워크 연결 및 GitHub 가용성 확인")" \
          "$(i18n "Retry: git clone --branch v2.30.0 https://github.com/kubernetes-sigs/kubespray.git $repo/kubespray/kubespray" "재시도: git clone --branch v2.30.0 https://github.com/kubernetes-sigs/kubespray.git $repo/kubespray/kubespray")"
        return 1
      fi
    fi

    # Ensure sudo access upfront (single prompt, then cached for entire run)
    # In auto mode, sudo failure is non-fatal if all deps are already installed
    ensure_sudo || {
      log_warn "$(i18n "sudo not available — will proceed if all dependencies are installed" "sudo 사용 불가 — 모든 의존성이 설치되어 있으면 계속 진행")"
    }

    # Run Phase 0 (dependencies) — skip if already complete
    if phase_is_done 0; then
      log_info "$(i18n "Phase 0 already complete — skipping dependency check" \
        "Phase 0 이미 완료 — 의존성 확인 건너뜀")"
      # Ensure PATH is set even when deps phase is skipped
      export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
      [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env" 2>/dev/null || true
    else
      phase_deps
    fi

    REPO_DIR="${repo:-$HOME/ScaleX-POD-mini}"
    state_set REPO_DIR "$REPO_DIR"

    # SSH health check: pre-provision — re-verify nodes after deps phase, before heavy provisioning.
    phase_ssh_check "$(i18n "pre-provision" "프로비저닝 전")" "$REPO_DIR" || return 1

    # Run Phase 4 (build & provision) — skip if already complete
    if phase_is_done 4; then
      log_info "$(i18n "Phase 4 already complete — skipping provisioning" \
        "Phase 4 이미 완료 — 프로비저닝 건너뜀")"
    elif ! phase_provision; then
      error_msg \
        "$(i18n "Auto mode: provisioning failed" "자동 모드: 프로비저닝 실패")" \
        "$(i18n "One or more provisioning steps failed (API tunnel setup, cluster init, etc.)" \
           "하나 이상의 프로비저닝 단계가 실패했습니다 (API 터널 설정, 클러스터 초기화 등)")" \
        "$(i18n "Review the installer log for details: $LOG_FILE" \
           "설치 로그를 확인하세요: $LOG_FILE")"
      return 1
    fi

    # SSH health check: post-install — verify all nodes still reachable after full provisioning.
    # Non-fatal in both modes: installation is already complete; any failure here should be
    # investigated but must not mask a successful install.  Satisfies the
    # feedback_network_safety_critical requirement: verify SSH AFTER every remote operation.
    phase_ssh_check "$(i18n "post-install" "설치 후 확인")" "$REPO_DIR" || \
      log_warn "$(i18n \
        "Post-install SSH check failed — installation succeeded but nodes may be temporarily unreachable. Investigate SSH connectivity." \
        "설치 후 SSH 확인 실패 — 설치는 성공했지만 노드에 일시적으로 접근 불가할 수 있음. SSH 연결을 점검하세요.")"

    show_dashboard
    post_install_summary
    log_raw "=== Installation completed successfully (auto mode) ==="
    return 0
  fi

  # --- Normal interactive flow ---
  resume_check

  local completed; completed=$(state_get_phase)

  if (( completed < 0 )); then
    show_dashboard
    phase_deps
    completed=0
  fi

  # SSH health check: after Phase 0 (deps installed) — verify nodes reachable before
  # bare-metal configuration phase. Skips gracefully if .baremetal-init.yaml is not yet
  # present (nodes are configured during Phase 1). Non-fatal in interactive mode.
  phase_ssh_check "$(i18n "after-deps" "의존성 설치 후")" || true

  if (( completed < 1 )); then
    show_dashboard
    phase_baremetal
    completed=1
  fi

  # SSH health check: after Phase 1 (bare-metal configured) — nodes must be reachable
  # before SDI VM provisioning. Non-fatal in interactive mode; warns and continues.
  phase_ssh_check "$(i18n "after-baremetal" "베어메탈 설정 후")" || true

  if (( completed < 2 )); then
    show_dashboard
    phase_sdi
    completed=2
  fi

  # SSH health check: after Phase 2 (SDI VMs created) — verify nodes still accessible
  # before k8s cluster bootstrap. Non-fatal in interactive mode.
  phase_ssh_check "$(i18n "after-sdi" "SDI 설정 후")" || true

  if (( completed < 3 )); then
    show_dashboard
    phase_cluster
    completed=3
  fi

  # SSH health check: after Phase 3 (cluster config done) — verify nodes reachable
  # before build & provision phase. Non-fatal in interactive mode.
  phase_ssh_check "$(i18n "after-cluster" "클러스터 설정 후")" || true

  if (( completed < 4 )); then
    show_dashboard
    phase_provision
    completed=4
  fi

  # SSH health check: post-install validation — verify all nodes still reachable after
  # full installation. Non-fatal in interactive mode (installation is already complete).
  phase_ssh_check "$(i18n "post-install" "설치 후 확인")" || true

  show_dashboard
  post_install_summary

  log_raw "=== Installation completed successfully ==="
}

main "$@"
