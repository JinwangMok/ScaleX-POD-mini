#!/usr/bin/env bash
# lib/cluster.sh — Tower (k3s via OpenTofu) and Sandbox (kubespray) cluster operations

# --- Tower ---

cluster_tower_create() {
    log_step "Creating tower cluster..."

    local tower_enabled
    tower_enabled=$(yq_read '.tower.enabled')
    if [[ "${tower_enabled}" != "true" ]]; then
        log_info "Tower disabled, skipping"
        return 0
    fi

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would create tower VM via OpenTofu"
        return 0
    fi

    local tower_host tower_ip vm_name cpus memory disk k3s_version ssh_key
    tower_host=$(yq_read '.tower.host')
    tower_ip=$(yq_read '.tower.vm.ip')
    vm_name=$(yq_read '.tower.vm.name')
    cpus=$(yq_read '.tower.vm.cpus')
    memory=$(yq_read '.tower.vm.memory_mb')
    disk=$(yq_read '.tower.vm.disk_gb')
    k3s_version=$(yq_read '.tower.k3s.version')
    ssh_key=$(yq_read '.nodes.ssh_key')

    log_info "Tower VM: ${vm_name} on ${tower_host} (${cpus} CPU, ${memory}MB RAM, ${disk}GB disk)"

    # Install KVM/libvirt on tower host if needed
    cluster_tower_ensure_kvm "${tower_host}"

    # Run OpenTofu
    cd "${PLAYBOX_ROOT}/tofu"
    ${TOFU} init -input=false
    ${TOFU} apply -auto-approve \
        -var="vm_name=${vm_name}" \
        -var="cpus=${cpus}" \
        -var="memory_mb=${memory}" \
        -var="disk_gb=${disk}" \
        -var="tower_ip=${tower_ip}" \
        -var="k3s_version=${k3s_version}" \
        -var="ssh_public_key=${ssh_key}.pub"
    cd "${PLAYBOX_ROOT}"

    # Wait for tower to be reachable
    log_info "Waiting for tower VM to be reachable..."
    local retries=30
    while ! ssh_cmd "jinwang@${tower_ip}" "hostname" &>/dev/null; do
        retries=$((retries - 1))
        if [[ ${retries} -le 0 ]]; then
            log_error "Tower VM not reachable after 5 minutes"
            return 1
        fi
        sleep 10
    done

    # Wait for k3s to be ready
    log_info "Waiting for k3s on tower..."
    local k3s_retries=18
    while ! ssh_cmd "jinwang@${tower_ip}" "sudo kubectl get nodes" &>/dev/null; do
        k3s_retries=$((k3s_retries - 1))
        if [[ ${k3s_retries} -le 0 ]]; then
            log_error "k3s not ready after 3 minutes"
            return 1
        fi
        sleep 10
    done

    # Fetch kubeconfig
    log_info "Fetching tower kubeconfig..."
    ssh_cmd "jinwang@${tower_ip}" "sudo cat /etc/rancher/k3s/k3s.yaml" | \
        sed "s/127.0.0.1/${tower_ip}/" > "${PLAYBOX_ROOT}/_generated/tower.kubeconfig"

    log_info "Tower cluster created successfully"
}

cluster_tower_ensure_kvm() {
    local host="$1"
    local host_ip
    host_ip=$(get_node_ip "${host}")
    local user
    user=$(get_superuser)

    if ssh_cmd "${user}@${host_ip}" "command -v virsh" &>/dev/null; then
        log_info "KVM already installed on ${host}"
        return 0
    fi

    log_info "Installing KVM/libvirt on ${host}..."
    ssh_cmd "${user}@${host_ip}" "sudo apt-get update && sudo apt-get install -y qemu-kvm libvirt-daemon-system libvirt-clients bridge-utils virtinst"
}

cluster_tower_destroy() {
    log_step "Destroying tower cluster..."

    local tower_enabled
    tower_enabled=$(yq_read '.tower.enabled')
    if [[ "${tower_enabled}" != "true" ]]; then
        log_info "Tower disabled, nothing to destroy"
        return 0
    fi

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would destroy tower VM"
        return 0
    fi

    cd "${PLAYBOX_ROOT}/tofu"
    ${TOFU} destroy -auto-approve
    cd "${PLAYBOX_ROOT}"
    rm -f "${PLAYBOX_ROOT}/_generated/tower.kubeconfig"
    log_info "Tower cluster destroyed"
}

# --- Sandbox ---

cluster_sandbox_create() {
    log_step "Creating sandbox cluster..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would create sandbox cluster via kubespray"
        return 0
    fi

    local k8s_version kubespray_version
    k8s_version=$(yq_read '.sandbox.kubernetes_version')
    kubespray_version=$(yq_read '.sandbox.kubespray_version')

    # Clone kubespray if needed
    cluster_sandbox_clone_kubespray "${kubespray_version}"

    # Generate inventory
    cluster_sandbox_generate_inventory

    # Generate cluster vars
    cluster_sandbox_generate_vars

    # Run kubespray
    log_info "Running kubespray cluster.yml..."
    cd "${PLAYBOX_ROOT}/kubespray/kubespray"
    ${ANSIBLE_PLAYBOOK} \
        -i "${PLAYBOX_ROOT}/_generated/inventory.ini" \
        --become \
        -e "@${PLAYBOX_ROOT}/_generated/cluster-vars.yml" \
        cluster.yml
    cd "${PLAYBOX_ROOT}"

    # Fetch kubeconfig
    local cp_ip
    cp_ip=$(get_node_ip "$(${YQ} eval '.nodes.control_plane[0].name' "${VALUES_FILE}")")
    local ansible_user
    ansible_user=$(get_ansible_user)

    log_info "Fetching sandbox kubeconfig..."
    ssh_cmd "${ansible_user}@${cp_ip}" "sudo cat /etc/kubernetes/admin.conf" | \
        sed "s|https://.*:6443|https://${cp_ip}:6443|" > "${PLAYBOX_ROOT}/_generated/sandbox.kubeconfig"

    # Delete kube-proxy DaemonSet (Cilium replaces it)
    export KUBECONFIG="${PLAYBOX_ROOT}/_generated/sandbox.kubeconfig"
    ${KUBECTL} -n kube-system delete ds kube-proxy --ignore-not-found
    ${KUBECTL} -n kube-system delete cm kube-proxy --ignore-not-found

    log_info "Sandbox cluster created successfully"
    log_info "Verifying nodes..."
    ${KUBECTL} get nodes
}

cluster_sandbox_clone_kubespray() {
    local version="$1"
    local kubespray_dir="${PLAYBOX_ROOT}/kubespray/kubespray"

    if [[ -d "${kubespray_dir}/.git" ]]; then
        log_info "Kubespray already cloned, checking version..."
        cd "${kubespray_dir}"
        local current_tag
        current_tag=$(git describe --tags --exact-match 2>/dev/null || echo "none")
        if [[ "${current_tag}" == "${version}" ]]; then
            log_info "Kubespray ${version} already checked out"
            cd "${PLAYBOX_ROOT}"
            return 0
        fi
        git fetch --tags
        git checkout "${version}"
        cd "${PLAYBOX_ROOT}"
    else
        log_info "Cloning kubespray ${version}..."
        mkdir -p "${PLAYBOX_ROOT}/kubespray"
        git clone --branch "${version}" --depth 1 \
            https://github.com/kubernetes-sigs/kubespray.git "${kubespray_dir}"
    fi

    # Install requirements
    log_info "Installing kubespray requirements..."
    pip install -r "${kubespray_dir}/requirements.txt"
}

cluster_sandbox_generate_inventory() {
    log_info "Generating kubespray inventory..."
    local output="${PLAYBOX_ROOT}/_generated/inventory.ini"
    local bastion
    bastion=$(yq_read '.management.bastion_host')
    local bastion_ip
    bastion_ip=$(get_node_ip "${bastion}")
    local bastion_user
    bastion_user=$(yq_read '.management.bastion_user')
    local ansible_user
    ansible_user=$(get_ansible_user)
    local ssh_key
    ssh_key=$(get_ssh_key)

    cat > "${output}" <<EOF
[all]
EOF

    # Add all nodes
    local nodes
    nodes=$(get_all_nodes)
    for node in ${nodes}; do
        local ip
        ip=$(get_node_ip "${node}")
        local proxy_cmd=""
        if [[ "${node}" != "${bastion}" ]]; then
            proxy_cmd="ansible_ssh_common_args='-o ProxyJump=${bastion_user}@${bastion_ip}'"
        fi
        echo "${node} ansible_host=${ip} ansible_user=${ansible_user} ansible_ssh_private_key_file=${ssh_key} ${proxy_cmd}" >> "${output}"
    done

    cat >> "${output}" <<EOF

[kube_control_plane]
EOF
    ${YQ} eval '.nodes.control_plane[].name' "${VALUES_FILE}" >> "${output}"

    cat >> "${output}" <<EOF

[kube_node]
EOF
    get_all_nodes >> "${output}"

    cat >> "${output}" <<EOF

[etcd]
EOF
    ${YQ} eval '.nodes.control_plane[].name' "${VALUES_FILE}" >> "${output}"

    cat >> "${output}" <<EOF

[k8s_cluster:children]
kube_control_plane
kube_node
EOF

    log_info "Inventory generated: ${output}"
}

cluster_sandbox_generate_vars() {
    log_info "Generating kubespray cluster vars..."
    local output="${PLAYBOX_ROOT}/_generated/cluster-vars.yml"

    local ansible_user k8s_version auth_domain realm client_id
    ansible_user=$(get_ansible_user)
    k8s_version=$(yq_read '.sandbox.kubernetes_version')
    auth_domain=$(yq_read '.domains.auth')
    realm=$(yq_read '.keycloak.realm')
    client_id=$(yq_read '.keycloak.client_id')

    cat > "${output}" <<EOF
ansible_user: ${ansible_user}
kube_owner: "{{ ansible_user }}"

kube_version: v${k8s_version}
kube_network_plugin: cni
kube_proxy_remove: true

cgroup_driver: systemd
kube_vip_enabled: false
helm_enabled: true

kube_oidc_auth: true
kube_oidc_url: "https://${auth_domain}/realms/${realm}"
kube_oidc_client_id: "${client_id}"
kube_oidc_username_claim: "email"
kube_oidc_username_prefix: "oidc:"
kube_oidc_groups_claim: "groups"
kube_oidc_groups_prefix: "oidc:"
kube_apiserver_enable_admission_plugins:
  - NodeRestriction
  - PodTolerationRestriction

kube_api_anonymous_auth: true
EOF

    log_info "Cluster vars generated: ${output}"
}

cluster_sandbox_destroy() {
    log_step "Destroying sandbox cluster..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would destroy sandbox cluster via kubespray reset"
        return 0
    fi

    local kubespray_dir="${PLAYBOX_ROOT}/kubespray/kubespray"
    if [[ ! -d "${kubespray_dir}" ]]; then
        log_error "Kubespray not found. Cannot reset."
        return 1
    fi

    cd "${kubespray_dir}"
    ${ANSIBLE_PLAYBOOK} \
        -i "${PLAYBOX_ROOT}/_generated/inventory.ini" \
        --become \
        -e "@${PLAYBOX_ROOT}/_generated/cluster-vars.yml" \
        -e "reset_confirmation=yes" \
        reset.yml
    cd "${PLAYBOX_ROOT}"

    rm -f "${PLAYBOX_ROOT}/_generated/sandbox.kubeconfig"
    log_info "Sandbox cluster destroyed"
}
