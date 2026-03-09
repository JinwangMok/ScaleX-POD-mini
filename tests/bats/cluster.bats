#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "cluster"
}

# --- Tower ---
@test "cluster_tower_create skips when tower disabled" {
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-minimal.yaml"
    run cluster_tower_create
    [ "$status" -eq 0 ]
    [[ "$output" == *"Tower disabled"* ]]
}

@test "cluster_tower_create dry-run does not call tofu" {
    export DRY_RUN="true"
    run cluster_tower_create
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}

@test "cluster_tower_destroy skips when tower disabled" {
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-minimal.yaml"
    run cluster_tower_destroy
    [ "$status" -eq 0 ]
    [[ "$output" == *"Tower disabled"* ]]
}

# --- Sandbox ---
@test "cluster_sandbox_create dry-run does not run kubespray" {
    export DRY_RUN="true"
    run cluster_sandbox_create
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}

@test "cluster_sandbox_generate_vars creates correct vars" {
    cluster_sandbox_generate_vars
    local output="${PLAYBOX_ROOT}/_generated/cluster-vars.yml"
    [ -f "${output}" ]
    grep -q "kube_oidc_auth: true" "${output}"
    grep -q "kube_proxy_remove: true" "${output}"
    grep -q "realms/kubernetes" "${output}"
    # Must NOT use realms/master
    ! grep -q "realms/master" "${output}"
}

@test "cluster_sandbox_generate_inventory creates inventory" {
    cluster_sandbox_generate_inventory
    local output="${PLAYBOX_ROOT}/_generated/inventory.ini"
    [ -f "${output}" ]
    grep -q "playbox-0" "${output}"
    grep -q "playbox-1" "${output}"
    grep -q "playbox-2" "${output}"
    grep -q "playbox-3" "${output}"
    grep -q "kube_control_plane" "${output}"
    grep -q "kube_node" "${output}"
}

@test "cluster_sandbox_generate_inventory includes ProxyJump for non-bastion" {
    cluster_sandbox_generate_inventory
    local output="${PLAYBOX_ROOT}/_generated/inventory.ini"
    grep "playbox-1" "${output}" | grep -q "ProxyJump"
}

@test "cluster_sandbox_generate_inventory no ProxyJump for bastion" {
    cluster_sandbox_generate_inventory
    local output="${PLAYBOX_ROOT}/_generated/inventory.ini"
    local bastion_line
    bastion_line=$(grep "^playbox-0 " "${output}")
    [[ "${bastion_line}" != *"ProxyJump"* ]]
}
