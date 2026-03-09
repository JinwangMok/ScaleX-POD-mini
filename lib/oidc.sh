#!/usr/bin/env bash
# lib/oidc.sh — Keycloak OIDC configuration via REST API

oidc_configure() {
    log_step "Configuring Keycloak OIDC..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would configure Keycloak OIDC"
        return 0
    fi

    local kc_admin kc_password realm client_id auth_domain
    kc_admin=$(yq_read '.keycloak.admin_user')
    kc_password=$(yq_read '.keycloak.admin_password')
    realm=$(yq_read '.keycloak.realm')
    client_id=$(yq_read '.keycloak.client_id')
    auth_domain=$(yq_read '.domains.auth')

    # Port-forward keycloak
    log_info "Setting up port-forward to Keycloak..."
    ${KUBECTL} -n keycloak port-forward svc/keycloak 8080:80 &
    local pf_pid=$!
    sleep 5

    local kc_url="http://localhost:8080"

    # Get admin token
    log_info "Getting admin token..."
    local token
    token=$(${CURL} -s -X POST "${kc_url}/realms/master/protocol/openid-connect/token" \
        -d "client_id=admin-cli" \
        -d "username=${kc_admin}" \
        -d "password=${kc_password}" \
        -d "grant_type=password" | ${YQ} eval '.access_token' -)

    if [[ -z "${token}" || "${token}" == "null" ]]; then
        kill "${pf_pid}" 2>/dev/null || true
        log_error "Failed to get Keycloak admin token"
        return 1
    fi

    # Create realm (idempotent)
    log_info "Creating realm: ${realm}..."
    ${CURL} -s -o /dev/null -w "%{http_code}" \
        -X POST "${kc_url}/admin/realms" \
        -H "Authorization: Bearer ${token}" \
        -H "Content-Type: application/json" \
        -d "{\"realm\": \"${realm}\", \"enabled\": true}" | grep -qE "201|409"

    # Create client
    log_info "Creating client: ${client_id}..."
    ${CURL} -s -o /dev/null \
        -X POST "${kc_url}/admin/realms/${realm}/clients" \
        -H "Authorization: Bearer ${token}" \
        -H "Content-Type: application/json" \
        -d "{
            \"clientId\": \"${client_id}\",
            \"enabled\": true,
            \"protocol\": \"openid-connect\",
            \"publicClient\": false,
            \"redirectUris\": [\"http://localhost:8000\", \"http://localhost:18000\"],
            \"webOrigins\": [\"+\"],
            \"standardFlowEnabled\": true,
            \"directAccessGrantsEnabled\": true,
            \"protocolMappers\": [{
                \"name\": \"groups\",
                \"protocol\": \"openid-connect\",
                \"protocolMapper\": \"oidc-group-membership-mapper\",
                \"config\": {
                    \"claim.name\": \"groups\",
                    \"full.path\": \"false\",
                    \"id.token.claim\": \"true\",
                    \"access.token.claim\": \"true\",
                    \"userinfo.token.claim\": \"true\"
                }
            }]
        }"

    # Create admin group
    log_info "Creating admin group..."
    ${CURL} -s -o /dev/null \
        -X POST "${kc_url}/admin/realms/${realm}/groups" \
        -H "Authorization: Bearer ${token}" \
        -H "Content-Type: application/json" \
        -d '{"name": "cluster-admin"}'

    # Create admin user
    log_info "Creating admin user..."
    ${CURL} -s -o /dev/null \
        -X POST "${kc_url}/admin/realms/${realm}/users" \
        -H "Authorization: Bearer ${token}" \
        -H "Content-Type: application/json" \
        -d "{
            \"username\": \"jinwang\",
            \"email\": \"jinwang@jinwang.dev\",
            \"enabled\": true,
            \"emailVerified\": true,
            \"credentials\": [{\"type\": \"password\", \"value\": \"${kc_password}\", \"temporary\": false}]
        }"

    kill "${pf_pid}" 2>/dev/null || true

    log_info "Keycloak OIDC configured successfully"
}
