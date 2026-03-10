#!/usr/bin/env bats

setup() {
    load 'test_helper/common-setup'
    source_lib "gitops"
}

@test "gitops_bootstrap dry-run does not call helm or kubectl" {
    export DRY_RUN="true"
    run gitops_bootstrap
    [ "$status" -eq 0 ]
    [[ "$output" == *"dry-run"* ]]
}

@test "spread.yaml exists and is valid YAML" {
    python3 -c "
import yaml
with open('${PROJECT_ROOT}/gitops/bootstrap/spread.yaml') as f:
    list(yaml.safe_load_all(f))
print('valid')
"
}

@test "generators exist for tower and sandbox" {
    [ -f "${PROJECT_ROOT}/gitops/generators/tower/common-generator.yaml" ]
    [ -f "${PROJECT_ROOT}/gitops/generators/tower/tower-generator.yaml" ]
    [ -f "${PROJECT_ROOT}/gitops/generators/sandbox/common-generator.yaml" ]
    [ -f "${PROJECT_ROOT}/gitops/generators/sandbox/sandbox-generator.yaml" ]
}

@test "common apps present in tower common-generator" {
    local gen="${PROJECT_ROOT}/gitops/generators/tower/common-generator.yaml"
    for app in cilium cilium-resources cert-manager cluster-config kyverno; do
        grep -q "${app}" "${gen}"
    done
}

@test "tower-specific apps present in tower-generator" {
    local gen="${PROJECT_ROOT}/gitops/generators/tower/tower-generator.yaml"
    for app in argocd keycloak cloudflared-tunnel socks5-proxy; do
        grep -q "${app}" "${gen}"
    done
}
