#!/usr/bin/env bash
# lib/common.sh — Shared utilities for playbox CLI

# Colors
readonly RED='\033[0;31m'
readonly YELLOW='\033[0;33m'
readonly GREEN='\033[0;32m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m'

# Configurable commands (for testing)
YQ="${PLAYBOX_YQ:-yq}"
SSH="${PLAYBOX_SSH_CMD:-ssh}"
KUBECTL="${PLAYBOX_KUBECTL:-kubectl}"
HELM="${PLAYBOX_HELM:-helm}"
CURL="${PLAYBOX_CURL:-curl}"
TOFU="${PLAYBOX_TOFU:-tofu}"
ANSIBLE_PLAYBOOK="${PLAYBOX_ANSIBLE:-ansible-playbook}"

# Script root directory
PLAYBOX_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VALUES_FILE="${PLAYBOX_VALUES:-${PLAYBOX_ROOT}/values.yaml}"

# Dry-run mode
DRY_RUN="${DRY_RUN:-false}"

# --- Logging ---
log_info() {
    echo -e "${GREEN}[INFO]${NC} $(date +'%H:%M:%S') $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $(date +'%H:%M:%S') $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $(date +'%H:%M:%S') $1" >&2
}

log_step() {
    echo -e "\n${BLUE}==>${NC} $1"
}

# --- yq wrappers ---
yq_read() {
    local path="$1"
    ${YQ} eval "${path}" "${VALUES_FILE}"
}

yq_read_default() {
    local path="$1"
    local default="$2"
    local result
    result=$(${YQ} eval "${path} // \"${default}\"" "${VALUES_FILE}")
    echo "${result}"
}

# --- Idempotency helpers ---
ensure_namespace() {
    local ns="$1"
    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would create namespace: ${ns}"
        return 0
    fi
    ${KUBECTL} get ns "${ns}" &>/dev/null || ${KUBECTL} create ns "${ns}"
}

helm_install() {
    local release="$1"
    local chart="$2"
    local namespace="$3"
    shift 3
    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would helm install: ${release} from ${chart} in ${namespace}"
        return 0
    fi
    ${HELM} upgrade --install "${release}" "${chart}" \
        --namespace "${namespace}" \
        --create-namespace \
        --atomic --wait --timeout 5m \
        "$@"
}

kubectl_apply() {
    local file="$1"
    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would apply: ${file}"
        return 0
    fi
    ${KUBECTL} apply -f "${file}"
}

# --- SSH helpers ---
ssh_cmd() {
    local host="$1"
    shift
    ${SSH} -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${host}" "$@"
}

# --- Node iteration helpers ---
get_all_nodes() {
    local cp_nodes worker_nodes
    cp_nodes=$(${YQ} eval '.nodes.control_plane[].name' "${VALUES_FILE}")
    worker_nodes=$(${YQ} eval '.nodes.workers[].name' "${VALUES_FILE}")
    echo "${cp_nodes}"
    if [[ -n "${worker_nodes}" && "${worker_nodes}" != "null" ]]; then
        echo "${worker_nodes}"
    fi
}

get_node_ip() {
    local node_name="$1"
    local ip
    ip=$(${YQ} eval "(.nodes.control_plane[] | select(.name == \"${node_name}\") | .ip) // (.nodes.workers[] | select(.name == \"${node_name}\") | .ip)" "${VALUES_FILE}")
    # Strip CIDR suffix
    echo "${ip%%/*}"
}

get_bastion_ip() {
    # Use management.bastion_ip (e.g., Tailscale) if set, otherwise fall back to node LAN IP
    local explicit_ip
    explicit_ip=$(yq_read_default '.management.bastion_ip' '')
    if [[ -n "${explicit_ip}" && "${explicit_ip}" != "null" ]]; then
        echo "${explicit_ip}"
    else
        local bastion
        bastion=$(yq_read '.management.bastion_host')
        get_node_ip "${bastion}"
    fi
}

get_ansible_user() {
    yq_read '.nodes.ansible_user'
}

get_superuser() {
    yq_read '.nodes.superuser'
}

get_ssh_key() {
    yq_read '.nodes.ssh_key'
}
