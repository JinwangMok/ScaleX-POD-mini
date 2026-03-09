#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "network"
}

@test "network_should_create_bridge returns true for tower host" {
    run network_should_create_bridge "playbox-0"
    [[ "$output" == "true" ]]
}

@test "network_should_create_bridge returns false for non-tower host" {
    run network_should_create_bridge "playbox-1"
    [[ "$output" == "false" ]]
}

@test "network_should_create_bridge returns false when tower disabled" {
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-minimal.yaml"
    run network_should_create_bridge "playbox-0"
    [[ "$output" == "false" ]]
}

@test "network_generate_netplan dry-run does not create files" {
    export DRY_RUN="true"
    run network_generate_netplan "playbox-0"
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}
