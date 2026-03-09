#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "oidc"
}

@test "oidc_configure dry-run does not call curl" {
    export DRY_RUN="true"
    run oidc_configure
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}
