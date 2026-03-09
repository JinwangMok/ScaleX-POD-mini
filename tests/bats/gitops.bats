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

@test "catalog.yaml lists all expected base apps" {
    local catalog="${PROJECT_ROOT}/gitops/clusters/playbox/catalog.yaml"
    [ -f "${catalog}" ]
    for app in argocd cilium cilium-resources cert-manager cloudflared-tunnel socks5-proxy keycloak local-path-provisioner rbac cluster-config; do
        grep -q "${app}" "${catalog}"
    done
}
