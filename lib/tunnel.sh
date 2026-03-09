#!/usr/bin/env bash
# lib/tunnel.sh — Cloudflare tunnel secret management

tunnel_create_secret() {
    log_step "Setting up Cloudflare tunnel..."

    local cf_creds cf_cert
    cf_creds=$(yq_read '.cloudflare.credentials_file')
    cf_cert=$(yq_read '.cloudflare.cert_file')

    if [[ -z "${cf_creds}" || "${cf_creds}" == "null" || "${cf_creds}" == "" ]]; then
        log_warn "Cloudflare credentials_file not set in values.yaml — skipping tunnel setup"
        log_warn "Run 'playbox bootstrap --tunnel-only' after setting credentials"
        return 0
    fi

    if [[ ! -f "${cf_creds}" ]]; then
        log_error "Cloudflare credentials file not found: ${cf_creds}"
        return 1
    fi

    if [[ "${DRY_RUN}" == "true" ]]; then
        log_info "[dry-run] Would create CF tunnel secret from ${cf_creds}"
        return 0
    fi

    ensure_namespace "kube-tunnel"

    ${KUBECTL} -n kube-tunnel create secret generic cloudflared-tunnel-credentials \
        --from-file=credentials.json="${cf_creds}" \
        --dry-run=client -o yaml | ${KUBECTL} apply -f -

    if [[ -n "${cf_cert}" && "${cf_cert}" != "null" && -f "${cf_cert}" ]]; then
        ${KUBECTL} -n kube-tunnel create secret generic cloudflared-tunnel-cert \
            --from-file=cert.pem="${cf_cert}" \
            --dry-run=client -o yaml | ${KUBECTL} apply -f -
    fi

    log_info "Cloudflare tunnel secrets created"
}
