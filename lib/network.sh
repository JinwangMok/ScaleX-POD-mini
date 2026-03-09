#!/usr/bin/env bash
# lib/network.sh — Network configuration (netplan generation)

network_get_bond_interfaces() {
    local node_name="$1"
    # Get the node's bond_roles and filter interfaces accordingly
    # Returns JSON array of interfaces suitable for bonding
    ${YQ} eval "
      ((.nodes.control_plane[] | select(.name == \"${node_name}\")) //
       (.nodes.workers[] | select(.name == \"${node_name}\"))) as \$node |
      \$node.interfaces[] |
      select(.role as \$r | \$node.network.bond_roles | contains([\$r])) |
      select(.state != \"down\")
    " "${VALUES_FILE}"
}

network_should_create_bridge() {
    local node_name="$1"
    local tower_enabled
    tower_enabled=$(yq_read '.tower.enabled')
    local tower_host
    tower_host=$(yq_read '.tower.host')
    if [[ "${tower_enabled}" == "true" && "${node_name}" == "${tower_host}" ]]; then
        echo "true"
    else
        echo "false"
    fi
}

network_generate_netplan() {
    local node_name="$1"
    local output_dir="${2:-${PLAYBOX_ROOT}/_generated}"
    mkdir -p "${output_dir}"

    log_info "Generating netplan for ${node_name}..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would generate netplan for ${node_name}"
        return 0
    fi

    local node_ip gateway nameservers bond_mode
    node_ip=$(get_node_ip "${node_name}")
    local node_cidr
    node_cidr=$(${YQ} eval "
      ((.nodes.control_plane[] | select(.name == \"${node_name}\")) //
       (.nodes.workers[] | select(.name == \"${node_name}\"))).ip
    " "${VALUES_FILE}")
    gateway=$(yq_read '.network.gateway')
    nameservers=$(yq_read '.network.nameservers | join(", ")')
    bond_mode=$(${YQ} eval "
      ((.nodes.control_plane[] | select(.name == \"${node_name}\")) //
       (.nodes.workers[] | select(.name == \"${node_name}\"))).network.bond_mode
    " "${VALUES_FILE}")

    local create_bridge
    create_bridge=$(network_should_create_bridge "${node_name}")

    log_info "Node: ${node_name}, IP: ${node_cidr}, Bridge: ${create_bridge}"
}

network_apply_netplan() {
    local node_name="$1"
    local user="$2"
    local node_ip
    node_ip=$(get_node_ip "${node_name}")

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would apply netplan on ${node_name}"
        return 0
    fi

    log_info "Applying netplan on ${node_name}..."
    ssh_cmd "${user}@${node_ip}" "sudo netplan try --timeout 120"
}

network_prepare_all() {
    log_step "Preparing network configuration..."

    local ansible_user
    ansible_user=$(get_ansible_user)
    local superuser
    superuser=$(get_superuser)

    # Use ansible to apply netplan
    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would run ansible prepare-nodes playbook"
        return 0
    fi

    local bastion
    bastion=$(yq_read '.management.bastion_host')
    local bastion_ip
    bastion_ip=$(get_node_ip "${bastion}")

    ${ANSIBLE_PLAYBOOK} \
        -i "${PLAYBOX_ROOT}/_generated/inventory.ini" \
        "${PLAYBOX_ROOT}/ansible/prepare-nodes.yml" \
        --extra-vars "@${VALUES_FILE}" \
        -e "playbox_root=${PLAYBOX_ROOT}"
}
