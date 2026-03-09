#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source "${PROJECT_ROOT}/lib/common.sh"
}

@test "log_info outputs with [INFO] prefix" {
    run log_info "test message"
    [[ "$output" == *"[INFO]"* ]]
    [[ "$output" == *"test message"* ]]
}

@test "log_warn outputs with [WARN] prefix" {
    run log_warn "warning message"
    [[ "$output" == *"[WARN]"* ]]
    [[ "$output" == *"warning message"* ]]
}

@test "log_error outputs with [ERROR] prefix" {
    run log_error "error message"
    [[ "$output" == *"[ERROR]"* ]]
    [[ "$output" == *"error message"* ]]
}

@test "log_step outputs with ==> prefix" {
    run log_step "step name"
    [[ "$output" == *"==>"* ]]
    [[ "$output" == *"step name"* ]]
}

@test "yq_read reads cluster.name from values" {
    run yq_read '.cluster.name'
    [[ "$output" == "playbox" ]]
}

@test "yq_read reads network.gateway from values" {
    run yq_read '.network.gateway'
    [[ "$output" == "192.168.88.1" ]]
}

@test "yq_read_default returns default when field missing" {
    run yq_read_default '.nonexistent.field' 'fallback'
    [[ "$output" == "fallback" ]]
}

@test "get_all_nodes returns all 4 nodes" {
    run get_all_nodes
    [[ "$output" == *"playbox-0"* ]]
    [[ "$output" == *"playbox-1"* ]]
    [[ "$output" == *"playbox-2"* ]]
    [[ "$output" == *"playbox-3"* ]]
}

@test "get_node_ip returns correct IP without CIDR" {
    run get_node_ip "playbox-0"
    [[ "$output" == "192.168.88.8" ]]
}

@test "get_node_ip returns correct IP for worker" {
    run get_node_ip "playbox-3"
    [[ "$output" == "192.168.88.11" ]]
}

@test "get_ansible_user returns ansible_user" {
    run get_ansible_user
    [[ "$output" == "ansible_user" ]]
}

@test "get_superuser returns jinwang" {
    run get_superuser
    [[ "$output" == "jinwang" ]]
}

@test "ensure_namespace in dry-run mode does not call kubectl" {
    export DRY_RUN="true"
    export PLAYBOX_KUBECTL="false"
    run ensure_namespace "test-ns"
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}

@test "helm_install in dry-run mode does not call helm" {
    export DRY_RUN="true"
    export PLAYBOX_HELM="false"
    run helm_install "test-release" "test-chart" "test-ns"
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}
