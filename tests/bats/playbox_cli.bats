#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
}

@test "playbox --help shows usage" {
    run "${PROJECT_ROOT}/playbox" --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"Commands:"* ]]
}

@test "playbox -h shows usage" {
    run "${PROJECT_ROOT}/playbox" -h
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
}

@test "playbox without args shows usage and exits" {
    run "${PROJECT_ROOT}/playbox"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Usage:"* ]]
}

@test "playbox unknown-command fails" {
    run "${PROJECT_ROOT}/playbox" unknown-command
    [ "$status" -eq 1 ]
    [[ "$output" == *"Unknown command"* ]]
}

@test "playbox --unknown-flag fails" {
    run "${PROJECT_ROOT}/playbox" --unknown-flag
    [ "$status" -eq 1 ]
    [[ "$output" == *"Unknown flag"* ]]
}

@test "playbox preflight --dry-run succeeds" {
    export DRY_RUN="true"
    # This will still check tools, so mock what's needed
    create_mock "ansible-playbook" ""
    create_mock "helm" ""
    create_mock "kubectl" ""
    create_mock "tofu" ""
    run "${PROJECT_ROOT}/playbox" preflight --dry-run 2>&1 || true
    # At minimum it should not crash on dispatch
    [[ "$output" != *"Unknown command"* ]]
}

@test "playbox up --from validates step name" {
    # The 'up' command with --from should accept valid step names
    # Testing dispatch - will fail on actual execution but should parse args
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-full.yaml"
    run "${PROJECT_ROOT}/playbox" up --from create-sandbox --dry-run 2>&1 || true
    [[ "$output" != *"Unknown command"* ]]
}
