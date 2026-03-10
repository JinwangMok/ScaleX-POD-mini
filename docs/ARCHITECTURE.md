# Architecture / 아키텍처

---

## 🇰🇷 Korean

## 2-클러스터 설계

**Tower 클러스터** (playbox-0 위 SDI VM, Kubespray K8s):
- 관리 플레인
- 양쪽 클러스터를 관리하는 ArgoCD 실행
- Sandbox 클러스터 리셋 시에도 유지됨
- OpenTofu + libvirt로 생성

**Sandbox 클러스터** (4대 베어메탈 노드에 kubespray 설치):
- 워크로드용 전체 Kubernetes 클러스터
- Cilium CNI (kube-proxy 대체)
- Keycloak을 통한 OIDC 인증
- Cloudflare Tunnel을 통한 외부 접근

## 네트워크

모든 노드는 192.168.88.0/24 LAN에 위치:
- playbox-0: 192.168.88.8 (컨트롤 플레인 + Tower 호스트)
- playbox-1: 192.168.88.9 (워커)
- playbox-2: 192.168.88.10 (워커)
- playbox-3: 192.168.88.11 (워커 + GPU + 10G NIC)
- tower-vm: 192.168.88.100 (playbox-0의 br0 브릿지 위)

### Bond 구성
- 단일 NIC 노드: bond0이 eno1을 래핑 (일관성을 위한 패스스루)
- playbox-0: bond0 + br0 브릿지 (Tower VM L2 네트워킹용)
- playbox-3: bond0에 eno1 + ens2f0np0 (10G 기본, 1G 백업)

## 접근 경로

```
클라이언트 kubectl
  │ server: https://api.k8s.jinwang.dev
  │ exec: kubectl oidc-login → 브라우저 → auth.jinwang.dev
  ▼
Cloudflare Edge (퍼블릭 TLS)
  ▼
Cloudflare Tunnel → cloudflared 파드
  │ api.k8s.jinwang.dev → https://kubernetes.default.svc:443
  │ auth.jinwang.dev → http://keycloak.keycloak.svc:80
  │ cd.jinwang.dev → http://argocd-server.argocd.svc:8080
  ▼
kube-apiserver (OIDC 토큰 검증 → RBAC)
```

kubectl + kubelogin 외에 클라이언트 측 소프트웨어 불필요.

## GitOps 흐름

```
spread.yaml (루트 Application)
  → tower-root / sandbox-root Applications
    → generators/ (ApplicationSets per cluster)
      → common/{app}/, tower/{app}/, sandbox/{app}/에서 Application 생성
```

## Sync Wave 순서
| Wave | 컴포넌트 |
|------|----------|
| 0 | ArgoCD, cluster-config |
| 1 | Cilium, cert-manager, Kyverno, local-path-provisioner |
| 2 | cilium-resources, cert-issuers, kyverno-policies |
| 3 | cloudflared-tunnel, socks5-proxy, keycloak |
| 4 | rbac |

---

## 🇬🇧 English

## Two-Cluster Design

**Tower cluster** (SDI VM on playbox-0, Kubespray K8s):
- Management plane
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
  → tower-root / sandbox-root Applications
    → generators/ (ApplicationSets per cluster)
      → creates Applications from common/{app}/, tower/{app}/, sandbox/{app}/
```

## Sync Waves
| Wave | Components |
|------|-----------|
| 0 | ArgoCD, cluster-config |
| 1 | Cilium, cert-manager, Kyverno, local-path-provisioner |
| 2 | cilium-resources, cert-issuers, kyverno-policies |
| 3 | cloudflared-tunnel, socks5-proxy, keycloak |
| 4 | rbac |
