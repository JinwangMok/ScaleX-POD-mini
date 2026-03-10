#!/usr/bin/env bash
# lib/gitops.sh — ArgoCD bootstrap and GitOps operations

gitops_bootstrap() {
    log_step "Bootstrapping ArgoCD and GitOps..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would bootstrap ArgoCD"
        return 0
    fi

    local argocd_ns
    argocd_ns=$(yq_read '.argocd.namespace')

    # Ensure namespace
    ensure_namespace "${argocd_ns}"

    # Add helm repo if needed
    ${HELM} repo add argo https://argoproj.github.io/argo-helm 2>/dev/null || true
    ${HELM} repo update argo 2>/dev/null || true

    # Install ArgoCD via helm (syncWave 0)
    log_info "Installing ArgoCD..."
    local argocd_version
    argocd_version=$(yq_read '.versions.argocd_chart')
    helm_install "argocd" "argo/argo-cd" "${argocd_ns}" \
        --version "${argocd_version}" \
        --set server.service.type=ClusterIP \
        --set server.replicas=1 \
        --set controller.replicas=1 \
        --set "configs.params.server\.insecure=true" \
        --set "global.domain=$(yq_read '.domains.argocd')"

    # Create secrets
    gitops_create_secrets

    # Apply spread.yaml (root Application + AppProjects)
    log_info "Applying spread.yaml..."
    kubectl_apply "${PLAYBOX_ROOT}/gitops/bootstrap/spread.yaml"

    log_info "GitOps bootstrap complete"
    log_info "Verifying ApplicationSets..."
    ${KUBECTL} get applicationsets -n "${argocd_ns}" 2>/dev/null || log_warn "ApplicationSets not yet synced (may take a minute)"
}

gitops_create_secrets() {
    log_info "Creating required secrets..."

    local argocd_ns
    argocd_ns=$(yq_read '.argocd.namespace')

    # --- Secret source: credentials/secrets.yaml (preferred) or values.yaml (fallback) ---
    local secrets_file="${PLAYBOX_ROOT}/credentials/secrets.yaml"

    _read_secret() {
        local path="$1"
        local fallback_path="$2"
        local result=""
        if [[ -f "${secrets_file}" ]]; then
            result=$(${YQ} eval "${path}" "${secrets_file}" 2>/dev/null)
        fi
        if [[ -z "${result}" || "${result}" == "null" ]]; then
            result=$(yq_read "${fallback_path}")
        fi
        echo "${result}"
    }

    # ArgoCD repo credentials (if private)
    local repo_pat
    repo_pat=$(_read_secret '.argocd.repo_pat' '.argocd.repo_pat')
    if [[ -n "${repo_pat}" && "${repo_pat}" != "null" && "${repo_pat}" != "" ]]; then
        local repo_url
        repo_url=$(yq_read '.argocd.repo_url')
        ${KUBECTL} -n "${argocd_ns}" create secret generic repo-creds \
            --from-literal=url="${repo_url}" \
            --from-literal=password="${repo_pat}" \
            --from-literal=username="git" \
            --from-literal=type="git" \
            --dry-run=client -o yaml | ${KUBECTL} apply -f -
        ${KUBECTL} -n "${argocd_ns}" label secret repo-creds \
            argocd.argoproj.io/secret-type=repository --overwrite
    fi

    # Cloudflare tunnel credentials
    local cf_creds
    cf_creds=$(_read_secret '.cloudflare.credentials_file' '.cloudflare.credentials_file')
    if [[ -n "${cf_creds}" && "${cf_creds}" != "null" && "${cf_creds}" != "" && -f "${cf_creds}" ]]; then
        ensure_namespace "kube-tunnel"
        ${KUBECTL} -n kube-tunnel create secret generic cloudflared-tunnel-credentials \
            --from-file=credentials.json="${cf_creds}" \
            --dry-run=client -o yaml | ${KUBECTL} apply -f -
    else
        log_warn "Cloudflare credentials not set — tunnel will not function until provided"
    fi

    # Keycloak DB password
    local kc_db_pass
    kc_db_pass=$(_read_secret '.keycloak.db_password' '.keycloak.db_password')
    ensure_namespace "keycloak"
    ${KUBECTL} -n keycloak create secret generic keycloak-db \
        --from-literal=password="${kc_db_pass}" \
        --dry-run=client -o yaml | ${KUBECTL} apply -f -

    # Keycloak admin password
    local kc_admin_pass
    kc_admin_pass=$(_read_secret '.keycloak.admin_password' '.keycloak.admin_password')
    ${KUBECTL} -n keycloak create secret generic keycloak-admin \
        --from-literal=KEYCLOAK_ADMIN="$(yq_read '.keycloak.admin_user')" \
        --from-literal=KEYCLOAK_ADMIN_PASSWORD="${kc_admin_pass}" \
        --dry-run=client -o yaml | ${KUBECTL} apply -f -
}
