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
3. Tunnel name: `playbox-tunnel` (또는 원하는 이름)
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
| `api.k8s.jinwang.dev` | `https://kubernetes.default:443` | K8s API (선택) |

3. 각 호스트네임에서 **TLS** → **No TLS Verify** 활성화 (클러스터 내부 자체서명 인증서)

#### Step 5: DNS 확인
- Cloudflare DNS에 `cd.jinwang.dev`, `auth.jinwang.dev` CNAME이 자동 생성됨
- Tunnel이 정상 연결되면 **Status: Healthy** 표시

#### Step 6: K8s Secret 생성 (CLI가 자동 처리)
`scalex` 또는 `./playbox bootstrap`가 자동으로:
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

# 방법 3: 전 노드에 일괄 적용 (향후 scalex kernel-tune 명령으로 지원 예정)
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

#### 방법 1: Cloudflare Tunnel (권장)
- 별도 소프트웨어 설치 불필요
- 브라우저로 직접 접근:
  - ArgoCD: `https://cd.jinwang.dev`
  - Keycloak: `https://auth.jinwang.dev`
- K8s API: `https://api.k8s.jinwang.dev` (kubectl 설정 필요)

#### 방법 2: Tailscale VPN
- Tailscale 설치 필요 (bastion 노드에 설치됨)
- bastion IP: `100.64.0.1` (예시, Tailscale 할당 IP)
- SSH 접근: `ssh admin@100.64.0.1`
- kubectl 접근: kubeconfig의 server를 Tailscale IP로 변경

### LAN 내부에서 접근

#### 네트워크 스위치 경유 접근
1. 물리적으로 LAN에 연결 (동일 서브넷: `192.168.88.0/24`)
2. 또는 L3 스위치의 관리 포트를 통해 접근

#### 직접 SSH 접근
```bash
# playbox-0 (bastion) 직접 접근
ssh admin@192.168.88.10

# playbox-1,2,3 (playbox-0 경유)
ssh -J admin@192.168.88.10 admin@192.168.88.11
ssh -J admin@192.168.88.10 admin@192.168.88.12
ssh -J admin@192.168.88.10 admin@192.168.88.13
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

## 5. Kyverno Placement Decision

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
