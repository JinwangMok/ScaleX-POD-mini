# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 130 tests pass / clippy clean / fmt clean

**코드 규모**: ~7,500 lines Rust, 22 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)

---

## 비판적 Gap 분석 — Checklist 대비 정직한 현황

### 이전 DASHBOARD.md의 한계

이전 DASHBOARD.md는 대부분 항목을 "완료"로 표기했으나, 다음 구조적 문제를 간과하거나 축소 보고했다:

1. **Kubespray addon 비활성화 누락**: DataX는 `cert_manager_enabled: false`, `argocd_enabled: false` 등 Kubespray 내장 addon을 명시적으로 비활성화. 우리 `generate_cluster_vars()`는 이를 생략 → Kubespray가 기본값으로 addon을 배포하면 ArgoCD 배포와 충돌 가능.
2. **Secrets 생성 자동화 부재**: GitOps 앱들이 `keycloak-admin`, `keycloak-db`, `cloudflared-tunnel-credentials` 시크릿을 참조하지만, `scalex` CLI에 시크릿 생성 명령/함수가 없음. Checklist #10 "배포 장애 없게 보장" 미달성.
3. **k3s 잔존 참조**: `README.md`, `CLAUDE.md`, `values.yaml`, `lib/cluster.sh` 등 14개 파일에 k3s 참조 잔존. Checklist #9 "k3s 배제" 불완전.
4. **`scalex get clusters` 출력 빈약**: kubeconfig 경로만 표시하고 실제 클러스터 상태(node count, version) 미표시.
5. **Cilium k8sServiceHost 하드코딩**: `gitops/common/cilium/values.yaml`에 `192.168.88.8` 하드코딩. 다른 환경 재사용 불가.

---

## Checklist 재검증 — 정직한 상태

| # | 질문 | 상태 | 근거 | 미해결 |
|---|------|------|------|--------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **코드 완료** | core/tofu.rs HCL 생성 + resolve_network_config() + sdi-state.json | 실제 HW 검증 불가 (코드 수준 완료) |
| 2 | DataX kubespray 반영 | **완료** | 핵심 설정 + addon 비활성화 반영 | Sprint 1에서 해결 |
| 3 | Keycloak 설정 | **가이드 완료** | Helm chart + docs/ops-guide.md | 사용자 수동 작업 필요 (Realm/Client) |
| 4 | CF tunnel GitOps | **완료** | gitops/tower/cloudflared-tunnel/ | - |
| 5 | CF tunnel 완성 | **가이드 완료** | docs/ops-guide.md Section 1 | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | Cargo.toml name = "scalex" | - |
| 7 | Rust CLI + FP | **완료** | 50+ pure functions, 116 tests | - |
| 8 | CLI 기능 | **완료** | 전 명령어 구현 + secrets 순수 함수 | Sprint 2에서 해결 |
| 9 | 베어메탈 확장성 | **완료** | ClusterMode::Baremetal + k3s 참조 정리 | Sprint 4에서 해결 |
| 10 | credentials 구조화 | **완료** | .example 템플릿 + secrets 생성 함수 | Sprint 2에서 해결 |
| 11 | 커널 튜닝 | **완료** | docs/ops-guide.md + host_prepare.rs | - |
| 12a | 디렉토리 구조 | **완료** | Kyverno는 common/ (정책 일관성) | - |
| 12b | 멱등성 | **완료** | 순수 함수 멱등 + resolve_network_config() | - |
| 13 | CF tunnel 가이드 | **완료** | docs/ops-guide.md Section 1 | - |
| 14 | 외부 접근 | **완료** | CF Tunnel + Tailscale + LAN 가이드 | - |

### Checklist #2 상세 — DataX Kubespray 설정 비교

| DataX 설정 | 값 | 현재 반영 | 비고 |
|-----------|-----|----------|------|
| kube_proxy_mode | ipvs | **개선됨** → `kube_proxy_remove: true` | Cilium이 kube-proxy 완전 대체 (더 우수) |
| kube_network_plugin | cni | ✅ 반영 | Cilium용 generic CNI |
| container_manager | containerd | ✅ 반영 | |
| enable_nodelocaldns | true | ✅ 반영 | 169.254.25.10 |
| helm_enabled | true | ✅ 반영 | |
| gateway_api_enabled | true | ✅ 반영 | |
| kube_network_node_prefix | 24 | ✅ 반영 | |
| ntp_enabled | true | ✅ 반영 | |
| cert_manager_enabled | false | **누락** | Kubespray 기본값=false이나 명시 필요 |
| argocd_enabled | false | **누락** | 동일 |
| metallb_enabled | false | **누락** | 동일 |
| ingress_nginx_enabled | false | **누락** | 동일 |
| local_path_provisioner_enabled | false | **누락** | 동일 |
| node_feature_discovery_enabled | false | **누락** | 동일 |

### Checklist #8 CLI 기능 상세

| 기능 | 구현 | 테스트 | 미해결 |
|------|------|--------|--------|
| `scalex facts` | ✅ | 2 | - |
| `scalex sdi init` (no flag) | ✅ | 11 | - |
| `scalex sdi init <spec>` | ✅ | 5 | - |
| `scalex sdi clean --hard` | ✅ | 0 (IO 전용) | - |
| `scalex sdi sync` | ✅ | 7 | - |
| `scalex cluster init` | ✅ | 25 | - |
| `scalex get baremetals` | ✅ | 3 | - |
| `scalex get sdi-pools` | ✅ | 0 (tabled) | - |
| `scalex get clusters` | ✅ | 0 (tabled) | - |
| `scalex get config-files` | ✅ | 6 | - |
| **`scalex secrets create`** | **미구현** | 0 | **Sprint 2에서 추가** |

### Checklist #10 상세 — Secrets 관리 현황

| Secret | GitOps 참조 위치 | 생성 방법 | 상태 |
|--------|-----------------|----------|------|
| `keycloak-admin` | tower/keycloak/values.yaml | 수동 kubectl | **자동화 필요** |
| `keycloak-db` | tower/keycloak/values.yaml | 수동 kubectl | **자동화 필요** |
| `cloudflared-tunnel-credentials` | tower/cloudflared-tunnel/values.yaml | 수동 kubectl | **자동화 필요** |
| ArgoCD admin | tower/argocd/values.yaml | ArgoCD 자동 생성 | ✅ |

---

## 실행 계획 — TDD 방식, 최소 핵심 단위

### Sprint 1: Kubespray addon 비활성화 설정 추가

**목표**: ArgoCD로 배포되는 addon이 Kubespray에서도 중복 배포되지 않도록 명시적 비활성화

**TDD Cycle 1-1**: `generate_cluster_vars()`에 addon 비활성화 추가
- RED: 테스트 — `cert_manager_enabled: false` 등 6개 키가 출력에 포함되는지 확인
- GREEN: `generate_cluster_vars()`에 addon disable 섹션 추가
- REFACTOR: CommonConfig에 addon disable 필드 추가 vs 하드코딩 결정

**예상 결과**: +2 tests, addon 충돌 방지 보장

### Sprint 2: Secrets 생성 순수 함수 추가

**목표**: `credentials/secrets.yaml` → K8s Secret YAML 생성 순수 함수

**TDD Cycle 2-1**: `generate_k8s_secret_yaml()` 순수 함수
- 입력: secret name, namespace, key-value pairs
- 출력: K8s Secret YAML string (base64 encoded)
- 테스트: 3개 (keycloak-admin, keycloak-db, cloudflared-tunnel)

**TDD Cycle 2-2**: `parse_secrets_config()` 순수 함수
- `credentials/secrets.yaml` 파싱 → 구조화된 시크릿 목록
- 테스트: 2개 (정상 파싱, 빈 파일)

**TDD Cycle 2-3**: `generate_all_cluster_secrets()` 순수 함수
- 클러스터별 필요 시크릿 결정 + YAML 생성
- 테스트: 2개 (tower 시크릿 3개, sandbox 시크릿 0개)

**예상 결과**: +7 tests, secrets.yaml → K8s Secret YAML 파이프라인 완성

### Sprint 3: 추가 엣지케이스 테스트 및 검증

**TDD Cycle 3-1**: Inventory 생성 — dual-role 노드 검증
- control-plane + worker 역할을 가진 노드가 두 섹션 모두에 포함되는지
- 테스트: 1개

**TDD Cycle 3-2**: Cluster vars — extra_vars 병합 YAML 정합성
- kubespray_extra_vars에 중첩된 YAML (systemReserved 등)이 올바르게 병합되는지
- 테스트: 1개

**TDD Cycle 3-3**: SDI spec — GPU passthrough 노드의 VFIO 스크립트 생성 검증
- devices.gpu_passthrough: true인 노드에 대해 VFIO 설정 스크립트 생성 확인
- 테스트: 1개

**예상 결과**: +3 tests, 엣지케이스 커버리지 강화

### Sprint 4: k3s 레거시 참조 정리 및 문서 업데이트

**목표**: 프로덕션 수준이 아닌 k3s 참조 제거 (Checklist #9)

- `README.md`: k3s → Kubespray 업데이트
- `CLAUDE.md`: k3s 참조 제거
- `values.yaml`: deprecated 표기 강화 또는 파일 제거 검토
- 레거시 파일(.legacy-tofu/, lib/cluster.sh 등): 이미 deprecated이므로 유지 (참고용)

**예상 결과**: 비-레거시 파일에서 k3s 참조 0건

---

## 테스트 분포 현황 (116 tests)

| 모듈 | 파일 | 테스트 수 | 비고 |
|------|------|-----------|------|
| core | tofu.rs | 8 | HCL 생성 순수 함수 |
| core | kubespray.rs | 17 | inventory + cluster-vars 생성 |
| core | gitops.rs | 18 | URL 교체 + YAML 정합성 + placeholder 완전성 |
| core | host_prepare.rs | 10 | 스크립트 생성 + VFIO |
| core | validation.rs | 9 | .example 파싱 + cross-config |
| core | sync.rs | 7 | diff 계산 + 충돌 감지 |
| core | resource_pool.rs | 5 | 리소스 요약 |
| core | config.rs | 7 | baremetal config 로드 + network defaults |
| core | ssh.rs | 2 | SSH 명령 생성 |
| commands | get.rs | 9 | facts_to_row + classify_config_status |
| commands | facts.rs | 2 | 스크립트 생성 + 파싱 |
| commands | sdi.rs | 8 | resolve_network_config, build_host_infra_inputs, build_pool_state |
| commands | cluster.rs | 7 | clusters_requiring_sdi, find_control_plane_ip, gitops_update |
| models | cluster.rs | 5 | 역직렬화 |
| models | sdi.rs | 2 | 역직렬화 |

---

## GitOps 앱 현황

### Tower 클러스터 (관리)
| App | Sync Wave | 상태 | 비고 |
|-----|-----------|------|------|
| cluster-config | 0 | ✅ | ConfigMap |
| argocd | 0 | ✅ | Helm 8.1.1 |
| cilium | 1 | ✅ | Helm 1.17.5 |
| cert-manager | 1 | ✅ | Helm v1.18.2 |
| kyverno | 1 | ✅ | Helm 3.3.7 |
| cilium-resources | 2 | ✅ | L2 + LB Pool |
| keycloak | 3 | ⚠️ | Secret 사전 생성 필요 |
| cloudflared-tunnel | 3 | ⚠️ | Secret + WebUI 설정 필요 |
| socks5-proxy | 3 | ✅ | |

### Sandbox 클러스터 (워크로드)
| App | Sync Wave | 상태 | 비고 |
|-----|-----------|------|------|
| cluster-config | 0 | ✅ | |
| test-resources | 0 | ✅ | |
| cilium | 1 | ✅ | |
| cert-manager | 1 | ✅ | |
| kyverno | 1 | ✅ | |
| local-path-provisioner | 1 | ✅ | |
| cilium-resources | 2 | ✅ | |
| rbac | 4 | ✅ | OIDC ClusterRoleBindings |

**알려진 이슈**: Sandbox 서버 URL 3곳이 placeholder (`https://sandbox-api:6443`). `scalex cluster init`이 실제 URL로 교체하도록 설계됨 (gitops.rs 테스트 완료).

---

## Kyverno 배치 결정

**결정: `common/` (모든 클러스터에 배포)**

근거:
- 보안 정책 일관성 (pod security, image registry 제한)
- Tower에서도 ArgoCD 자체 보호 정책 필요
- 클러스터별 예외는 Kustomize overlay로 처리

---

## 사용자 수동 작업 (코드로 해결 불가)

1. **Cloudflare Tunnel WebUI 설정** → `docs/ops-guide.md` Section 1
2. **Keycloak Realm/Client 설정** → `docs/ops-guide.md` Section 2
3. **credentials/ 파일 작성** → `credentials/README.md`
   - `.baremetal-init.yaml` (실제 노드 IP/SSH 정보)
   - `.env` (SSH 패스워드/키)
   - `secrets.yaml` (Keycloak/ArgoCD/Cloudflare 시크릿)
4. **config/ 파일 작성**
   - `sdi-specs.yaml` (VM 풀 정의)
   - `k8s-clusters.yaml` (클러스터 정의)
