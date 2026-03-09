# Common setup for all BATS tests

# Project root
export PROJECT_ROOT="$(cd "${BATS_TEST_DIRNAME}/../../.." && pwd)"

# Load bats helpers if available
if [[ -d "${BATS_TEST_DIRNAME}/bats-support" ]]; then
    load "${BATS_TEST_DIRNAME}/bats-support/load"
    load "${BATS_TEST_DIRNAME}/bats-assert/load"
fi

# Create temp dir for test artifacts
setup() {
    TEST_TEMP_DIR="$(mktemp -d)"
    export TEST_TEMP_DIR

    # Mock commands that write to stdout
    export PLAYBOX_YQ="yq"
    export PLAYBOX_VALUES="${PROJECT_ROOT}/tests/fixtures/values-full.yaml"

    # Create mock scripts directory
    MOCK_DIR="${TEST_TEMP_DIR}/mocks"
    mkdir -p "${MOCK_DIR}"
    export PATH="${MOCK_DIR}:${PATH}"
}

teardown() {
    rm -rf "${TEST_TEMP_DIR}"
}

# Helper: create a mock command
create_mock() {
    local cmd="$1"
    local output="${2:-}"
    local exit_code="${3:-0}"
    cat > "${MOCK_DIR}/${cmd}" <<EOF
#!/usr/bin/env bash
echo "${output}"
exit ${exit_code}
EOF
    chmod +x "${MOCK_DIR}/${cmd}"
}

# Helper: create a mock that records calls
create_recording_mock() {
    local cmd="$1"
    local record_file="${TEST_TEMP_DIR}/${cmd}.calls"
    cat > "${MOCK_DIR}/${cmd}" <<'EOF'
#!/usr/bin/env bash
echo "$@" >> "${RECORD_FILE}"
echo "mock-${CMD_NAME}-output"
EOF
    sed -i "s|\${RECORD_FILE}|${record_file}|g" "${MOCK_DIR}/${cmd}"
    sed -i "s|\${CMD_NAME}|${cmd}|g" "${MOCK_DIR}/${cmd}"
    chmod +x "${MOCK_DIR}/${cmd}"
}

# Helper: source a lib module with common.sh
source_lib() {
    local module="$1"
    source "${PROJECT_ROOT}/lib/common.sh"
    source "${PROJECT_ROOT}/lib/${module}.sh"
}
