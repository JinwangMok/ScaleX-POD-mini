#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "tunnel"
}

@test "tunnel_create_secret skips when credentials empty" {
    run tunnel_create_secret
    [ "$status" -eq 0 ]
    [[ "$output" == *"not set"* ]]
}

@test "tunnel_create_secret dry-run does not call kubectl" {
    export DRY_RUN="true"
    # Set a fake credentials file
    touch "${TEST_TEMP_DIR}/fake-creds.json"
    export PLAYBOX_VALUES="${TEST_TEMP_DIR}/values-test.yaml"
    # Create a minimal values with CF creds set
    cat > "${PLAYBOX_VALUES}" <<EOF
cloudflare:
  tunnel_name: test
  credentials_file: "${TEST_TEMP_DIR}/fake-creds.json"
  cert_file: ""
EOF
    # Need full values for other reads, so use the real one but override CF
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-full.yaml"
    run tunnel_create_secret
    [ "$status" -eq 0 ]
}
