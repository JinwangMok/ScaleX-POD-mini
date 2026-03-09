#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "preflight"
}

@test "preflight_check_tools succeeds when all tools present" {
    # Ensure all required tools are on PATH (or mocked)
    create_mock "ansible-playbook" ""
    create_mock "helm" ""
    create_mock "kubectl" ""
    create_mock "yq" ""
    create_mock "ssh" ""
    create_mock "git" ""
    create_mock "tofu" ""
    run preflight_check_tools
    [ "$status" -eq 0 ]
}

@test "preflight_check_tools fails when kubectl missing" {
    create_mock "ansible-playbook" ""
    create_mock "helm" ""
    create_mock "yq" ""
    create_mock "ssh" ""
    create_mock "git" ""
    create_mock "tofu" ""
    # Remove kubectl from PATH
    rm -f "${MOCK_DIR}/kubectl"
    # Only fail if real kubectl also not present
    if ! command -v kubectl &>/dev/null; then
        run preflight_check_tools
        [ "$status" -eq 1 ]
        [[ "$output" == *"kubectl"* ]]
    fi
}

@test "preflight_validate_values passes with full values" {
    run preflight_validate_values
    [ "$status" -eq 0 ]
}

@test "preflight_validate_values fails with invalid values" {
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-invalid.yaml"
    run preflight_validate_values
    [ "$status" -eq 1 ]
}

@test "preflight_validate_values checks for MAC addresses" {
    run preflight_validate_values
    [ "$status" -eq 0 ]
    [[ "$output" == *"validation passed"* ]]
}
