# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 152 tests pass / clippy 11 warnings / fmt clean

**코드 규모**: ~8,360 lines Rust, 23 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)
**레거시 잔존**: `gitops-apps/`, `gitops-manual/`, `values.yaml`, drawio k3s 참조

---

## 심층 비판적 분석 (2026-03-10, 전체 코드베이스 검증)

### 비판 1: 이전 DASHBOARD "80% CLI 완료" 평가 — 정확하나 불완전

이전 DASHBOARD는 CLI 기능별 체크리스트를 표기했으나, **각 기능의 내부 품질 문제를 간과**:

- `kubespray.rs`의 `generate_cluster_vars()`가 DataX legacy 설정 127개 중 핵심만 생성 — `metrics_server_enabled: false`, `registry_enabled: false` 등 addon 비활성화가 불완전
- Clippy 경고 11개(redundant_closure) 방치 — CI/CD 환경에서 `-D warnings` 사용 시 빌드 실패
- `kube_api_anonymous_auth: true`가 `kubespray_extra_vars`에만 있고 `generate_cluster_vars()` 기본값에 미포함

### 비판 2: 레거시 디렉토리 방치

- `gitops-apps/`: 구 ArgoCD 구조 (Helm chart 기반, Application template). 현재 `gitops/`의 Kustomize + ApplicationSet과 **완전 다른 패러다임**
- `gitops-manual/`: 수동 kubespray 설정 (inventory.ini, addons.yml 등). `scalex cluster init`가 이를 대체
- `values.yaml`: 파일 상단에 DEPRECATED 표기되어 있으나 루트에 그대로 존재
- 이들은 혼란을 유발하며, "어느 것이 진짜 설정인지" 불명확

### 비판 3: DataX Kubespray 설정 대조 결과

`.legacy-datax-kubespray/inventory/datax-cluster-vars.yml` 핵심 비교:

| DataX 설정 | 우리 프로젝트 | 상태 |
|-----------|-------------|------|
| `kube_proxy_remove: true` | `generate_cluster_vars()` | ✅ 반영 |
| `kube_network_plugin: cni` | `generate_cluster_vars()` | ✅ 반영 |
| OIDC 전체 (6개 항목) | `generate_cluster_vars()` | ✅ 반영 |
| `helm_enabled: true` | `generate_cluster_vars()` | ✅ 반영 |
| `gateway_api_enabled: true` | `generate_cluster_vars()` | ✅ 반영 |
| `kubeconfig_localhost: true` | `generate_cluster_vars()` | ✅ 반영 |
| `kubeconfig_localhost_ansible_host: true` | `generate_cluster_vars()` | ✅ 반영 |
| `enable_nodelocaldns: true` | `generate_cluster_vars()` | ✅ 반영 |
| `kube_network_node_prefix: 24` | `generate_cluster_vars()` | ✅ 반영 |
| Admission plugins (NodeRestriction, PodTolerationRestriction) | `generate_cluster_vars()` | ✅ 반영 |
| Addon disablement (cert-manager, argocd, metallb, ingress-nginx, local-path, nfd) | `generate_cluster_vars()` | ✅ 반영 |
| `metrics_server_enabled: false` | `generate_cluster_vars()` | ❌ 누락 |
| `registry_enabled: false` | `generate_cluster_vars()` | ❌ 누락 |
| `kube_api_anonymous_auth: true` | `kubespray_extra_vars`에만 | ⚠️ 기본값 누락 |
| `container_manager: containerd` | `generate_cluster_vars()` | ✅ 반영 |
| `kube_proxy_mode: ipvs` | N/A (kube_proxy_remove=true로 불필요) | ✅ 불필요 확인 |

### 비판 4: 테스트 커버리지 — 양은 많으나 빈틈 존재

152개 테스트가 있으나:
- `generate_cluster_vars()`의 DataX 호환성 테스트가 addon disablement만 검증 — `metrics_server_enabled` 등 누락 addon 미검증
- `gitops-apps/`, `gitops-manual/` 존재에 대한 레거시 감지 테스트 없음
- `values.yaml` deprecated 파일 존재에 대한 테스트 없음

---

## Checklist 재검증 (2026-03-10, 코드 + 테스트 증거 기반)

| # | 질문 | 상태 | 증거 | 미해결 |
|---|------|------|------|--------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **구현됨** | `sdi.rs`: host prep + tofu HCL + pool state; `tofu.rs`: libvirt VM 생성 | — |
| 2 | DataX kubespray 반영 | **95% 반영** | `kubespray.rs`: 핵심 설정 전부 + 테스트 | `metrics_server_enabled`, `registry_enabled` 누락 |
| 3 | Keycloak 설정 | **가이드 완료** | `gitops/tower/keycloak/` + `docs/ops-guide.md` | 사용자 Realm/Client 설정 필요 |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` + sync-wave 3 | — |
| 5 | CF tunnel 완성 | **가이드 완료** | `docs/ops-guide.md` Section 1 | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | `main.rs`: `name = "scalex"` | — |
| 7 | Rust + FP 스타일 | **거의 완료** | 152 tests, 55+ pure functions | **clippy 11 warnings** |
| 8 | CLI 기능 완성도 | **95%** | 모든 subcommand 구현됨 (아래 상세) | addon disablement 보완 필요 |
| 9 | 베어메탈 확장성 / k3s 배제 | **완료** | `ClusterMode::Baremetal`, k3s 코드 제거 완료 | drawio 파일만 수동 수정 필요 |
| 10 | Secrets 구조화 | **완료** | `secrets.rs` + `credentials/*.example` | — |
| 11 | 커널 튜닝 가이드 | **완료** | `docs/ops-guide.md` Section 3 | — |
| 12a | 디렉토리 구조 | **결함** | `scalex-cli/`, `gitops/`, `credentials/`, `config/` 정상 | `gitops-apps/`, `gitops-manual/`, `values.yaml` 레거시 잔존 |
| 12b | 멱등성 | **완료** | 순수 함수, re-runnable | — |
| 13 | CF tunnel 가이드 | **완료** | `docs/ops-guide.md` Section 1 | — |
| 14 | 외부 접근 가이드 | **완료** | `docs/ops-guide.md` Section 4 | — |

### Checklist #8 CLI 기능 상세

| 명령어 | 구현 | 테스트 | 미해결 |
|--------|------|--------|--------|
| `scalex facts` | ✅ SSH gathering + 전체 HW 파싱 | ✅ 4개 | — |
| `scalex sdi init` (no flag) | ✅ host prep + resource pool + tofu | ✅ tofu 테스트 | — |
| `scalex sdi init <spec>` | ✅ VM HCL + pool state | ✅ tofu/sdi 테스트 | — |
| `scalex sdi clean --hard` | ✅ tofu destroy + node cleanup | ✅ | — |
| `scalex sdi sync` | ✅ diff + VM conflict detection | ✅ 3개 | — |
| `scalex cluster init` | ✅ inventory + vars + kubespray + kubeconfig | ✅ 7개 | addon vars 보완 |
| `scalex get baremetals` | ✅ table output | ✅ 3개 | — |
| `scalex get sdi-pools` | ✅ pool state table | ✅ 3개 | — |
| `scalex get clusters` | ✅ cluster table | ✅ 2개 | — |
| `scalex get config-files` | ✅ validation table | ✅ 6개 | — |
| `scalex secrets apply` | ✅ K8s secret manifest 생성 | ✅ | — |

---

## 결함 목록 (우선순위순)

### DEFECT-1: Clippy 11 warnings (redundant_closure) — OPEN
**영향**: CI 환경 빌드 실패 가능, 코드 품질 저하
**수정**: `cargo clippy --fix`

### DEFECT-2: DataX kubespray addon 비활성화 불완전 — OPEN
**영향**: Kubespray가 metrics-server, registry를 배포하여 ArgoCD 관리 리소스와 충돌 가능
**수정**: `kubespray.rs`의 `generate_cluster_vars()`에 누락 addon 추가 + 테스트

### DEFECT-3: 레거시 디렉토리 잔존 — OPEN
**영향**: 프로젝트 구조 혼란, 새 기여자가 잘못된 설정 참조 가능
**수정**: `.legacy-` prefix로 이동 + 레거시 감지 테스트 추가

### DEFECT-4: kube_api_anonymous_auth 기본값 누락 — OPEN
**영향**: kubespray_extra_vars에 명시하지 않으면 기본 false → API 접근 문제
**수정**: `generate_cluster_vars()`에 `kube_api_anonymous_auth: true` 기본 추가

---

## 실행 계획 — TDD, 최소 핵심 단위

### Unit 1: Clippy 경고 수정 (DEFECT-1)

**1-1 RED**: `cargo clippy -- -D warnings` 실행 → 실패 확인
**1-2 GREEN**: `cargo clippy --fix` 적용
**1-3 VERIFY**: `cargo clippy -- -D warnings` + `cargo test` 통과 확인

### Unit 2: DataX Kubespray Addon 비활성화 보완 (DEFECT-2, DEFECT-4)

**2-1 RED**: `test_cluster_vars_all_datax_addon_disablements` — `metrics_server_enabled: false`, `registry_enabled: false`, `kube_api_anonymous_auth: true` 검증 → 실패
**2-2 GREEN**: `kubespray.rs`에 누락 addon 비활성화 + `kube_api_anonymous_auth` 추가
**2-3 REFACTOR**: 기존 152 테스트 + 새 테스트 전부 통과 확인

### Unit 3: 레거시 디렉토리 정리 (DEFECT-3)

**3-1 RED**: `test_no_legacy_gitops_directories` — `gitops-apps/`, `gitops-manual/` 존재 감지 + `values.yaml` deprecated 감지 → 실패
**3-2 GREEN**: 레거시 디렉토리 `.legacy-` prefix 이동 + `values.yaml` 삭제
**3-3 VERIFY**: 테스트 통과 확인

### Unit 4: QA 최종 검증

**4-1**: `cargo test` — 전체 테스트 통과
**4-2**: `cargo clippy -- -D warnings` — 경고 0
**4-3**: `cargo fmt --check` — 포맷 정상

---

## 사용자 수동 작업 (코드로 해결 불가)

1. **Cloudflare Tunnel WebUI 설정** → `docs/ops-guide.md` Section 1 가이드 참조
2. **Keycloak Realm/Client 설정** → `docs/ops-guide.md` Section 2 가이드 참조
3. **credentials/ 실제 파일 작성** (.baremetal-init.yaml, .env, secrets.yaml)
4. **config/ 실제 파일 작성** (sdi-specs.yaml, k8s-clusters.yaml)
5. **GitOps repo URL 확인**: 모든 gitops YAML이 `k8s-playbox.git` 참조 — 실제 레포명과 일치 여부
6. **drawio 파일 수동 수정**: `docs/architecture-overview.drawio`, `docs/provisioning-flow.drawio`에서 k3s 참조 제거

---

## Kyverno 배치 결정: **Common** (확정)

모든 클러스터에 일관된 보안/운영 정책. `gitops/common/kyverno/` 위치. 클러스터별 예외는 PolicyException으로.

---

## 아키텍처 요약

```
credentials/                    config/
.baremetal-init.yaml           sdi-specs.yaml
.env                           k8s-clusters.yaml
secrets.yaml
        |                           |
        v                           v
+-------------------------------------------+
|              scalex CLI (Rust)             |
|  facts -> sdi init -> cluster init        |
|  get baremetals/sdi-pools/clusters        |
|  secrets apply                            |
+-------------------------------------------+
        |
        v
_generated/
+-- facts/          (hardware JSON)
+-- sdi/            (OpenTofu HCL + state)
+-- clusters/       (inventory.ini + vars)
        |
        v
+-------------------------------------------+
|           gitops/ (ArgoCD)                |
|  bootstrap/spread.yaml                    |
|  generators/{tower,sandbox}/              |
|  common/ tower/ sandbox/                  |
+-------------------------------------------+
```
