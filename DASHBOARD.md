# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 130 tests pass / clippy clean / fmt clean

**코드 규모**: ~7,600 lines Rust, 22 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)

---

## 이전 DASHBOARD.md 비판적 분석

이전 DASHBOARD.md는 대부분 항목을 "완료"로 표기하고 Sprint로 나눈 향후 계획을 제시했으나, 다음과 같은 **근본적 문제**를 가지고 있었다:

### 1. "코드 완료"와 "검증 완료"의 혼동
- 대부분의 항목에 "코드 완료" 또는 "완료"로 표기했으나, **실제 동작을 검증한 테스트가 없는 영역**이 다수 존재
- `scalex get sdi-pools`: 0개 테스트, `scalex get clusters`: 0개 테스트, `scalex sdi clean`: 0개 테스트
- 순수 함수가 존재한다는 것과, 해당 함수가 요구사항을 만족한다는 것은 별개

### 2. 아키텍처적 결함 미발견
- **Cilium `k8sServiceHost` 하드코딩 (`192.168.88.8`)**: `gitops/common/cilium/values.yaml`에 tower control-plane IP가 하드코딩되어 있어 **sandbox 클러스터에서는 Cilium이 정상 동작하지 않음**. common/ 디렉토리가 양쪽 클러스터에 배포되므로 치명적 오류.
- 이 문제는 "Cilium k8sServiceHost 하드코딩" 항목으로 언급만 되고 해결되지 않음

### 3. Sprint 계획의 비현실성
- Sprint 1~4로 분류했으나 실제 구현 우선순위와 의존관계가 불명확
- "예상 결과: +N tests"로 정량화했으나, 실제 미해결 문제의 심각도를 반영하지 못함
- `node_feature_discovery_enabled: false` 누락을 표로 보여주면서도 Sprint에 반영하지 않음

### 4. k3s 잔존 참조 과소평가
- 8개 파일에 k3s 참조가 있음에도 Sprint 4(마지막)로 미룸
- Checklist #9에서 "k3s 배제"가 핵심 요구사항인데 우선순위 불일치

---

## Checklist 재검증 — 정직한 상태 (2026-03-10)

| # | 질문 | 상태 | 미해결 사항 |
|---|------|------|------------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **코드 완료** | `sdi init` (no flag) vs `sdi init <spec>` 차별화 검증 필요 |
| 2 | DataX kubespray 반영 | **90%** | `node_feature_discovery_enabled: false` 누락 |
| 3 | Keycloak 설정 | **가이드 완료** | 사용자 수동 작업 필요 (Realm/Client/Group) |
| 4 | CF tunnel GitOps 배포 | **완료** | — |
| 5 | CF tunnel 완성 | **가이드 완료** | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | — |
| 7 | Rust + FP 스타일 | **완료** | 130 tests, 55+ pure functions |
| 8 | CLI 기능 완성도 | **90%** | `get sdi-pools`/`get clusters` 테스트 0개, secrets apply 명령 없음 |
| 9 | 베어메탈 확장성 / k3s 배제 | **미완성** | **8개 파일에 k3s 참조 잔존** |
| 10 | Secrets 구조화 | **코드 완료** | secrets.rs 순수 함수 완료, CLI apply 미구현 |
| 11 | 커널 튜닝 가이드 | **완료** | docs/ops-guide.md Section 3 |
| 12a | 디렉토리 구조 | **완료** | Kyverno → common/ 결정 |
| 12b | 멱등성 | **완료** | 순수 함수 기반 |
| 13 | CF tunnel 가이드 | **완료** | docs/ops-guide.md Section 1 |
| 14 | 외부 접근 가이드 | **완료** | docs/ops-guide.md Section 4 |

---

## 치명적 결함 (Critical Defects)

### CRITICAL-1: Cilium k8sServiceHost 하드코딩

**파일**: `gitops/common/cilium/values.yaml`
```yaml
k8sServiceHost: "192.168.88.8"  # ← tower CP IP 하드코딩
k8sServicePort: 6443
```

**문제**: `common/cilium/`은 ApplicationSet으로 tower와 sandbox 모두에 배포됨. sandbox의 control-plane IP는 다르므로 **sandbox Cilium이 API 서버에 연결 실패**.

**해결 방안**:
- Cilium을 common/에서 분리하여 `tower/cilium/`, `sandbox/cilium/` 각각으로 이동
- 또는 Kustomize overlay로 클러스터별 k8sServiceHost 오버라이드
- `scalex cluster init`이 클러스터별 Cilium values를 업데이트하는 순수 함수 추가

### CRITICAL-2: k3s 잔존 참조 (8개 파일)

Checklist #9 "k3s 배제"에 직접 위배:
- `README.md` — k3s tower 설명
- `CLAUDE.md` — "k3s for tower which is being replaced"
- `values.yaml` — k3s 설정
- `lib/cluster.sh` — k3s 설치/제거 함수
- `tests/fixtures/values-full.yaml`, `tests/fixtures/values-minimal.yaml` — k3s 참조
- `PROMPT.md` — k3s 참조

---

## 미달성 항목 근본 원인 분석

| 미달성 항목 | 근본 원인 |
|------------|----------|
| Cilium 하드코딩 | multi-cluster에서 common/ 앱의 클러스터별 값 차이를 고려하지 않은 초기 설계 |
| k3s 잔존 | 레거시 코드와 신규 코드가 공존하며 정리 시점을 놓침 |
| `node_feature_discovery_enabled` 누락 | DataX 설정 비교 시 addon 목록이 불완전 |
| get 명령 테스트 부재 | IO 의존 명령의 순수 함수 분리가 불완전 |
| secrets apply 미구현 | 순수 함수(generate)는 완료, CLI 서브커맨드(apply)는 미연결 |

---

## 실행 계획 — TDD 방식, 최소 핵심 단위

### Unit 1: Cilium 멀티클러스터 분리 (CRITICAL-1 해결)
> Cilium values를 클러스터별로 분리하고 `scalex cluster init`이 k8sServiceHost를 자동 설정

**1-1**: `generate_cilium_values()` 순수 함수 추가 (TDD)
- 입력: cluster control-plane IP, 기본 Cilium config
- 출력: 클러스터별 Cilium values.yaml 문자열
- 테스트: tower IP → values에 tower IP, sandbox IP → values에 sandbox IP

**1-2**: GitOps 구조 변경
- `gitops/common/cilium/` → 기본 values 유지 (k8sServiceHost 제거)
- `gitops/tower/cilium/`, `gitops/sandbox/cilium/` 각각에 Kustomize overlay 추가
- 또는 각 generator에서 직접 참조

**1-3**: generator YAML 업데이트 및 테스트
- tower/sandbox 각각의 generator에서 클러스터별 cilium 참조
- 기존 gitops YAML 파싱 테스트 업데이트

### Unit 2: Kubespray addon 완전 비활성화 (Checklist #2 해결)
> `node_feature_discovery_enabled: false` 추가

**2-1**: `generate_cluster_vars()`에 누락 addon 추가 (TDD)
- RED: `node_feature_discovery_enabled: false` 포함 여부 테스트
- GREEN: 코드에 추가
- REFACTOR: 기존 addon 비활성화 섹션과 통합

### Unit 3: k3s 잔존 참조 정리 (CRITICAL-2 해결)
> 비-레거시 파일에서 k3s 참조 완전 제거

**3-1**: README.md, CLAUDE.md 업데이트
**3-2**: values.yaml → deprecated 파일 제거 또는 이동
**3-3**: lib/cluster.sh k3s 함수 제거 (레거시 → .legacy-* 이동 검토)
**3-4**: 테스트 fixture 정리 (values-full/minimal.yaml k3s 참조 제거)

### Unit 4: `get` 명령 테스트 보강
> sdi-pools, clusters 포맷팅 순수 함수에 테스트 추가

**4-1**: `get sdi-pools` 출력 포맷팅 테스트 (TDD)
- pool state → table 문자열 변환 순수 함수 검증
**4-2**: `get clusters` 출력 포맷팅 테스트 (TDD)
- cluster inventory → table 문자열 변환 순수 함수 검증

### Unit 5: Secrets CLI apply 연결
> `scalex secrets apply` 서브커맨드 추가

**5-1**: `secrets apply` 명령 구현 — secrets.rs 순수 함수를 CLI에 연결
- `credentials/secrets.yaml` 파싱 → K8s Secret YAML 생성 → kubectl apply

---

## Checklist #2 상세 — DataX Kubespray 설정 비교

| DataX 설정 | 값 | 현재 반영 | 비고 |
|-----------|-----|----------|------|
| kube_proxy_mode | ipvs | **개선됨** → `kube_proxy_remove: true` | Cilium 대체 |
| kube_network_plugin | cni | ✅ | Cilium용 |
| container_manager | containerd | ✅ | |
| enable_nodelocaldns | true | ✅ | |
| helm_enabled | true | ✅ | |
| gateway_api_enabled | true | ✅ | |
| kube_network_node_prefix | 24 | ✅ | |
| ntp_enabled | true | ✅ | |
| cert_manager_enabled | false | ✅ | |
| argocd_enabled | false | ✅ | |
| metallb_enabled | false | ✅ | |
| ingress_nginx_enabled | false | ✅ | |
| local_path_provisioner_enabled | false | ✅ | |
| node_feature_discovery_enabled | false | **누락** | Unit 2에서 해결 |

---

## Checklist #8 CLI 기능 상세

| 기능 | 구현 | 테스트 | 미해결 |
|------|------|--------|--------|
| `scalex facts` | ✅ | 2 | — |
| `scalex sdi init` (no flag) | ✅ | 11 | — |
| `scalex sdi init <spec>` | ✅ | 5 | — |
| `scalex sdi clean --hard` | ✅ | 0 | IO 전용 (테스트 불필요) |
| `scalex sdi sync` | ✅ | 7 | — |
| `scalex cluster init` | ✅ | 25 | — |
| `scalex get baremetals` | ✅ | 3 | — |
| `scalex get sdi-pools` | ✅ | **0** | Unit 4에서 추가 |
| `scalex get clusters` | ✅ | **0** | Unit 4에서 추가 |
| `scalex get config-files` | ✅ | 6 | — |
| `scalex secrets apply` | **미구현** | 0 | Unit 5에서 추가 |

---

## GitOps 앱 현황

### Tower 클러스터 (관리)
| App | Sync Wave | 상태 | 비고 |
|-----|-----------|------|------|
| cluster-config | 0 | ✅ | ConfigMap |
| argocd | 0 | ✅ | Helm 8.1.1 |
| cilium | 1 | **CRITICAL** | k8sServiceHost 하드코딩 → Unit 1 |
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
| cilium | 1 | **CRITICAL** | 잘못된 k8sServiceHost → Unit 1 |
| cert-manager | 1 | ✅ | |
| kyverno | 1 | ✅ | |
| local-path-provisioner | 1 | ✅ | |
| cilium-resources | 2 | ✅ | |
| rbac | 4 | ✅ | OIDC ClusterRoleBindings |

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

---

## 테스트 분포 현황 (130 tests)

| 모듈 | 파일 | 테스트 수 |
|------|------|-----------|
| core | kubespray.rs | 17 |
| core | gitops.rs | 18 |
| core | tofu.rs | 8 |
| core | host_prepare.rs | 10 |
| core | validation.rs | 9 |
| core | sync.rs | 7 |
| core | config.rs | 7 |
| core | resource_pool.rs | 5 |
| core | secrets.rs | 9 |
| core | ssh.rs | 2 |
| commands | get.rs | 9 |
| commands | cluster.rs | 7 |
| commands | sdi.rs | 8 |
| commands | facts.rs | 2 |
| models | cluster.rs | 5 |
| models | sdi.rs | 2 |
| **합계** | | **130** |
