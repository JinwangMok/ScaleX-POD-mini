#!/usr/bin/env bash
# install.sh — ScaleX-POD-mini Interactive TUI Installer
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
readonly GEN_DIR="$INSTALLER_DIR/generated"
readonly LOG_DIR="$INSTALLER_DIR/logs"
readonly LOG_FILE="$LOG_DIR/install-$(date +%Y%m%d-%H%M%S).log"
readonly REPO_URL="${SCALEX_REPO_URL:-https://github.com/ScaleX-project/ScaleX-POD-mini.git}"

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

# ============================================================================
# Section 1: Utility Functions
# ============================================================================

init_dirs() {
  mkdir -p "$INSTALLER_DIR" "$GEN_DIR/credentials" "$GEN_DIR/config" "$LOG_DIR"
}

mask_secrets() {
  sed -E 's/(password|secret|pat|token|PASSWORD|SECRET|PAT|TOKEN)([=:]["'"'"' ]*)[^ "'"'"']*/\1\2****/gi'
}

log_raw() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | mask_secrets >> "$LOG_FILE"; }
log_info() { log_raw "INFO: $*"; echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { log_raw "WARN: $*"; echo -e "${YELLOW}[WARN]${NC} $*"; }
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
  log_info "$(i18n "sudo access required. Please enter your password (one-time)." "sudo 권한이 필요합니다. 비밀번호를 입력해 주세요 (최초 1회).")"
  sudo -v || { log_error "$(i18n "sudo authentication failed" "sudo 인증 실패")"; return 1; }
  # Keep sudo timestamp alive in background
  ( while true; do sudo -n true 2>/dev/null; sleep 50; kill -0 "$$" 2>/dev/null || exit; done ) &
  SUDO_KEEPALIVE_PID=$!
  log_info "$(i18n "sudo credential cache enabled (PID: $SUDO_KEEPALIVE_PID)" "sudo 인증 캐시 활성화 (PID: $SUDO_KEEPALIVE_PID)")"
}

# --- API tunnel management (E2E: kubectl/helm access to cluster APIs) ---
# Sets up SSH port-forward tunnels through a bastion node to reach cluster API servers.
# Only needed when installer can't directly reach VM IPs (e.g., remote via Tailscale).
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

  for kc in "$clusters_dir"/*/kubeconfig.yaml; do
    [[ -f "$kc" ]] || continue
    local cluster_name; cluster_name=$(basename "$(dirname "$kc")")

    # Extract server URL from kubeconfig
    local server_url; server_url=$(grep 'server:' "$kc" | head -1 | awk '{print $2}' | tr -d '"')
    local server_ip; server_ip=$(echo "$server_url" | sed 's|https://||; s|:.*||')
    local server_port; server_port=$(echo "$server_url" | sed 's|.*:||')
    [[ -z "$server_port" ]] && server_port=6443

    # Check direct connectivity first
    if curl -sk --connect-timeout 3 "${server_url}/healthz" &>/dev/null; then
      log_info "$(i18n "${cluster_name}: API directly accessible — tunnel not needed" "${cluster_name}: API 직접 접근 가능 — 터널 불필요")"
      continue
    fi

    # Set up SSH tunnel: localhost:<port> → <vm-ip>:6443 via bastion
    # Use background ssh -N (not -f) for reliable PID capture and set -e compatibility
    log_info "$(i18n "${cluster_name}: SSH tunnel setup (localhost:${local_port} → ${server_ip}:${server_port} via ${bastion_target})" "${cluster_name}: SSH 터널 설정 (localhost:${local_port} → ${server_ip}:${server_port} via ${bastion_target})")"
    ssh -N \
      -o StrictHostKeyChecking=no \
      -o BatchMode=yes \
      -o ExitOnForwardFailure=yes \
      -o ServerAliveInterval=30 \
      -L "${local_port}:${server_ip}:${server_port}" \
      "$bastion_target" 2>/dev/null &
    local tpid=$!
    sleep 1

    # Verify tunnel process is still alive (ExitOnForwardFailure kills it on bind failure)
    if ! kill -0 "$tpid" 2>/dev/null; then
      log_error "$(i18n "${cluster_name}: SSH tunnel setup failed (PID $tpid terminated)" "${cluster_name}: SSH 터널 설정 실패 (PID $tpid 종료됨)")"
      return 1
    fi
    API_TUNNEL_PIDS+=("$tpid")

    # Backup kubeconfig and rewrite server URL to tunnel
    cp "$kc" "${kc}.bak"
    API_TUNNEL_BACKUPS+=("$kc")
    sed -i "s|${server_url}|https://localhost:${local_port}|g" "$kc"
    log_info "${cluster_name}: kubeconfig → localhost:${local_port}"

    local_port=$((local_port + 1))
  done
}

cleanup_api_tunnels() {
  # Kill tunnel processes
  for pid in "${API_TUNNEL_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  if [[ ${#API_TUNNEL_PIDS[@]} -gt 0 ]]; then
    log_info "$(i18n "SSH API tunnel cleanup complete (${#API_TUNNEL_PIDS[@]} tunnels)" "SSH API 터널 ${#API_TUNNEL_PIDS[@]}개 정리 완료")"
  fi
  API_TUNNEL_PIDS=()

  # Restore original kubeconfigs (with real CP IPs, not localhost)
  for kc in "${API_TUNNEL_BACKUPS[@]}"; do
    if [[ -f "${kc}.bak" ]]; then
      mv "${kc}.bak" "$kc"
    fi
  done
  API_TUNNEL_BACKUPS=()
}

# --- SSH config generation ---
# Generates ~/.ssh/config entries from .baremetal-init.yaml for libvirt qemu+ssh:// access.
# Reads node topology and creates ProxyJump entries for non-direct nodes.
generate_ssh_config() {
  local repo_dir="${1:-.}"
  local yaml_file="$repo_dir/credentials/.baremetal-init.yaml"
  local env_file="$repo_dir/credentials/.env"
  local ssh_config="$HOME/.ssh/config"
  local marker="# --- ScaleX-POD-mini managed ---"
  local end_marker="# --- End ScaleX-POD-mini ---"

  [[ -f "$yaml_file" ]] || return 0

  # Skip if already configured
  if [[ -f "$ssh_config" ]] && grep -q "$marker" "$ssh_config"; then
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
  local name="" ip="" reachable_ip="" user="" auth_mode="" via=""
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
        config_block+="    User $user"$'\n'
        [[ "$auth_mode" == "key" ]] && config_block+="    IdentityFile $ssh_key"$'\n'
        [[ -n "$via" ]] && config_block+="    ProxyJump $via"$'\n'
        config_block+="    StrictHostKeyChecking no"$'\n'$'\n'
      fi
      name="${BASH_REMATCH[1]}"
      ip="" reachable_ip="" user="" auth_mode="" via=""
      in_node=true
    elif $in_node; then
      if [[ "$line" =~ reachable_node_ip:[[:space:]]*\"?([^\"]+)\"? ]]; then
        reachable_ip="${BASH_REMATCH[1]}"
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

phase_label() {
  case "$1" in
    0) echo "Dependencies" ;; 1) echo "Bare-metal & SSH" ;;
    2) echo "SDI Virtualization" ;; 3) echo "Cluster & GitOps" ;;
    4) echo "Build & Provision" ;; *) echo "Unknown" ;;
  esac
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
    state_save_phase 0
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
  state_save_phase 0
}

# ============================================================================
# Section 3: Phase 1 — Bare-metal & SSH
# ============================================================================

phase_baremetal() {
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
  state_save_phase 1
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
  state_save_phase 2
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
  state_save_phase 3
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
      if ! tui_yesno "$(i18n "Continue" "계속")" "$(i18n "Build failed. Continue anyway?" "빌드 실패. 그래도 계속하시겠습니까?")"; then return 1; fi
    }
    mkdir -p "$HOME/.local/bin"
    cp "$REPO_DIR/scalex-cli/target/release/scalex" "$HOME/.local/bin/scalex" 2>/dev/null || true
    chmod +x "$HOME/.local/bin/scalex" 2>/dev/null || true
    echo -e "  ${GREEN}OK${NC} — ~/.local/bin/scalex"
  else
    log_warn "$(i18n "scalex-cli directory not found. Skipping." "scalex-cli 디렉토리를 찾을 수 없습니다. 건너뜁니다.")"
  fi

  # Step 4: Validate configs
  echo -e "${CYAN}[Phase 4/4] [Step 4/${total_steps}]${NC} $(i18n "Validating config..." "구성 검증...")"
  if command -v scalex &>/dev/null; then
    (cd "$REPO_DIR" && scalex get config-files 2>&1) || log_warn "$(i18n "Config validation warnings detected" "구성 검증 경고 발생")"
  else
    log_warn "$(i18n "scalex CLI not in PATH. Skipping validation." "scalex CLI가 PATH에 없습니다. 검증을 건너뜁니다.")"
  fi
  echo -e "  ${GREEN}OK${NC}"

  # Step 5: Auto-provisioning
  echo -e "${CYAN}[Phase 4/4] [Step 5/${total_steps}]${NC} $(i18n "Auto-provisioning..." "자동 프로비저닝...")"
  if tui_yesno "$(i18n "Provisioning" "프로비저닝")" "$(i18n "Start auto-provisioning?\n\nThe following tasks will run in order:\n1. facts --all\n2. sdi init\n3. cluster init\n4. API tunnel setup\n5. secrets apply (management + workload)\n6. bootstrap" "자동 프로비저닝을 시작하시겠습니까?\n\n다음 작업이 순서대로 실행됩니다:\n1. facts --all\n2. sdi init\n3. cluster init\n4. API 터널 설정\n5. secrets apply (management + workload)\n6. bootstrap")"; then

    if [[ "$AUTO_MODE" == "true" ]]; then
      # --- Auto mode: explicit step ordering with API tunnel lifecycle ---
      local ps_total=7

      echo -e "  ${CYAN}[1/${ps_total}]${NC} scalex facts --all..."
      if ! (cd "$REPO_DIR" && scalex facts --all 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: facts failed — aborting provisioning" "자동 모드: facts 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[2/${ps_total}]${NC} scalex sdi init..."
      if ! (cd "$REPO_DIR" && scalex sdi init config/sdi-specs.yaml 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: SDI init failed — aborting provisioning" "자동 모드: SDI 초기화 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[3/${ps_total}]${NC} scalex cluster init..."
      if ! (cd "$REPO_DIR" && scalex cluster init config/k8s-clusters.yaml 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: cluster init failed — aborting provisioning" "자동 모드: 클러스터 초기화 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      # Set up API tunnels AFTER cluster init creates kubeconfigs
      echo -e "  ${CYAN}[4/${ps_total}]${NC} $(i18n "API access tunnel setup..." "API 접근 터널 설정...")"
      if ! setup_api_tunnels "$REPO_DIR"; then
        log_error "$(i18n "Auto mode: API tunnel setup failed — aborting provisioning" "자동 모드: API 터널 설정 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[5/${ps_total}]${NC} scalex secrets apply (management)..."
      if ! (cd "$REPO_DIR" && scalex secrets apply --role management 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: management cluster secrets apply failed — aborting provisioning" "자동 모드: 관리 클러스터 시크릿 적용 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[6/${ps_total}]${NC} scalex secrets apply (workload)..."
      if ! (cd "$REPO_DIR" && scalex secrets apply --role workload 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_warn "$(i18n "Workload secrets apply failed — continuing" "워크로드 시크릿 적용 실패 — 계속 진행")"
      fi
      echo -e "  ${GREEN}OK${NC}"

      echo -e "  ${CYAN}[7/${ps_total}]${NC} scalex bootstrap..."
      if ! (cd "$REPO_DIR" && scalex bootstrap 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        log_error "$(i18n "Auto mode: bootstrap failed — aborting provisioning" "자동 모드: 부트스트랩 실패 — 프로비저닝 중단")"
        return 1
      fi
      echo -e "  ${GREEN}OK${NC}"

      # Clean up API tunnels — restore original kubeconfigs with real CP IPs
      cleanup_api_tunnels

    else
      # --- Interactive mode: explicit steps with tunnel support ---
      local prov_pre_tunnel=("scalex facts --all"
                             "scalex sdi init config/sdi-specs.yaml"
                             "scalex cluster init config/k8s-clusters.yaml")
      local prov_post_tunnel=("scalex secrets apply --role management"
                              "scalex secrets apply --role workload"
                              "scalex bootstrap")
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
    find "$REPO_DIR/_generated" -name "admin.conf" -o -name "config" 2>/dev/null | while read -r kc; do
      local cluster_dir; cluster_dir=$(basename "$(dirname "$kc")")
      mkdir -p "$SCALEX_HOME/credentials/${cluster_dir}"
      cp "$kc" "$SCALEX_HOME/credentials/${cluster_dir}/config"
      chmod 600 "$SCALEX_HOME/credentials/${cluster_dir}/config"
      log_info "$(i18n "kubeconfig saved: ${cluster_dir}" "kubeconfig 저장: ${cluster_dir}")"
    done
  fi
  echo -e "  ${GREEN}OK${NC}"

  state_save_phase 4
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
    if (( i <= completed )); then
      icon="\\xE2\\x9C\\x93"; status="Done"  # checkmark
    elif (( i == completed + 1 )); then
      icon="\\xE2\\x96\\xB6"; status="Active"  # play arrow
    else
      icon=" "; status="Pending"
    fi
    lines+="$(printf "  [%b] Phase %d: %-24s %s\n" "$icon" "$i" "$label" "$status")"$'\n'
  done

  echo ""
  echo -e "${BOLD}${BLUE}+------------------------------------------------------+${NC}"
  echo -e "${BOLD}${BLUE}|  ScaleX-POD-mini Installer                    v${VERSION}  |${NC}"
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
        fi
        ;;
      fresh)
        if tui_yesno "$(i18n "Confirm" "확인")" "$(i18n "Reset all state?" "모든 상태를 초기화하시겠습니까?")"; then
          rm -f "$STATE_FILE" "$PHASE_FILE"
          rm -f "$INSTALLER_DIR/nodes.txt" "$INSTALLER_DIR/pools.txt"
          rm -f "$INSTALLER_DIR/clusters.txt" "$INSTALLER_DIR/apps_selected.txt"
          rm -rf "$GEN_DIR"
          mkdir -p "$GEN_DIR/credentials" "$GEN_DIR/config"
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
  echo -e "${BOLD}${GREEN}  $(i18n "ScaleX-POD-mini installation complete!" "ScaleX-POD-mini 설치 완료!")${NC}"
  echo -e "${BOLD}${GREEN}============================================================${NC}"
  echo ""
  echo -e "  ${BOLD}$(i18n "Repository:" "리포지토리:")${NC}  ${repo_dir}"
  echo -e "  ${BOLD}CLI:${NC}         ~/.local/bin/scalex"
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
  echo -e "    scalex get config-files    # $(i18n "Check config" "구성 확인")"
  echo -e "    scalex status              # $(i18n "Cluster status" "클러스터 상태")"
  echo ""
  echo -e "  ${BOLD}$(i18n "ArgoCD dashboard:" "ArgoCD 대시보드:")${NC}"
  local argo_domain; argo_domain=$(state_get "" "")
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
  echo "  ____            _       __  __     ____   ___  ____                   _       _ "
  echo " / ___|  ___ __ _| | ___\ \/ /    |  _ \ / _ \|  _ \   _ __ ___  (_)_ __ (_)"
  echo " \___ \ / __/ _\` | |/ _ \\\\  /_____| |_) | | | | | | | | '_ \` _ \\ | | '_ \\| |"
  echo "  ___) | (_| (_| | |  __//  \\_____|  __/| |_| | |_| | | | | | | || | | | | |"
  echo " |____/ \\___\\__,_|_|\\___/_/\\_\\     |_|    \\___/|____/  |_| |_| |_||_|_| |_|_|"
  echo -e "${NC}"
  echo -e "  ${BOLD}Interactive Installer v${VERSION}${NC}"
  echo ""

  detect_tui
  log_raw "=== ScaleX-POD-mini Installer v${VERSION} started ==="
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
        "$(i18n "No ScaleX-POD-mini repo detected in cwd, SCALEX_REPO_DIR, or \$HOME/ScaleX-POD-mini" "현재 디렉토리, SCALEX_REPO_DIR, \$HOME/ScaleX-POD-mini에서 리포를 찾을 수 없습니다")" \
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

    # Generate ~/.ssh/config for ProxyJump nodes (required by libvirt qemu+ssh://)
    generate_ssh_config "$repo"

    # Clone kubespray if not present (not a submodule — runtime dependency)
    if [[ -n "$repo" && ! -f "$repo/kubespray/kubespray/cluster.yml" ]]; then
      log_info "$(i18n "Cloning Kubespray v2.30.0..." "Kubespray v2.30.0 클론 중...")"
      git clone --branch v2.30.0 --depth 1 \
        https://github.com/kubernetes-sigs/kubespray.git \
        "$repo/kubespray/kubespray" 2>&1 | tail -3
    fi

    # Ensure sudo access upfront (single prompt, then cached for entire run)
    ensure_sudo

    # Run Phase 0 (dependencies) + Phase 4 (build & provision)
    phase_deps
    REPO_DIR="${repo:-$HOME/ScaleX-POD-mini}"
    state_set REPO_DIR "$REPO_DIR"
    phase_provision

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

  if (( completed < 1 )); then
    show_dashboard
    phase_baremetal
    completed=1
  fi

  if (( completed < 2 )); then
    show_dashboard
    phase_sdi
    completed=2
  fi

  if (( completed < 3 )); then
    show_dashboard
    phase_cluster
    completed=3
  fi

  if (( completed < 4 )); then
    show_dashboard
    phase_provision
    completed=4
  fi

  show_dashboard
  post_install_summary

  log_raw "=== Installation completed successfully ==="
}

main "$@"
