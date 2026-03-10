#!/usr/bin/env bash
# lib/preflight.sh — Preflight checks

preflight_check_tools() {
    log_step "Checking required tools..."
    local missing=()
    local tools=(ansible-playbook helm kubectl yq ssh git)

    # Add tofu if tower enabled (check yq first before using it)
    if command -v "${YQ}" &>/dev/null; then
        local tower_enabled
        tower_enabled=$(yq_read '.tower.enabled')
        if [[ "${tower_enabled}" == "true" ]]; then
            tools+=(tofu)
        fi
    fi

    for tool in "${tools[@]}"; do
        if ! command -v "${tool}" &>/dev/null; then
            missing+=("${tool}")
        fi
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "Missing tools: ${missing[*]}"
        return 1
    fi
    log_info "All required tools found"
}

preflight_validate_values() {
    log_step "Validating values.yaml..."

    # Check required fields
    local required_fields=(
        ".cluster.name"
        ".cluster.domain"
        ".nodes.superuser"
        ".nodes.ansible_user"
        ".nodes.ssh_key"
        ".network.gateway"
    )

    for field in "${required_fields[@]}"; do
        local val
        val=$(yq_read "${field}")
        if [[ -z "${val}" || "${val}" == "null" ]]; then
            log_error "Required field missing or empty: ${field}"
            return 1
        fi
    done

    # Check at least one control plane node
    local cp_count
    cp_count=$(${YQ} eval '.nodes.control_plane | length' "${VALUES_FILE}")
    if [[ "${cp_count}" -lt 1 ]]; then
        log_error "At least one control_plane node is required"
        return 1
    fi

    # Check all nodes have MAC addresses
    local all_macs
    all_macs=$(${YQ} eval '
      [.nodes.control_plane[].interfaces[].mac, .nodes.workers[].interfaces[].mac]
      | .[] | select(. == null or . == "")
    ' "${VALUES_FILE}" 2>/dev/null || true)
    if [[ -n "${all_macs}" ]]; then
        log_error "All interfaces must have MAC addresses"
        return 1
    fi

    log_info "values.yaml validation passed"
}

preflight_check_ssh() {
    log_step "Testing SSH connectivity..."
    local superuser
    superuser=$(get_superuser)
    local bastion
    bastion=$(yq_read '.management.bastion_host')
    local bastion_ip
    bastion_ip=$(get_bastion_ip)

    # Test SSH to bastion
    if ! ssh_cmd "${superuser}@${bastion_ip}" "hostname" &>/dev/null; then
        log_error "Cannot SSH to bastion ${bastion} (${bastion_ip}) as ${superuser}"
        return 1
    fi
    log_info "SSH to bastion OK: ${bastion}"

    # Test SSH from bastion to all other nodes
    local nodes
    nodes=$(get_all_nodes)
    for node in ${nodes}; do
        local ip
        ip=$(get_node_ip "${node}")
        if [[ "${node}" == "${bastion}" ]]; then
            continue
        fi
        if ! ssh_cmd "${superuser}@${bastion_ip}" "${SSH} -o StrictHostKeyChecking=no ${superuser}@${ip} hostname" &>/dev/null; then
            log_error "Cannot SSH from bastion to ${node} (${ip})"
            return 1
        fi
        log_info "SSH to ${node} OK (via bastion)"
    done
}

preflight_run() {
    preflight_check_tools
    preflight_validate_values
    preflight_check_ssh
    log_info "Preflight checks passed!"
}
