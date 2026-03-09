#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "client"
}

@test "client_generate_kubeconfig dry-run does not create file" {
    export DRY_RUN="true"
    run client_generate_kubeconfig
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}
