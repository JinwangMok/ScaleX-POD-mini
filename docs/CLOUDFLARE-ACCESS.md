# Cloudflare Access Setup

## Overview

Cloudflare Tunnel provides secure external access without exposing ports. Three services are exposed:

| Domain | Service | Purpose |
|--------|---------|---------|
| api.k8s.jinwang.dev | K8s API | kubectl access |
| auth.jinwang.dev | Keycloak | OIDC authentication |
| cd.jinwang.dev | ArgoCD | GitOps dashboard |

## Setup Steps

### 1. Create Tunnel
```bash
# In Cloudflare Zero Trust dashboard:
# Access → Tunnels → Create a tunnel
# Name: playbox-admin-static
# Download: credentials JSON + cert.pem
```

### 2. DNS Records
Cloudflare automatically creates CNAME records when tunnel is configured.
Verify in DNS dashboard:
- `api.k8s.jinwang.dev` → tunnel CNAME
- `auth.jinwang.dev` → tunnel CNAME
- `cd.jinwang.dev` → tunnel CNAME

### 3. Zero Trust Policies (Optional)
For additional security, add Access policies:
- Allow only specific email domains
- Require MFA for API access
- Bypass for OIDC callback URLs

### 4. TLS
- **Client → Cloudflare**: Cloudflare's edge cert (publicly trusted, automatic)
- **Cloudflare → Origin**: `noTLSVerify: true` for K8s API (self-signed CA). cert-manager handles internal certs.
- **Client kubeconfig**: No `insecure-skip-tls-verify` needed

## Troubleshooting

- Tunnel not connecting: Check `kubectl logs -n kube-tunnel -l app=cloudflared`
- 502 errors: Verify target service is running
- Certificate errors: Ensure Cloudflare SSL mode is "Full" not "Full (Strict)" for tunnel origins
