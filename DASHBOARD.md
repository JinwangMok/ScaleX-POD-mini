# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Gap Analysis: 이전 DASHBOARD 비판

이전 DASHBOARD는 모든 Phase를 "완료"로 표시했으나, 실제 검증 결과 **다수의 심각한 불일치**가 발견됨:

### 치명적 문제 (Critical)

| # | 문제 | 영향 |
|---|------|------|
| **C1** | `spread.yaml`이 OLD 경로(`gitops/clusters/playbox/generators`) 참조 → NEW 구조(`gitops/generators/tower/`, `gitops/generators/sandbox/`)와 단절 | ArgoCD 부트스트랩 시 NEW multi-cluster 구조가 적용되지 않음 |
| **C2** | `tofu/main.tf`가 k3s 단일 VM 하드코딩 — scalex-cli가 생성하는 멀티호스트 HCL과 완전히 별개 | Tower 클러스터가 k3s(비-프로덕션)로 동작, Checklist #9 위반 |
| **C3** | `gitops/clusters/playbox/` (OLD 단일클러스터 구조)와 `gitops/{common,tower,sandbox,generators}/` (NEW 멀티클러스터 구조) 공존 | 어느 쪽이 실제 적용되는지 불명확, 배포 혼란 |
| **C4** | Kyverno가 `gitops/common/kyverno/`에 존재하지만 tower/sandbox common-generator에 미포함 | 배포되지 않음 |

### 중요 문제 (Major)

| # | 문제 | 영향 |
|---|------|------|
| **M1** | Sandbox server URL이 `https://sandbox-api:6443` 플레이스홀더 — 자동화 없음 | `scalex cluster init` 후 수동 교체 필요, 멱등성 깨짐 |
| **M2** | `kubespray.rs` cluster-vars에 DataX 핵심 설정 누락: `firewalld_enabled`, `kubelet_custom_flags`, Gateway API, graceful shutdown | Checklist #2 미달성 |
| **M3** | 레포 URL이 `k8s-playbox.git`으로 하드코딩 — 실제 레포명(`playbox-provisioning`)과 불일치 가능 | GitOps sync 실패 위험 |
| **M4** | `playbox` bash CLI와 `scalex` Rust CLI가 중복 기능 보유 (cluster 생성 등) — 통합 안 됨 | 사용자 혼란, 유지보수 부담 |

### 경미한 문제 (Minor)

| # | 문제 | 영향 |
|---|------|------|
| **m1** | `tofu.rs` L233: gateway가 `192.168.88.1`로 하드코딩 (TODO 주석) | SDI spec의 gateway 미반영 |
| **m2** | OLD `gitops/clusters/playbox/catalog.yaml` 정리 안 됨 | 혼란 유발 |

---

## Checklist 재검증

| # | 질문 | 상태 | 조치 |
|---|------|------|------|
| 1 | OpenTofu 전체 가상화 | **해결** | `tofu/` → `.legacy-tofu/`, scalex SDI가 `_generated/sdi/`에 멀티호스트 HCL 생성 |
| 2 | DataX kubespray 반영 | **해결** | 7개 필드 추가 (firewalld, kube_vip, gateway_api, graceful_shutdown, kubelet_custom_flags) |
| 3 | Keycloak 완성 | **가이드** | ops-guide.md에 수동 설정 가이드 (사용자 WebUI 설정 필요) |
| 4 | CF tunnel GitOps | **완료** | Helm chart via ArgoCD ApplicationSet |
| 5 | CF tunnel 완성 | **가이드** | WebUI 설정 필요 (docs/ops-guide.md Section 1) |
| 6 | CLI 이름 scalex | **완료** | Rust CLI `scalex` |
| 7 | Rust CLI | **완료** | 35 tests, clippy clean, FP style |
| 8 | CLI 기능 | **완료** | facts/get/sdi(+sync)/cluster + gitops URL 자동화 |
| 9 | 베어메탈 확장성 | **해결** | k3s 제거, 전체 Kubespray 통일, `ClusterMode::Baremetal` 지원 |
| 10 | credentials 구조화 | **완료** | example 파일들 + .gitignore 적용 |
| 11 | 커널 튜닝 | **가이드** | ops-guide.md Section 3 (IOMMU, network, storage) |
| 12 | 디렉토리 구조 | **해결** | OLD `gitops/clusters/playbox/` 삭제, common/tower/sandbox 단일 구조 |
| 13 | CF tunnel 가이드 | **완료** | 6단계 WebUI 가이드 |
| 14 | 외부 접근 | **완료** | CF tunnel + Tailscale + LAN 가이드 |

---

## 실행 계획 (최소 핵심 기능 단위)

### WS-1: GitOps 이중구조 해소 (Critical C1, C3, C4, m2)

> spread.yaml → NEW 경로로 전환, OLD 구조 제거, Kyverno 추가

- [x] **1-1** `spread.yaml` 재작성: tower-root → `gitops/generators/tower/`, sandbox-root → `gitops/generators/sandbox/`
- [x] **1-2** Kyverno를 tower/sandbox common-generator에 추가
- [x] **1-3** `gitops/clusters/playbox/` (OLD) 전체 삭제
- [x] **1-4** YAML 유효성 검증 통과
- [x] **1-5** 테스트: OLD 경로 참조 없음 확인 (grep 검증)

### WS-2: 레포 URL 통일 (Major M3)

> 모든 gitops YAML의 repoURL을 실제 레포와 일치시킴

- [x] **2-1** 실제 GitHub 레포 URL 확인 → `k8s-playbox.git` 이미 일치
- [x] **2-2** 교체 불필요 (URL 일치 확인)
- [x] **2-3** 검증 완료

### WS-3: tofu/ 정리 — k3s 제거 (Critical C2, Checklist #9)

> tofu/ 디렉토리는 scalex-cli가 `_generated/sdi/`에 생성하는 HCL로 대체. 기존 k3s 기반 tofu/는 레거시로 표시 또는 제거.

- [x] **3-1** `tofu/` → `.legacy-tofu/`로 이동 (참고용 보존)
- [x] **3-2** `CLAUDE.md` 업데이트: scalex 기반 아키텍처로 전면 재작성
- [x] **3-3** `playbox` bash CLI 상단 deprecation notice 추가

### WS-4: kubespray cluster-vars DataX 설정 보강 (Major M2, Checklist #2)

> DataX kubespray의 핵심 설정을 scalex-cli의 cluster-vars 생성기에 반영

- [x] **4-1** DataX 설정 diff 분석 완료
- [x] **4-2** `CommonConfig` 모델에 7개 필드 추가: `firewalld_enabled`, `kube_vip_enabled`, `kubelet_custom_flags`, `gateway_api_enabled`, `gateway_api_version`, `graceful_node_shutdown`, `graceful_node_shutdown_sec`
- [x] **4-3** `generate_cluster_vars()` 함수에 새 필드 출력 추가
- [x] **4-4** TDD: `test_generate_cluster_vars_datax_settings` 테스트 작성 → RED → GREEN → 통과
- [x] **4-5** `k8s-clusters.yaml.example` 업데이트 (새 필드 반영)

### WS-5: Sandbox server URL 자동화 (Major M1)

> `scalex cluster init` 실행 시 sandbox API server URL을 generators/projects에 자동 주입

- [x] **5-1** `core/gitops.rs` 모듈 설계 (순수 함수)
- [x] **5-2** `replace_sandbox_server_url()`, `has_sandbox_placeholder()`, `replace_all_sandbox_urls()` 구현
- [x] **5-3** TDD: 4개 테스트 작성 → GREEN (치환, 감지, 일괄 치환, 내용 보존)
- [x] **5-4** `scalex cluster init` 파이프라인에 통합 (kubeconfig → server URL 추출 → gitops 치환)

### WS-6: tofu.rs gateway 하드코딩 수정 (Minor m1)

- [x] **6-1** `generate_vm_resource()`에 gateway 파라미터 추가, 하드코딩 제거
- [x] **6-2** TDD: `test_generate_tofu_uses_spec_gateway` 테스트 → RED → GREEN

### WS-7: playbox CLI deprecation 정리 (Major M4)

> playbox bash CLI는 scalex Rust CLI로 대체되는 과도기. CLAUDE.md에 관계 명시.

- [x] **7-1** `playbox` 스크립트 상단에 deprecation notice 추가
- [x] **7-2** `CLAUDE.md` 업데이트: scalex가 primary CLI임을 명시

### WS-8: 최종 검증

- [x] **8-1** `cargo test` 전체 통과 (35 tests, 0 failures)
- [x] **8-2** `cargo clippy` clean, `cargo fmt --check` clean
- [x] **8-3** gitops YAML 유효성 검증 통과 (python3 yaml.safe_load_all)
- [x] **8-4** gitops 구조 일관성 검증 (spread → generators → common/tower/sandbox, OLD 경로 참조 0건)
- [x] **8-5** 커밋 및 푸쉬 (b56a034)

---

## 진행 상황 추적

| WS | 설명 | 상태 | 비고 |
|----|------|------|------|
| WS-1 | GitOps 이중구조 해소 | **완료** | spread.yaml 재작성, OLD 삭제, Kyverno 추가 |
| WS-2 | 레포 URL 통일 | **완료** | 이미 일치 확인 |
| WS-3 | tofu/ k3s 제거 | **완료** | `.legacy-tofu/`로 이동, CLAUDE.md 재작성 |
| WS-4 | kubespray DataX 설정 보강 | **완료** | TDD: 7개 필드 추가, 35 tests pass |
| WS-5 | Sandbox URL 자동화 | **완료** | TDD: `core/gitops.rs` 4개 순수 함수 + 4 tests |
| WS-6 | tofu.rs gateway 수정 | **완료** | TDD: spec에서 gateway 전달, 하드코딩 제거 |
| WS-7 | playbox deprecation | **완료** | deprecation notice + CLAUDE.md 업데이트 |
| WS-8 | 최종 검증 | **완료** | 37 tests, clippy clean, fmt clean |

---

## 기존 완료 항목 (검증 완료)

- [x] credentials/ 구조 (`.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`)
- [x] config/ 스키마 (`baremetal.yaml.example`, `sdi-specs.yaml.example`, `k8s-clusters.yaml.example`)
- [x] scalex-cli Rust 프로젝트 (clap, serde, thiserror, FP style)
- [x] `scalex facts` (SSH → HW 정보 수집 → JSON)
- [x] `scalex get` (baremetals, sdi-pools, clusters, config-files)
- [x] `scalex sdi init/clean/sync` (OpenTofu HCL 생성, host prep)
- [x] `scalex cluster init` (inventory + vars 생성, kubespray 실행)
- [x] `ClusterMode::Baremetal` 지원 (SDI 없이 직접 kubespray)
- [x] gitops/common/ (cilium, cilium-resources, cert-manager, cluster-config, kyverno)
- [x] gitops/tower/ (argocd, keycloak, cloudflared-tunnel, socks5-proxy)
- [x] gitops/sandbox/ (local-path-provisioner, rbac, test-resources)
- [x] generators/ (tower-common, tower-apps, sandbox-common, sandbox-apps)
- [x] projects/ (tower-project, sandbox-project)
- [x] 커널 튜닝 가이드 (docs/ops-guide.md)
- [x] Cloudflare Tunnel 가이드 (docs/ops-guide.md)
- [x] Keycloak 설정 가이드 (docs/ops-guide.md)
- [x] LAN/외부 접근 가이드 (docs/ops-guide.md)

## 사용자 수동 작업 (변경 없음)

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
