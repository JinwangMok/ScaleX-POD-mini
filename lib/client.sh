#!/usr/bin/env bash
# lib/client.sh — Client kubeconfig generation

client_generate_kubeconfig() {
    log_step "Generating client kubeconfig..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would generate client kubeconfig"
        return 0
    fi

    local api_domain auth_domain realm client_id
    api_domain=$(yq_read '.domains.k8s_api')
    auth_domain=$(yq_read '.domains.auth')
    realm=$(yq_read '.keycloak.realm')
    client_id=$(yq_read '.keycloak.client_id')

    # Fetch client secret from Keycloak
    local client_secret=""
    log_info "Fetching Keycloak client secret..."

    # Port-forward keycloak
    ${KUBECTL} -n keycloak port-forward svc/keycloak 8080:80 &
    local pf_pid=$!
    sleep 3

    local kc_admin kc_password
    kc_admin=$(yq_read '.keycloak.admin_user')
    kc_password=$(yq_read '.keycloak.admin_password')

    local token
    token=$(${CURL} -s -X POST "http://localhost:8080/realms/master/protocol/openid-connect/token" \
        -d "client_id=admin-cli" \
        -d "username=${kc_admin}" \
        -d "password=${kc_password}" \
        -d "grant_type=password" | ${YQ} eval '.access_token' -)

    if [[ -n "${token}" && "${token}" != "null" ]]; then
        local client_uuid
        client_uuid=$(${CURL} -s "http://localhost:8080/admin/realms/${realm}/clients?clientId=${client_id}" \
            -H "Authorization: Bearer ${token}" | ${YQ} eval '.[0].id' -)
        if [[ -n "${client_uuid}" && "${client_uuid}" != "null" ]]; then
            client_secret=$(${CURL} -s "http://localhost:8080/admin/realms/${realm}/clients/${client_uuid}/client-secret" \
                -H "Authorization: Bearer ${token}" | ${YQ} eval '.value' -)
        fi
    fi

    kill "${pf_pid}" 2>/dev/null || true

    local output="${PLAYBOX_ROOT}/client/kubeconfig-oidc.yaml"

    cat > "${output}" <<EOF
apiVersion: v1
kind: Config
clusters:
  - name: playbox
    cluster:
      server: https://${api_domain}
contexts:
  - name: playbox-oidc
    context:
      cluster: playbox
      user: oidc-user
current-context: playbox-oidc
users:
  - name: oidc-user
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1beta1
        command: kubectl
        args:
          - "oidc-login"
          - "get-token"
          - "--oidc-issuer-url=https://${auth_domain}/realms/${realm}"
          - "--oidc-client-id=${client_id}"
          - "--oidc-client-secret=${client_secret}"
          - "--oidc-extra-scope=email,groups"
          - "--grant-type=authcode"
EOF

    log_info "Client kubeconfig generated: ${output}"
    echo ""
    log_info "=== Client Setup Instructions ==="
    log_info "1. Install kubectl and kubelogin: kubectl krew install oidc-login"
    log_info "2. Copy kubeconfig: cp ${output} ~/.kube/config"
    log_info "3. Test: kubectl get nodes → browser opens for OIDC login"
}
