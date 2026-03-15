#!/usr/bin/env bash
# install.sh — ScaleX-POD-mini Interactive TUI Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh | bash
# Or:    wget -qO- https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh | bash
# Or:    bash install.sh
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
readonly REPO_URL="https://github.com/JinwangMok/ScaleX-POD-mini.git"

readonly KUBECTL_VERSION="v1.33.1"
readonly HELM_VERSION="v3.17.3"
readonly OPENTOFU_VERSION="1.9.0"
readonly ARGOCD_VERSION="v2.14.0"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

TUI=""
REPO_DIR=""
NODE_COUNT=0
POOL_COUNT=0
CLUSTER_COUNT=0
AUTO_MODE="${AUTO_MODE:-false}"

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
  if [[ $exit_code -ne 0 ]]; then
    echo ""
    log_warn "인스톨러가 중단되었습니다. 현재 상태가 저장되었습니다."
    log_warn "다시 실행하면 이어서 진행할 수 있습니다: bash install.sh"
  fi
}
trap cleanup_handler EXIT
trap 'echo ""; log_warn "Ctrl+C 감지. 상태를 저장합니다..."; exit 130' INT

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
    fallback) echo -e "\n${BOLD}[$title]${NC}\n$msg\n"; read -rp "Enter를 눌러 계속..." ;;
  esac
}

tui_input() {
  local title="$1" prompt="$2" default="${3:-}"
  case "$TUI" in
    whiptail) whiptail --title "$title" --inputbox "$prompt" 10 72 "$default" 3>&1 1>&2 2>&3 ;;
    dialog)   dialog --title "$title" --inputbox "$prompt" 10 72 "$default" 3>&1 1>&2 2>&3 ;;
    fallback)
      echo -e "${BOLD}$title${NC}: $prompt"
      [[ -n "$default" ]] && echo -e "  (기본값: $default)"
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
      local val; read -rp "선택> " val; echo "$val"
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
      echo "활성화할 항목을 공백으로 구분 입력 (예: cert-manager kyverno):"
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
    log_info "SSH config 이미 설정됨 — 건너뜀"
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
  log_info "SSH config 생성 완료 (~/.ssh/config)"
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
  log_info "$name 설치 중..."
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
        macos) brew install hudochenkov/sshpass/sshpass 2>/dev/null || log_warn "sshpass는 macOS에서 별도 설치 필요" ;;
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
  log_phase "Phase 0: 의존성 확인"
  local os; os=$(detect_os)
  log_info "운영체제: $os"
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
    log_warn "선택적 도구 누락 (password SSH 인증 시 필요): ${opt_missing[*]}"
  fi

  if [[ ${#missing[@]} -eq 0 ]]; then
    log_info "모든 필수 의존성이 설치되어 있습니다."
    state_save_phase 0
    return 0
  fi

  log_warn "누락된 도구: ${missing[*]}"
  if tui_yesno "의존성 설치" "다음 도구를 설치하시겠습니까?\n\n${missing[*]}"; then
    for name in "${missing[@]}"; do
      install_dep "$name" "$os" || {
        error_msg "$name 설치 실패" "패키지 매니저 오류 또는 네트워크 문제" "수동 설치 후 다시 실행하세요"
        return 1
      }
    done
    # Install optional deps (best-effort, don't fail)
    for name in "${opt_missing[@]}"; do
      install_dep "$name" "$os" || log_warn "$name 설치 실패 — password SSH 미사용 시 무시 가능"
    done
    # Re-source cargo env if rust was installed
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env" 2>/dev/null || true
    log_info "의존성 설치 완료"
  else
    log_warn "의존성을 건너뜁니다. 일부 기능이 작동하지 않을 수 있습니다."
  fi
  state_save_phase 0
}

# ============================================================================
# Section 3: Phase 1 — Bare-metal & SSH
# ============================================================================

phase_baremetal() {
  log_phase "Phase 1: 베어메탈 노드 & SSH 설정"

  # Network defaults
  local bridge cidr gateway
  bridge=$(tui_input "네트워크 기본값" "관리 브릿지 인터페이스:" "$(state_get NET_BRIDGE br0)")
  cidr=$(tui_input "네트워크 기본값" "관리 네트워크 CIDR:" "$(state_get NET_CIDR 192.168.88.0/24)")
  while ! validate_cidr "$cidr"; do
    log_error "유효하지 않은 CIDR: $cidr"
    cidr=$(tui_input "네트워크 기본값" "관리 네트워크 CIDR (예: 192.168.88.0/24):" "192.168.88.0/24")
  done
  gateway=$(tui_input "네트워크 기본값" "게이트웨이 IP:" "$(state_get NET_GW 192.168.88.1)")
  while ! validate_ip "$gateway"; do
    log_error "유효하지 않은 IP: $gateway"
    gateway=$(tui_input "네트워크 기본값" "게이트웨이 IP:" "192.168.88.1")
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
    log_info "노드 #${NODE_COUNT} 설정"

    local name; name=$(tui_input "노드 #${NODE_COUNT}" "노드 이름 (예: playbox-0):" "")
    while ! validate_not_empty "$name"; do
      name=$(tui_input "노드 #${NODE_COUNT}" "노드 이름 (비어있을 수 없음):" "")
    done

    local access; access=$(tui_menu "노드 접근 방식" "SSH 접근 방식 선택:" \
      "direct"  "직접 접근 (같은 LAN)" \
      "external" "외부 IP (예: Tailscale)" \
      "proxy"   "ProxyJump (다른 노드 경유)")

    local node_ip; node_ip=$(tui_input "노드 IP" "노드 LAN IP:" "")
    while ! validate_ip "$node_ip"; do
      log_error "유효하지 않은 IP"
      node_ip=$(tui_input "노드 IP" "노드 LAN IP (예: 192.168.88.8):" "")
    done

    local admin_user; admin_user=$(tui_input "SSH 사용자" "SSH 사용자:" "jinwang")

    local auth_mode; auth_mode=$(tui_menu "인증 방식" "SSH 인증 방식:" \
      "password" "비밀번호" \
      "key"      "SSH 키")

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
        local ext_ip; ext_ip=$(tui_input "외부 IP" "외부 접근 IP (예: Tailscale IP):" "")
        while ! validate_ip "$ext_ip"; do
          ext_ip=$(tui_input "외부 IP" "유효한 IP를 입력하세요:" "")
        done
        node_yaml+="    direct_reachable: false\n"
        node_yaml+="    reachable_node_ip: \"${ext_ip}\"\n"
        node_yaml+="    node_ip: \"${node_ip}\"\n"
        ;;
      proxy)
        local proxy_node; proxy_node=$(tui_input "ProxyJump" "경유할 노드 이름:" "")
        node_yaml+="    direct_reachable: false\n"
        node_yaml+="    reachable_via: [\"${proxy_node}\"]\n"
        node_yaml+="    node_ip: \"${node_ip}\"\n"
        ;;
    esac

    node_yaml+="    adminUser: \"${admin_user}\"\n"
    node_yaml+="    sshAuthMode: \"${auth_mode}\"\n"

    if [[ "$auth_mode" == "password" ]]; then
      local pw; pw=$(tui_password "SSH 비밀번호" "${name} 의 SSH 비밀번호:")
      env_lines+="${var_upper}_PASSWORD=\"${pw}\"\n"
      node_yaml+="    sshPassword: \"${var_upper}_PASSWORD\"\n"
    else
      local kp; kp=$(tui_input "SSH 키 경로" "SSH 키 경로:" "~/.ssh/id_ed25519")
      env_lines+="SSH_KEY_PATH=\"${kp}\"\n"
      node_yaml+="    sshKeyPath: \"SSH_KEY_PATH\"\n"
    fi

    yaml_nodes+="${node_yaml}\n"
    echo "${name}|${access}|${node_ip}|${admin_user}|${auth_mode}" >> "$INSTALLER_DIR/nodes.txt"

    if ! tui_yesno "노드 추가" "다른 노드를 추가하시겠습니까?"; then
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
  log_info "노드 ${NODE_COUNT}개 구성 완료"

  # SSH test option
  if tui_yesno "SSH 테스트" "구성된 노드에 SSH 연결을 테스트하시겠습니까?"; then
    while IFS='|' read -r n _ ip user auth; do
      echo -n "  ${n} (${ip})... "
      if ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no -o BatchMode=yes "${user}@${ip}" "echo ok" 2>/dev/null; then
        echo -e "${GREEN}OK${NC}"
      else
        echo -e "${YELLOW}FAIL (수동 확인 필요)${NC}"
      fi
    done < "$INSTALLER_DIR/nodes.txt"
  fi

  state_set NODE_COUNT "$NODE_COUNT"
  state_save_phase 1
  log_info "Phase 1 완료"
}

# ============================================================================
# Section 4: Phase 2 — SDI Virtualization
# ============================================================================

collect_vm_specs() {
  local pool_name="$1" purpose="$2" pool_idx="$3"
  local vm_yaml="" vm_count=0 adding_vm=true

  # Placement
  local place_mode; place_mode=$(tui_menu "배치 방식 ($pool_name)" "VM 배치 방식:" \
    "hosts" "특정 호스트 지정" \
    "spread" "호스트 전체에 분산")

  local place_yaml=""
  if [[ "$place_mode" == "spread" ]]; then
    place_yaml="      placement:\n        spread: true"
  else
    local hosts_str; hosts_str=$(tui_input "호스트 지정" "호스트 이름 (쉼표 구분):" "")
    local hosts_list; hosts_list=$(echo "$hosts_str" | sed 's/,/, /g; s/^/[/; s/$/]/')
    place_yaml="      placement:\n        hosts: ${hosts_list}"
  fi

  while $adding_vm; do
    vm_count=$((vm_count + 1))
    log_info "  VM #${vm_count} (${pool_name})"

    local vm_name; vm_name=$(tui_input "VM 이름" "VM 이름:" "${pool_name}-cp-$((vm_count-1))")
    local vm_ip; vm_ip=$(tui_input "VM IP" "VM IP:" "")
    while ! validate_ip "$vm_ip"; do
      vm_ip=$(tui_input "VM IP" "유효한 IP를 입력하세요:" "")
    done
    local vm_cpu; vm_cpu=$(tui_input "CPU" "CPU 코어 수:" "4")
    local vm_mem; vm_mem=$(tui_input "메모리" "메모리 (GB):" "8")
    local vm_disk; vm_disk=$(tui_input "디스크" "디스크 (GB):" "60")

    local vm_host=""
    if [[ "$place_mode" == "spread" ]]; then
      vm_host=$(tui_input "호스트" "이 VM의 호스트 (비워두면 자동):" "")
    fi

    local roles_str; roles_str=$(tui_checklist "역할 선택" "VM 역할 선택:" \
      "control-plane" "컨트롤 플레인" "OFF" \
      "etcd"          "etcd"           "OFF" \
      "worker"        "워커 노드"      "ON")
    # Normalize checklist output
    roles_str=$(echo "$roles_str" | tr -d '"' | tr ' ' ', ')
    [[ -z "$roles_str" ]] && roles_str="worker"
    local roles_yaml; roles_yaml=$(echo "$roles_str" | sed 's/,/, /g; s/^/[/; s/$/]/')

    local gpu_line=""
    if tui_yesno "GPU" "GPU 패스스루를 활성화하시겠습니까?"; then
      gpu_line="\n          devices:\n            gpu_passthrough: true"
    fi

    vm_yaml+="        - node_name: \"${vm_name}\"\n"
    vm_yaml+="          ip: \"${vm_ip}\"\n"
    vm_yaml+="          cpu: ${vm_cpu}\n"
    vm_yaml+="          mem_gb: ${vm_mem}\n"
    vm_yaml+="          disk_gb: ${vm_disk}\n"
    [[ -n "$vm_host" ]] && vm_yaml+="          host: \"${vm_host}\"\n"
    vm_yaml+="          roles: ${roles_yaml}${gpu_line}\n"

    if ! tui_yesno "VM 추가" "이 풀에 VM을 더 추가하시겠습니까?"; then
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
  log_phase "Phase 2: SDI 가상화 설정"

  local bridge; bridge=$(state_get NET_BRIDGE "br0")
  local cidr; cidr=$(state_get NET_CIDR "192.168.88.0/24")
  local gateway; gateway=$(state_get NET_GW "192.168.88.1")

  # Resource pool
  local pool_rp_name; pool_rp_name=$(tui_input "리소스 풀" "리소스 풀 이름:" "playbox-pool")
  local dns_str; dns_str=$(tui_input "DNS" "DNS 서버 (쉼표 구분):" "8.8.8.8,8.8.4.4")
  local dns_yaml; dns_yaml=$(echo "$dns_str" | sed 's/ *//g; s/,/", "/g; s/^/["/; s/$/"]/')

  # OS image
  local os_url; os_url=$(tui_input "OS 이미지" "클라우드 이미지 URL:" \
    "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img")
  local os_fmt; os_fmt=$(tui_input "이미지 포맷" "이미지 포맷:" "qcow2")

  # Cloud init
  local ssh_pubkey; ssh_pubkey=$(tui_input "SSH 공개키" "SSH 공개키 경로:" "~/.ssh/id_ed25519.pub")
  local pkg_str; pkg_str=$(tui_input "패키지" "설치할 패키지 (쉼표 구분):" \
    "curl,apt-transport-https,nfs-common,open-iscsi")
  local pkg_yaml; pkg_yaml=$(echo "$pkg_str" | sed 's/ *//g; s/,/, /g; s/^/[/; s/$/]/')

  # Tower pool (required)
  log_info "Tower 풀 설정 (필수 — 관리 클러스터용)"
  local tower_pool_yaml; tower_pool_yaml=$(collect_vm_specs "tower" "management" 0)

  # Sandbox pool (required)
  log_info "워크로드 풀 설정 (필수 — 첫 번째 워크로드 클러스터용)"
  local sandbox_name; sandbox_name=$(tui_input "워크로드 풀" "워크로드 풀 이름:" "sandbox")
  local sandbox_pool_yaml; sandbox_pool_yaml=$(collect_vm_specs "$sandbox_name" "workload" 1)

  local extra_pools_yaml=""
  POOL_COUNT=2

  while tui_yesno "추가 풀" "추가 SDI 풀을 만드시겠습니까?"; do
    POOL_COUNT=$((POOL_COUNT + 1))
    local ep_name; ep_name=$(tui_input "풀 이름" "풀 이름:" "pool-${POOL_COUNT}")
    local ep_purpose; ep_purpose=$(tui_menu "용도" "풀 용도:" \
      "workload" "워크로드" "storage" "스토리지" "monitoring" "모니터링")
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
  log_info "Phase 2 완료 — ${POOL_COUNT}개 풀 구성됨"
}

# ============================================================================
# Section 5: Phase 3 — Cluster & GitOps
# ============================================================================

collect_cluster() {
  local cname="$1" crole="$2" cid="$3" pool_name="${4:-}"
  local cluster_yaml=""

  if [[ -z "$pool_name" ]]; then
    pool_name=$(tui_input "SDI 풀" "${cname} 클러스터의 SDI 풀 이름:" "$cname")
  fi

  local mode; mode=$(tui_menu "클러스터 모드" "클러스터 모드:" \
    "sdi" "SDI VM 풀 사용" "baremetal" "베어메탈 직접 사용")

  local ssh_user; ssh_user=$(tui_input "SSH 사용자" "클러스터 SSH 사용자:" "jinwang")

  # Network
  local pod_cidr service_cidr dns_domain
  if [[ "$crole" == "management" ]]; then
    pod_cidr=$(tui_input "네트워크" "Pod CIDR:" "10.244.0.0/20")
    service_cidr=$(tui_input "네트워크" "Service CIDR:" "10.96.0.0/20")
    dns_domain=$(tui_input "네트워크" "DNS 도메인:" "tower.local")
  else
    pod_cidr=$(tui_input "네트워크" "Pod CIDR:" "10.233.0.0/17")
    service_cidr=$(tui_input "네트워크" "Service CIDR:" "10.233.128.0/18")
    dns_domain=$(tui_input "네트워크" "DNS 도메인:" "${cname}.local")
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
    local nrc; nrc=$(tui_input "네트워크" "Native routing CIDR (비워두면 생략):" "")
    [[ -n "$nrc" ]] && cluster_yaml+="        native_routing_cidr: \"${nrc}\"\n"
  fi

  cluster_yaml+="      cilium:\n"
  cluster_yaml+="        cluster_id: ${cid}\n"
  cluster_yaml+="        cluster_name: \"${cname}\"\n"

  # OIDC (workload only)
  if [[ "$crole" == "workload" ]]; then
    if tui_yesno "OIDC" "OIDC 인증을 활성화하시겠습니까?"; then
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
  log_phase "Phase 3: 클러스터 & GitOps 설정"

  # Common K8s settings
  local k8s_ver; k8s_ver=$(tui_input "Kubernetes" "Kubernetes 버전:" "1.33.1")
  local ks_ver; ks_ver=$(tui_input "Kubespray" "Kubespray 버전:" "v2.30.0")
  local cilium_ver; cilium_ver=$(tui_input "Cilium" "Cilium 버전:" "1.17.5")

  local advanced_common=""
  local kube_proxy_remove="true" cgroup_driver="systemd" helm_enabled="true"
  local gw_api_enabled="true" gw_api_ver="1.3.0" nodelocaldns="true"
  local node_prefix="24" ntp="true" etcd_type="host" dns_mode="coredns"
  local graceful_shutdown="true" graceful_sec="120"

  if tui_yesno "고급 설정" "공통 Kubernetes 고급 설정을 변경하시겠습니까?"; then
    kube_proxy_remove=$(tui_menu "kube-proxy" "kube-proxy 제거 (Cilium 대체):" "true" "예" "false" "아니오")
    cgroup_driver=$(tui_menu "cgroup" "cgroup 드라이버:" "systemd" "systemd (권장)" "cgroupfs" "cgroupfs")
    gw_api_ver=$(tui_input "Gateway API" "Gateway API 버전:" "$gw_api_ver")
    node_prefix=$(tui_input "노드 프리픽스" "Pod 네트워크 노드 프리픽스 (/N):" "$node_prefix")
    etcd_type=$(tui_menu "etcd" "etcd 배포 방식:" "host" "Host (권장)" "kubeadm" "Kubeadm")
  fi

  # Tower cluster (required)
  log_info "Tower 클러스터 설정 (필수 — 관리 클러스터)"
  local tower_yaml; tower_yaml=$(collect_cluster "tower" "management" 1 "tower")

  # Sandbox cluster (required)
  local sandbox_pool; sandbox_pool=$(state_get SANDBOX_POOL_NAME "sandbox")
  log_info "워크로드 클러스터 설정 (필수)"
  local sandbox_name; sandbox_name=$(tui_input "클러스터" "워크로드 클러스터 이름:" "sandbox")
  local sandbox_yaml; sandbox_yaml=$(collect_cluster "$sandbox_name" "workload" 2 "$sandbox_pool")

  local extra_clusters_yaml=""
  CLUSTER_COUNT=2
  local managed_list="\"${sandbox_name}\""

  while tui_yesno "추가 클러스터" "클러스터를 더 추가하시겠습니까?"; do
    CLUSTER_COUNT=$((CLUSTER_COUNT + 1))
    local ec_name; ec_name=$(tui_input "클러스터 이름" "클러스터 이름:" "cluster-${CLUSTER_COUNT}")
    local ec_yaml; ec_yaml=$(collect_cluster "$ec_name" "workload" "$CLUSTER_COUNT" "")
    extra_clusters_yaml+="\n${ec_yaml}"
    managed_list+=", \"${ec_name}\""
  done

  # ArgoCD settings
  local argo_ns; argo_ns=$(tui_input "ArgoCD" "네임스페이스:" "argocd")
  local argo_repo; argo_repo=$(tui_input "ArgoCD" "Git 리포 URL:" "$REPO_URL")
  local argo_branch; argo_branch=$(tui_input "ArgoCD" "브랜치:" "main")

  # Domains
  local dom_auth; dom_auth=$(tui_input "도메인" "Auth 도메인 (Keycloak):" "auth.example.com")
  local dom_argo; dom_argo=$(tui_input "도메인" "ArgoCD 도메인:" "cd.example.com")
  local dom_api; dom_api=$(tui_input "도메인" "K8s API 도메인:" "api.k8s.example.com")

  # Secrets
  log_info "시크릿 설정"
  local kc_admin_pw; kc_admin_pw=$(tui_password "Keycloak" "Keycloak Admin 비밀번호:")
  local kc_db_pw; kc_db_pw=$(tui_password "Keycloak" "Keycloak DB 비밀번호:")
  local argo_pat; argo_pat=$(tui_password "ArgoCD" "GitHub PAT (비공개 리포가 아니면 비워두세요):")

  # Cloudflare
  local cf_enabled=false cf_account="" cf_secret="" cf_tunnel_id=""
  if tui_yesno "Cloudflare Tunnel" "Cloudflare Tunnel을 사용하시겠습니까?"; then
    cf_enabled=true
    cf_account=$(tui_input "Cloudflare" "Account Tag:" "")
    cf_secret=$(tui_password "Cloudflare" "Tunnel Secret:")
    cf_tunnel_id=$(tui_input "Cloudflare" "Tunnel ID:" "")
  fi

  # App selection
  log_info "앱 선택"
  local common_apps; common_apps=$(tui_checklist "공통 앱" "모든 클러스터에 설치할 앱:" \
    "cilium-resources"  "Cilium 리소스 (manifest)" "ON" \
    "cert-manager"      "cert-manager v1.18.2"     "ON" \
    "kyverno"           "Kyverno 3.3.7"            "ON" \
    "kyverno-policies"  "Kyverno 정책 (manifest)"   "ON")
  echo "$common_apps" > "$INSTALLER_DIR/apps_selected.txt"

  local tower_apps; tower_apps=$(tui_checklist "Tower 앱" "Tower 클러스터 앱:" \
    "cilium"            "Cilium ${cilium_ver}"       "ON" \
    "argocd"            "ArgoCD 8.1.1"               "ON" \
    "cluster-config"    "클러스터 설정 (manifest)"    "ON" \
    "cert-issuers"      "인증서 발급자 (manifest)"    "ON" \
    "keycloak"          "Keycloak 25.1.2"            "ON" \
    "cloudflared-tunnel" "Cloudflare Tunnel 2.1.2"   "$(${cf_enabled} && echo ON || echo OFF)" \
)
  echo "$tower_apps" >> "$INSTALLER_DIR/apps_selected.txt"

  local sandbox_apps; sandbox_apps=$(tui_checklist "Sandbox 앱" "${sandbox_name} 클러스터 앱:" \
    "cilium"                  "Cilium ${cilium_ver}"                "ON" \
    "cluster-config"          "클러스터 설정 (manifest)"             "ON" \
    "local-path-provisioner"  "Local Path Provisioner v0.0.32"     "ON" \
    "rbac"                    "RBAC (manifest)"                    "ON" \
    "test-resources"          "테스트 리소스 (manifest)"             "OFF")
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
  log_info "Phase 3 완료 — ${CLUSTER_COUNT}개 클러스터 구성됨"
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
    if tui_yesno "재시도" "${desc} 실패. 재시도하시겠습니까?"; then
      "$@"
    else
      return 1
    fi
  fi
}

phase_provision() {
  log_phase "Phase 4: 빌드 & 프로비저닝"
  local total_steps=6
  local repo_url; repo_url=$(state_get REPO_URL_USER "$REPO_URL")

  # Step 1: Clone or locate repo
  echo -e "${CYAN}[Phase 4/4] [Step 1/${total_steps}]${NC} 리포지토리 준비..."
  if [[ -n "${SCALEX_REPO_DIR:-}" && -d "$SCALEX_REPO_DIR/.git" ]]; then
    REPO_DIR="$SCALEX_REPO_DIR"
    log_info "기존 리포 사용: $REPO_DIR"
  elif [[ -d "$HOME/local-workspace/ScaleX-POD-mini/.git" ]]; then
    REPO_DIR="$HOME/local-workspace/ScaleX-POD-mini"
    log_info "로컬 리포 발견: $REPO_DIR"
    if tui_yesno "리포지토리" "기존 리포를 사용하시겠습니까?\n${REPO_DIR}"; then
      log_info "기존 리포 사용"
    else
      REPO_DIR="$HOME/ScaleX-POD-mini"
      git clone "$repo_url" "$REPO_DIR" 2>&1 | tail -1
    fi
  else
    REPO_DIR="$HOME/ScaleX-POD-mini"
    log_info "리포 클론 중: $repo_url"
    git clone "$repo_url" "$REPO_DIR" 2>&1 | tail -1
  fi
  echo -e "  ${GREEN}OK${NC} — $REPO_DIR"
  state_set REPO_DIR "$REPO_DIR"

  # Step 2: Copy generated config files (skip in auto mode — repo already has correct config)
  echo -e "${CYAN}[Phase 4/4] [Step 2/${total_steps}]${NC} 구성 파일 복사..."
  if [[ "$AUTO_MODE" == "true" ]]; then
    log_info "자동 모드: 기존 구성 파일 유지 (덮어쓰기 건너뜀)"
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
  echo -e "${CYAN}[Phase 4/4] [Step 3/${total_steps}]${NC} scalex CLI 빌드..."
  if [[ -d "$REPO_DIR/scalex-cli" ]]; then
    (cd "$REPO_DIR/scalex-cli" && cargo build --release 2>&1 | tail -3) || {
      error_msg "scalex CLI 빌드 실패" "Rust 컴파일 오류" "cargo build 로그를 확인하세요"
      if ! tui_yesno "계속" "빌드 실패. 그래도 계속하시겠습니까?"; then return 1; fi
    }
    mkdir -p "$HOME/.local/bin"
    cp "$REPO_DIR/scalex-cli/target/release/scalex" "$HOME/.local/bin/scalex" 2>/dev/null || true
    chmod +x "$HOME/.local/bin/scalex" 2>/dev/null || true
    echo -e "  ${GREEN}OK${NC} — ~/.local/bin/scalex"
  else
    log_warn "scalex-cli 디렉토리를 찾을 수 없습니다. 건너뜁니다."
  fi

  # Step 4: Validate configs
  echo -e "${CYAN}[Phase 4/4] [Step 4/${total_steps}]${NC} 구성 검증..."
  if command -v scalex &>/dev/null; then
    (cd "$REPO_DIR" && scalex get config-files 2>&1) || log_warn "구성 검증 경고 발생"
  else
    log_warn "scalex CLI가 PATH에 없습니다. 검증을 건너뜁니다."
  fi
  echo -e "  ${GREEN}OK${NC}"

  # Step 5: Auto-provisioning
  echo -e "${CYAN}[Phase 4/4] [Step 5/${total_steps}]${NC} 자동 프로비저닝..."
  if tui_yesno "프로비저닝" "자동 프로비저닝을 시작하시겠습니까?\n\n다음 작업이 순서대로 실행됩니다:\n1. facts --all\n2. sdi init\n3. cluster init\n4. secrets apply\n5. bootstrap"; then
    local prov_steps=("scalex facts --all"
                      "scalex sdi init config/sdi-specs.yaml"
                      "scalex cluster init config/k8s-clusters.yaml"
                      "scalex secrets apply"
                      "scalex bootstrap")
    local ps_i=0 ps_total=${#prov_steps[@]}
    for cmd in "${prov_steps[@]}"; do
      ps_i=$((ps_i + 1))
      echo -e "  ${CYAN}[${ps_i}/${ps_total}]${NC} ${cmd}..."
      if (cd "$REPO_DIR" && eval "$cmd" 2>&1 | tee -a "$LOG_FILE" | tail -5); then
        echo -e "  ${GREEN}OK${NC}"
      else
        log_error "프로비저닝 실패: $cmd"
        # Auto mode: stop on first failure (steps are sequential dependencies)
        if [[ "$AUTO_MODE" == "true" ]]; then
          log_error "자동 모드: 프로비저닝 중단 (이전 단계 실패)"
          return 1
        fi
        if ! tui_yesno "계속" "${cmd} 실패. 다음 단계로 계속하시겠습니까?"; then
          break
        fi
      fi
    done
  else
    log_info "프로비저닝을 건너뜁니다. 수동으로 실행하세요."
  fi

  # Step 6: Save kubeconfigs
  echo -e "${CYAN}[Phase 4/4] [Step 6/${total_steps}]${NC} kubeconfig 저장..."
  mkdir -p "$SCALEX_HOME/credentials"
  if [[ -d "$REPO_DIR/_generated" ]]; then
    find "$REPO_DIR/_generated" -name "admin.conf" -o -name "config" 2>/dev/null | while read -r kc; do
      local cluster_dir; cluster_dir=$(basename "$(dirname "$kc")")
      mkdir -p "$SCALEX_HOME/credentials/${cluster_dir}"
      cp "$kc" "$SCALEX_HOME/credentials/${cluster_dir}/config"
      chmod 600 "$SCALEX_HOME/credentials/${cluster_dir}/config"
      log_info "kubeconfig 저장: ${cluster_dir}"
    done
  fi
  echo -e "  ${GREEN}OK${NC}"

  state_save_phase 4
  log_info "Phase 4 완료"
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
    log_info "이전 설치 상태가 발견되었습니다 (Phase ${completed} 완료)"
    show_dashboard
    local choice; choice=$(tui_menu "재개" "어떻게 진행하시겠습니까?" \
      "continue" "이어서 진행" \
      "reset"    "특정 Phase부터 재시작" \
      "fresh"    "처음부터 시작")
    case "$choice" in
      continue) return 0 ;;
      reset)
        local phase; phase=$(tui_input "Phase 선택" "재시작할 Phase 번호 (0-4):" "")
        if [[ "$phase" =~ ^[0-4]$ ]]; then
          local prev=$((phase - 1))
          (( prev < 0 )) && prev=-1
          state_save_phase "$prev"
        fi
        ;;
      fresh)
        if tui_yesno "확인" "모든 상태를 초기화하시겠습니까?"; then
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
  echo -e "${BOLD}${GREEN}  ScaleX-POD-mini 설치 완료!${NC}"
  echo -e "${BOLD}${GREEN}============================================================${NC}"
  echo ""
  echo -e "  ${BOLD}리포지토리:${NC}  ${repo_dir}"
  echo -e "  ${BOLD}CLI:${NC}         ~/.local/bin/scalex"
  echo -e "  ${BOLD}상태:${NC}        ${INSTALLER_DIR}"
  echo -e "  ${BOLD}로그:${NC}        ${LOG_FILE}"
  echo ""

  if [[ -d "$SCALEX_HOME/credentials" ]]; then
    echo -e "  ${BOLD}Kubeconfigs:${NC}"
    find "$SCALEX_HOME/credentials" -name "config" 2>/dev/null | while read -r kc; do
      local cname; cname=$(basename "$(dirname "$kc")")
      echo -e "    - ${cname}: ${kc}"
    done
    echo ""
  fi

  echo -e "  ${BOLD}다음 단계:${NC}"
  echo -e "    export PATH=\"\$HOME/.local/bin:\$PATH\""
  echo -e "    cd ${repo_dir}"
  echo -e "    scalex get config-files    # 구성 확인"
  echo -e "    scalex status              # 클러스터 상태"
  echo ""
  echo -e "  ${BOLD}ArgoCD 대시보드:${NC}"
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
    log_info "자동 모드 활성화 — Phases 1-3 건너뜀"

    # Locate repo: env var > well-known path > current dir
    local repo="${SCALEX_REPO_DIR:-}"
    if [[ -z "$repo" ]]; then
      if [[ -d "$HOME/local-workspace/ScaleX-POD-mini/.git" ]]; then
        repo="$HOME/local-workspace/ScaleX-POD-mini"
      elif [[ -d "$(pwd)/.git" && -f "$(pwd)/install.sh" ]]; then
        repo="$(pwd)"
      fi
    fi

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
        error_msg "필수 파일 없음: $f" \
          "자동 모드는 사전 구성된 config 파일이 필요합니다" \
          "bash install.sh (대화형 모드)로 먼저 설정하세요"
        return 1
      fi
    done
    log_info "필수 구성 파일 확인 완료"

    # Generate ~/.ssh/config for ProxyJump nodes (required by libvirt qemu+ssh://)
    generate_ssh_config "$repo"

    # Clone kubespray if not present (not a submodule — runtime dependency)
    if [[ -n "$repo" && ! -f "$repo/kubespray/kubespray/cluster.yml" ]]; then
      log_info "Kubespray v2.30.0 클론 중..."
      git clone --branch v2.30.0 --depth 1 \
        https://github.com/kubernetes-sigs/kubespray.git \
        "$repo/kubespray/kubespray" 2>&1 | tail -3
    fi

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
