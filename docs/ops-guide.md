# ScaleX Operations Guide

## 1. Cloudflare Tunnel Setup (Checklist #5, #13)

Cloudflare Tunnel은 GitOps(ArgoCD)로 배포되지만, **Cloudflare Dashboard에서 사전 설정이 필요합니다.**

### 사용자가 직접 수행해야 하는 작업

#### Step 1: Cloudflare Zero Trust 대시보드 접속
1. https://one.dash.cloudflare.com/ 접속
2. 좌측 메뉴 → **Networks** → **Tunnels**

#### Step 2: Tunnel 생성
1. **Create a tunnel** 클릭
2. Tunnel type: **Cloudflared** 선택
3. Tunnel name: `playbox-admin-static` (또는 원하는 이름)
4. **Save tunnel**

#### Step 3: Credentials 저장
1. Tunnel 생성 후 **Install and run a connector** 화면에서 토큰 확인
2. 또는 **API** 탭에서 `credentials file` 다운로드
3. 다운로드한 파일을 `credentials/cloudflare-tunnel.json`으로 저장:
   ```json
   {
     "AccountTag": "<ACCOUNT_ID>",
     "TunnelSecret": "<TUNNEL_SECRET>",
     "TunnelID": "<TUNNEL_ID>"
   }
   ```

#### Step 4: Public Hostname 설정 (Cloudflare Dashboard)
1. 생성한 Tunnel 클릭 → **Public Hostname** 탭
2. 다음 호스트네임 추가:

| Hostname | Service | Description |
|----------|---------|-------------|
| `cd.jinwang.dev` | `http://argocd-server.argocd:80` | ArgoCD UI |
| `auth.jinwang.dev` | `http://keycloak.keycloak:8080` | Keycloak OIDC |
| `api.tower.jinwang.dev` | `https://kubernetes.default:443` | Tower K8s API (외부 kubectl 접근 시 필수) |
| `api.sandbox.jinwang.dev` | `https://192.168.88.110:6443` | Sandbox K8s API (외부 kubectl 접근 시 필수) |

3. 각 호스트네임에서 **TLS** → **No TLS Verify** 활성화 (클러스터 내부 자체서명 인증서)

#### Step 5: DNS 확인
- Cloudflare DNS에 `cd.jinwang.dev`, `auth.jinwang.dev` CNAME이 자동 생성됨
- Tunnel이 정상 연결되면 **Status: Healthy** 표시

#### Step 6: K8s Secret 생성 (CLI가 자동 처리)
`scalex secrets apply`가 자동으로:
```bash
kubectl create secret generic tunnel-credentials \
  --from-file=credentials.json=credentials/cloudflare-tunnel.json \
  -n kube-tunnel --dry-run=client -o yaml | kubectl apply -f -
```

### 완료 후 확인
```bash
kubectl -n kube-tunnel get pods  # cloudflared pod Running 확인
kubectl -n kube-tunnel logs -l app=cloudflared  # 연결 로그 확인
```

---

## 2. Keycloak Setup (Checklist #3)

Keycloak은 Helm chart으로 GitOps 배포되지만, **Realm/Client 설정은 사용자가 직접 수행해야 합니다.**

### 자동으로 배포되는 항목
- Keycloak 서버 (Helm chart via ArgoCD)
- PostgreSQL DB (embedded 또는 external)
- Ingress/Route 설정 (Cloudflare Tunnel 경유)

### 사용자가 직접 수행해야 하는 작업

#### Step 1: Admin 로그인
```bash
# Admin 비밀번호 확인
kubectl -n keycloak get secret keycloak -o jsonpath="{.data.admin-password}" | base64 -d; echo
```
- URL: `https://auth.jinwang.dev` (Cloudflare Tunnel 경유)
- Username: `admin`

#### Step 2: Realm 생성
1. 좌측 상단 드롭다운 → **Create Realm**
2. Realm name: `kubernetes`
3. **Create**

#### Step 3: Client 생성 (OIDC)
1. 좌측 **Clients** → **Create client**
2. Client ID: `kubernetes`
3. Client Protocol: `OpenID Connect`
4. **Next** → Access Type: `confidential`
5. Valid Redirect URIs:
   - `http://localhost:8000/*` (kubectl oidc-login)
   - `https://cd.jinwang.dev/auth/callback` (ArgoCD)
6. **Save**

#### Step 4: Group Mapper 설정
1. Client `kubernetes` → **Client scopes** → `kubernetes-dedicated`
2. **Add mapper** → **By configuration** → **Group Membership**
3. Name: `groups`, Token Claim Name: `groups`
4. Full group path: **OFF**
5. Add to ID token: **ON**, Add to access token: **ON**

#### Step 5: 사용자 및 그룹 생성
1. **Groups** → 생성: `cluster-admin`, `developer`
2. **Users** → 사용자 생성 → **Groups** 탭에서 그룹 할당
3. **Credentials** 탭에서 비밀번호 설정

#### Step 6: ArgoCD OIDC 연동 (이미 GitOps로 배포됨)
- `argocd-cm` ConfigMap에 OIDC 설정이 포함됨
- Client Secret은 `credentials/secrets.yaml`에서 관리

### 검증 체크리스트
- Redirect URI: `http://localhost:8000/*` (kubelogin), `https://cd.jinwang.dev/auth/callback` (ArgoCD)
- Keycloak 그룹 → K8s RBAC 매핑: `cluster-admin` → `ClusterRoleBinding`, `developer` → `RoleBinding`
- ArgoCD RBAC: `argocd-rbac-cm` ConfigMap에서 `policy.csv`로 Keycloak 그룹 매핑

### OIDC kubeconfig 배포 (외부 사용자)
```bash
# 1. kubelogin 설치
kubectl krew install oidc-login

# 2. OIDC kubeconfig 사용
export KUBECONFIG=client/kubeconfig-oidc.yaml
kubectl get nodes  # 브라우저가 열리며 Keycloak 로그인

# 3. 또는 client/setup-client.sh 실행
cd client && ./setup-client.sh
```

---

## 3. Kernel Parameter Tuning (Checklist #11)

커널 파라미터는 `scalex sdi init` 시 자동 적용되는 기본값과, 수동으로 튜닝할 수 있는 고급 설정으로 나뉩니다.

### 자동 적용 (cloud-init / host_prepare)
```bash
# 네트워크 기본 (Kubernetes 필수)
net.ipv4.ip_forward = 1
net.bridge.bridge-nf-call-iptables = 1
net.bridge.bridge-nf-call-ip6tables = 1
```

### 수동 튜닝 가이드

#### 스토리지 최적화
```bash
# /etc/sysctl.d/99-scalex-storage.conf
vm.dirty_ratio = 10
vm.dirty_background_ratio = 5
vm.swappiness = 10
```

#### 네트워크 최적화
```bash
# /etc/sysctl.d/99-scalex-network.conf
net.core.somaxconn = 65535
net.core.netdev_max_backlog = 65535
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.tcp_tw_reuse = 1
net.ipv4.tcp_fin_timeout = 10
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
```

#### IOMMU / GPU Passthrough
```bash
# /etc/default/grub (GRUB_CMDLINE_LINUX_DEFAULT에 추가)
intel_iommu=on iommu=pt
# 적용: update-grub && reboot
```

#### 적용 방법
```bash
# 방법 1: scalex를 통한 원격 적용
scalex facts --host playbox-0  # 현재 커널 파라미터 확인

# 방법 2: SSH로 직접 적용
ssh admin@playbox-0 'sudo sysctl -p /etc/sysctl.d/99-scalex-storage.conf'

# 방법 3: scalex kernel-tune으로 권장 파라미터 확인 및 적용
scalex kernel-tune                        # worker 역할 기본 권장값 출력
scalex kernel-tune --role control-plane   # control-plane 권장값 출력
scalex kernel-tune --format ansible       # Ansible playbook 형식 출력
scalex kernel-tune --diff-node playbox-0  # 현재 값과 권장값 비교

# 방법 4: SSH로 전 노드에 일괄 적용
for node in playbox-{0..3}; do
  ssh admin@$node 'sudo sysctl -p /etc/sysctl.d/99-scalex-network.conf'
done
```

### facts에서 커널 파라미터 확인
```bash
scalex facts --host playbox-0
# _generated/facts/playbox-0.json의 kernel.params 섹션 참조
```

---

## 4. External Access Methods (Checklist #14)

### NAT 망 외부에서 접근 (2가지 방법)

#### 방법 1: Cloudflare Tunnel (OIDC 설정 완료 후에만 kubectl 가능)
- 별도 소프트웨어 설치 불필요
- 브라우저로 직접 접근:
  - ArgoCD: `https://cd.jinwang.dev`
  - Keycloak: `https://auth.jinwang.dev`
- Tower K8s API: `https://api.tower.jinwang.dev` (**OIDC 설정 완료 후**에만 kubectl 가능)
- Sandbox K8s API: `https://api.sandbox.jinwang.dev` (**OIDC 설정 완료 후**에만 kubectl 가능)

> **⚠ CF Tunnel과 client certificate 인증 제한 (중요)**
>
> Cloudflare Tunnel은 HTTP 레이어(L7)에서 동작하며, **TLS를 CF Edge에서 종단(terminate)** 합니다.
> 따라서 kubectl의 client certificate auth가 kube-apiserver에 전달되지 않습니다:
>
> 1. kubectl이 `api.tower.jinwang.dev` 또는 `api.sandbox.jinwang.dev`에 연결 → CF Edge가 TLS 종단
> 2. CF Edge → cloudflared Pod → kube-apiserver로 **새로운** HTTPS 연결 생성
> 3. **원본 client certificate는 이 과정에서 소실됨**
>
> 결론: CF Tunnel 경유 kubectl은 **OIDC token 기반 인증만 가능**합니다.
> client certificate를 사용하는 admin kubeconfig는 CF Tunnel을 통해 동작하지 않습니다.

##### Pre-OIDC kubectl 접근 — Tailscale 사용 (CF Tunnel 불가)

Keycloak OIDC가 아직 설정되지 않은 경우, **CF Tunnel 경유 kubectl은 불가능**합니다.
Pre-OIDC 상태에서 외부 kubectl 접근은 **Tailscale 경유만 가능**합니다:

```bash
# Tailscale 경유 Tower kubectl (client certificate 인증 사용 — 정상 동작)
cp _generated/clusters/tower/kubeconfig.yaml ~/.kube/tower-tailscale.yaml
sed -i 's|server: https://.*:6443|server: https://<TAILSCALE_BASTION_IP>:6443|' ~/.kube/tower-tailscale.yaml
export KUBECONFIG=~/.kube/tower-tailscale.yaml
kubectl get nodes
```

> **참고**: Tailscale은 L3(WireGuard) VPN이므로 TLS가 종단되지 않고 client certificate가
> 그대로 kube-apiserver에 전달됩니다. Pre-OIDC 외부 접근의 유일한 방법입니다.

##### OIDC 설정 완료 후 CF Tunnel kubectl 접근

Keycloak OIDC 설정 완료 후에는 CF Tunnel 경유 kubectl이 가능합니다:

```bash
# OIDC kubeconfig 사용 (token 기반 — CF Tunnel 통과 가능)
export KUBECONFIG=client/kubeconfig-oidc.yaml
kubectl get nodes  # 브라우저가 열리며 Keycloak 로그인
```

#### 접근 경로별 인증 호환성

| 접근 경로 | client certificate | OIDC token | bearer token |
|-----------|-------------------|------------|--------------|
| **LAN 직접** | ✅ | ✅ | ✅ |
| **Tailscale VPN** | ✅ (L3 VPN, TLS 비종단) | ✅ | ✅ |
| **CF Tunnel** | ❌ (TLS 종단으로 인증서 소실) | ✅ | ✅ |

#### 방법 2: Tailscale VPN

##### 설치 및 설정
```bash
# 1. bastion 노드(playbox-0)에 Tailscale 설치
curl -fsSL https://tailscale.com/install.sh | sh

# 2. Tailscale 연결 (브라우저 인증)
sudo tailscale up

# 3. 할당된 Tailscale IP 확인
tailscale ip -4
# 예시 출력: <TAILSCALE_BASTION_IP>
```

##### Tailscale을 통한 SSH 접근
```bash
# bastion 노드에 직접 SSH (Tailscale IP 사용)
ssh jinwang@<TAILSCALE_BASTION_IP>

# bastion 경유로 다른 노드 접근
ssh -J jinwang@<TAILSCALE_BASTION_IP> jinwang@192.168.88.9
```

##### Tailscale을 통한 kubectl 접근
```bash
# kubeconfig의 server를 Tailscale IP로 변경
cp _generated/clusters/tower/kubeconfig.yaml ~/.kube/tower-tailscale.yaml
sed -i 's|server: https://192.168.88.100:6443|server: https://<TAILSCALE_BASTION_IP>:6443|' ~/.kube/tower-tailscale.yaml
export KUBECONFIG=~/.kube/tower-tailscale.yaml
kubectl get nodes
```

#### 접근 방법 비교

| 방법 | 설치 필요 | 속도 | 보안 | 사용 시나리오 |
|------|----------|------|------|-------------|
| **Cloudflare Tunnel** | 없음 | 보통 (CDN 경유) | TLS + CF WAF | 외부 어디서든 kubectl/웹 접근 |
| **Tailscale VPN** | Tailscale 앱 | 빠름 (P2P) | WireGuard 암호화 | SSH + kubectl 직접 접근 |
| **LAN 직접** | 없음 | 가장 빠름 | 물리 네트워크 | 동일 LAN 내 작업 |

### LAN 내부에서 접근

#### 네트워크 스위치 경유 접근
1. 물리적으로 LAN에 연결 (동일 서브넷: `192.168.88.0/24`)
2. 또는 L3 스위치의 관리 포트를 통해 접근

#### 직접 SSH 접근
```bash
# playbox-0 (bastion) 직접 접근
ssh jinwang@192.168.88.8

# playbox-1,2,3 (playbox-0 경유)
ssh -J jinwang@192.168.88.8 jinwang@192.168.88.9
ssh -J jinwang@192.168.88.8 jinwang@192.168.88.10
ssh -J jinwang@192.168.88.8 jinwang@192.168.88.11
```

#### kubectl 직접 접근
```bash
# Tower 클러스터 (관리)
export KUBECONFIG=_generated/clusters/tower/kubeconfig.yaml
kubectl get nodes

# Sandbox 클러스터 (워크로드)
export KUBECONFIG=_generated/clusters/sandbox/kubeconfig.yaml
kubectl get nodes
```

#### 스위치 설정 참고
- 관리 VLAN: 별도 VLAN 구성 시 태그 설정 필요
- 포트 미러링: 네트워크 디버깅 시 스위치의 모니터링 포트 활용
- PoE: 일부 장비 PoE 지원 시 UPS 연결 권장

---

## 5. Sandbox Access Architecture (C-5)

### 설계 결정: Sandbox API는 CF Tunnel에 노출하지 않음

Sandbox 클러스터는 Tower ArgoCD를 통해 GitOps로 관리됩니다.
일반 운영 시 Sandbox에 직접 kubectl 접근할 필요가 없습니다:

- **애플리케이션 배포**: Tower ArgoCD가 Sandbox에 자동 배포 (ApplicationSet)
- **상태 모니터링**: ArgoCD UI (`cd.jinwang.dev`)에서 Sandbox 앱 상태 확인
- **로그/이벤트**: 향후 중앙 모니터링 스택(Grafana/Loki) 배포 예정

### Sandbox API가 CF Tunnel에 없는 이유

1. **보안**: 워크로드 클러스터 API를 외부에 노출하면 공격 표면이 증가
2. **아키텍처**: Tower가 유일한 관리 진입점 — 역할 분리 원칙
3. **간소화**: CF Tunnel 라우팅은 관리 도구(ArgoCD, Keycloak)만 대상

### Sandbox kubectl 접근 방법 (디버깅/긴급 시)

일반 운영에서는 불필요하지만, 디버깅이나 긴급 상황에서 Sandbox에 직접 접근할 수 있습니다.

#### 방법 1: LAN 내부 직접 접근

```bash
# Sandbox kubeconfig 사용 (LAN 내부)
export KUBECONFIG=_generated/clusters/sandbox/kubeconfig.yaml
kubectl get nodes
kubectl get pods -A
```

#### 방법 2: Tailscale 경유 (외부)

```bash
# bastion(playbox-0)에 Tailscale로 SSH 접속 후 Sandbox kubectl 실행
ssh jinwang@<TAILSCALE_BASTION_IP>  # Tailscale IP
export KUBECONFIG=~/ScaleX-POD-mini/_generated/clusters/sandbox/kubeconfig.yaml
kubectl get nodes
```

#### 방법 3: Tower에서 bastion 경유 포트포워딩

```bash
# Tower에서 Sandbox API로 SSH 터널
ssh -L 6444:<sandbox-api-ip>:6443 jinwang@<TAILSCALE_BASTION_IP>
# 별도 터미널에서:
kubectl --server=https://localhost:6444 --insecure-skip-tls-verify get nodes
```

### 향후 확장: Sandbox 외부 접근 (선택적)

필요 시 CF Tunnel에 Sandbox API를 추가할 수 있습니다:

```yaml
# gitops/tower/cloudflared-tunnel/ 설정에 추가
- hostname: "api.sandbox.k8s.jinwang.dev"
  service: "https://<sandbox-api-ip>:6443"
  originRequest:
    noTLSVerify: true
```

> **주의**: 이 경우 Sandbox API가 외부에 노출되므로, OIDC 인증(Keycloak) 설정 완료 후에만 권장합니다.

---

## 6. Kyverno Placement Decision

**결정: `common/` (모든 클러스터에 배포)**

근거:
- 정책 일관성: 보안 정책(pod security, image registry 제한 등)은 모든 클러스터에 동일 적용
- 클러스터별 차이: Kustomize overlay로 클러스터별 예외 처리
- Tower에서도 필요: ArgoCD 자체 보호 정책, namespace 제한 등

```
gitops/
  common/
    kyverno/          # Kyverno 엔진 (모든 클러스터)
    kyverno-policies/ # 공통 정책
  tower/
    kyverno-policies-override/  # Tower 전용 예외 (필요시)
```
