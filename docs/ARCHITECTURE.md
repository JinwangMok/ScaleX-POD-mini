# Architecture

## Two-Cluster Design

**Tower cluster** (k3s VM on playbox-0):
- Lightweight management plane
- Runs ArgoCD that manages both clusters
- Survives sandbox cluster resets
- Created via OpenTofu + libvirt

**Sandbox cluster** (kubespray on all 4 bare-metal nodes):
- Full Kubernetes cluster for workloads
- Cilium CNI (kube-proxy replacement)
- Keycloak for OIDC authentication
- Cloudflare Tunnel for external access

## Network

All nodes on 192.168.88.0/24 LAN:
- playbox-0: 192.168.88.8 (control plane + tower host)
- playbox-1: 192.168.88.9 (worker)
- playbox-2: 192.168.88.10 (worker)
- playbox-3: 192.168.88.11 (worker + GPU + 10G NIC)
- tower-vm: 192.168.88.100 (on br0 bridge of playbox-0)

### Bond Configuration
- Single-NIC nodes: bond0 wrapping eno1 (pass-through for consistency)
- playbox-0: bond0 + br0 bridge (for tower VM L2 networking)
- playbox-3: bond0 with eno1 + ens2f0np0 (10G primary, 1G backup)

## Access Path

```
Client kubectl
  │ server: https://api.k8s.jinwang.dev
  │ exec: kubectl oidc-login → browser → auth.jinwang.dev
  ▼
Cloudflare Edge (public TLS)
  ▼
Cloudflare Tunnel → cloudflared pod
  │ api.k8s.jinwang.dev → https://kubernetes.default.svc:443
  │ auth.jinwang.dev → http://keycloak.keycloak.svc:80
  │ cd.jinwang.dev → http://argocd-server.argocd.svc:8080
  ▼
kube-apiserver (validates OIDC token → RBAC)
```

No client-side software needed beyond kubectl + kubelogin.

## GitOps Flow

```
spread.yaml (root Application)
  → generators/ (ApplicationSets)
    → reads catalog.yaml
      → creates Applications from apps/<generator>/<app>/
```

## Sync Waves
| Wave | Components |
|------|-----------|
| 0 | ArgoCD, cluster-config |
| 1 | Cilium, cert-manager, local-path-provisioner |
| 2 | cilium-resources |
| 3 | cloudflared-tunnel, socks5-proxy, keycloak |
| 4 | rbac |
