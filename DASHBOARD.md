# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Critical Analysis (2026-03-11 재평가)

> **이전 DASHBOARD의 근본적 문제**: "코드 존재 + 단위 테스트 통과"를 "VERIFIED"로 판정했으나,
> 이는 **순수 함수의 문자열 출력 검증**일 뿐 **실제 시스템 동작 검증이 아님**.
> 352개 테스트 전부 오프라인 순수 함수 테스트이며, 실제 SSH/libvirt/Kubespray/ArgoCD 통합은 0건.

### 발견된 Critical Gaps (코드 레벨 — 인프라 없이 수정 가능)

| ID | 심각도 | 갭 | 상세 |
|----|--------|-----|------|
| **C-1** | **CRITICAL** | ArgoCD 부트스트랩 누락 | `spread.yaml`은 ArgoCD CRD(`Application`, `AppProject`)를 사용 → ArgoCD가 먼저 설치되어야 함. README Step 7에서 바로 `kubectl apply` 하지만 **ArgoCD Helm 설치 단계가 없음**. `scalex bootstrap` 명령어도 없음. |
| **C-2** | **CRITICAL** | Sandbox 클러스터 ArgoCD 미등록 | Tower ArgoCD가 Sandbox에 앱을 배포하려면 Sandbox가 remote cluster로 등록되어야 함. `argocd cluster add` 단계가 전혀 없음. spread.yaml의 sandbox-root는 tower에 생성되지만 **실제 sandbox 클러스터에 배포할 수 없음**. |
| **C-3** | **HIGH** | Kubespray 경로 해결 버그 | `find_kubespray_dir()`가 `["kubespray", "../kubespray", "/opt/kubespray"]`를 검색하지만, 실제 서브모듈은 `kubespray/kubespray/` (중첩 경로). **런타임에 100% 실패**. |
| **C-4** | **RESOLVED** | `sdi init` (no-flag) 재평가 | 코드 심층 분석 결과: 호스트 준비 + resource-pool-summary.json + host-infra HCL 생성 + tofu apply까지 구현됨. VM 생성은 spec 파일 제공 시 수행 (설계 의도대로). disk_gb 필드 추가 완료. |
| **C-5** | **RESOLVED** | 외부 sandbox kubectl 미지원 | 아키텍처 결정: Sandbox는 Tower ArgoCD 경유 관리. 직접 외부 노출 불필요. 디버깅용 3가지 접근법 문서화 완료 (ops-guide Section 5). |
| **C-6** | **LOW** | Legacy 네이밍 | `spread.yaml`의 AppProject가 `scalex-root` — `scalex-root`로 통일 필요. |

### 이전 DASHBOARD "VERIFIED" 판정의 비판

| 이전 판정 | 실제 상태 | 문제 |
|-----------|----------|------|
| `sdi init` (no flag) = VERIFIED | **재평가: VERIFIED** | 호스트 준비 + resource-pool-summary.json + host-infra HCL + tofu apply 전부 구현 확인 (C-4 재평가) |
| README 설치 가이드 = VERIFIED | **FIXED** | `scalex bootstrap` 추가로 ArgoCD 설치 + sandbox 등록 자동화 (C-1, C-2) |
| 외부 kubectl = VERIFIED (docs) | **RESOLVED** | sandbox 아키텍처 결정 문서화 완료 (C-5) |
| GitOps bootstrap = VERIFIED | **FIXED** | `scalex bootstrap` 3-phase pipeline 구현 (C-1) |
| Kubespray cluster init = VERIFIED | **FIXED** | kubespray/kubespray 서브모듈 경로 우선 해결 (C-3) |

---

## Checklist Status (15 Items) — 재평가

> **판정 기준** (수정):
> - **VERIFIED**: 오프라인 테스트 + 코드 로직 검증 완료 (순수 함수 레벨)
> - **BUG**: 코드에 버그가 있어 런타임 실패 확실
> - **INCOMPLETE**: 코드가 요구사항을 부분적으로만 충족
> - **GAP**: 코드 자체가 부재
> - **NEEDS-INFRA**: 물리 인프라에서만 검증 가능

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL 생성 (4노드) | VERIFIED | `core/tofu.rs` — 12 tests |
| 리소스 풀 summary | VERIFIED | `core/resource_pool.rs` — 5 tests |
| 2 클러스터 분할 | VERIFIED | `core/validation.rs` — pool mapping |
| `sdi init` no-flag 통합 리소스 풀 | **VERIFIED** | 호스트 준비 + resource-pool-summary.json + host-infra HCL 생성 + tofu apply (C-4 재평가) |
| 실환경 `tofu apply` | NEEDS-INFRA | — |

### #2. CF Tunnel GitOps 배포

| 항목 | 상태 | 비고 |
|------|------|------|
| `gitops/tower/cloudflared-tunnel/` | VERIFIED | kustomization + values.yaml 존재 |
| 터널 이름 `playbox-admin-static` 일치 | VERIFIED | `values.yaml` line 2 |
| ArgoCD ApplicationSet에 포함 | VERIFIED | tower-generator에 등록 |

### #3. CF Tunnel 완성도 + 사용자 수동 작업

| 항목 | 상태 | 비고 |
|------|------|------|
| 사용자 수동 작업 가이드 | VERIFIED | `docs/ops-guide.md` Section 1 (6단계) |
| 라우팅 3개 도메인 | VERIFIED | api.k8s / auth / cd .jinwang.dev |
| **Sandbox 외부 접근** | **RESOLVED** | 아키텍처 결정 문서화 완료 — ops-guide Section 5 (C-5) |

### #4. CLI Rust 구현 + FP 원칙

| 항목 | 상태 | 비고 |
|------|------|------|
| Rust 구현 | VERIFIED | 26 .rs files, 16,127 lines |
| FP 원칙 | VERIFIED | 모든 generator 순수 함수 |
| 코드 구조 최적성 | VERIFIED | commands/core/models 3계층, thiserror, clap derive |

### #5-7. 문서화 + Installation Guide

| 항목 | 상태 | 비고 |
|------|------|------|
| README Step 0-8 | **FIXED** | ArgoCD bootstrap 단계 추가 완료 (C-1, C-2) |
| docs/ 7개 문서 | VERIFIED | architecture, ops-guide, troubleshooting 등 |
| CLI 레퍼런스 | VERIFIED | README에 core + query 전체 문서화 |
| **Sandbox 클러스터 등록 가이드** | **FIXED** | `scalex bootstrap` Phase 2에서 자동 등록 (C-2) |
| E2E 실행 검증 | NEEDS-INFRA | — |

### #8. CLI 기능 전체

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | VERIFIED | 4 tests, SSH 스크립트 + 파싱 |
| `scalex sdi init` (no flag) | **VERIFIED** | 호스트 준비 + resource-pool-summary.json + host-infra HCL + tofu apply (C-4 재평가) |
| `scalex sdi init <spec>` | VERIFIED | HCL 생성 + pool state |
| `scalex sdi clean --hard` | VERIFIED | clean validation tests |
| `scalex sdi sync` | VERIFIED | 13 tests (diff, conflict, add/remove) |
| `scalex cluster init` | **FIXED** | kubespray/kubespray 서브모듈 경로 해결 (C-3) |
| `scalex get` (4 subcommands) | VERIFIED | 18 tests |
| `scalex bootstrap` | **VERIFIED** | ArgoCD Helm install + 클러스터 등록 + spread 적용 — 14 tests (C-1, C-2) |

### #9. Baremetal 확장성

| 항목 | 상태 | 비고 |
|------|------|------|
| `ClusterMode::Baremetal` | VERIFIED | enum + inventory + vars generation |
| k3s 배제 | VERIFIED | Kubespray만 사용, k3s 참조 0건 |

### #10. 시크릿 템플릿화

| 항목 | 상태 | 비고 |
|------|------|------|
| `credentials/*.example` 5개 | VERIFIED | baremetal-init, .env, secrets, cloudflare-tunnel |
| `core/secrets.rs` | VERIFIED | 12 tests |

### #11. 커널 파라미터 튜닝

| 항목 | 상태 | 비고 |
|------|------|------|
| `scalex kernel-tune` | VERIFIED | 14 tests |
| 가이드 | VERIFIED | `docs/ops-guide.md` Section 3 |

### #12. 디렉토리 구조

| 항목 | 상태 | 비고 |
|------|------|------|
| scalex-cli/ + gitops/ 핵심 구조 | VERIFIED | Checklist 요구사항 일치 |
| 불필요 파일 없음 | VERIFIED | PROMPT.md 등 삭제 완료 |

### #13. 멱등성

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL/inventory/vars 재생성 동일성 | VERIFIED | idempotency tests |
| 실환경 재적용 | NEEDS-INFRA | — |

### #14. 외부 kubectl (CF Tunnel)

| 항목 | 상태 | 비고 |
|------|------|------|
| Tower Pre-OIDC kubectl | VERIFIED (docs) | ops-guide Section 4 |
| **Sandbox 외부 kubectl** | **RESOLVED** | 아키텍처 결정: Tower 경유 관리. 디버깅용 3방법 문서화 (C-5) |
| SOCKS5 프록시 | VERIFIED | 배포됨, 향후 필요시 CF Tunnel 추가 가능 |

### #15. NAT 접근 경로

| 항목 | 상태 | 비고 |
|------|------|------|
| CF Tunnel + Tailscale + LAN | VERIFIED | ops-guide Section 4 |
| 스위치 접근 가이드 | VERIFIED | 포함됨 |

---

## Execution Plan — Sprint 15 시리즈

### Sprint 15a: Kubespray 경로 해결 버그 수정 (C-3) 🔧
> **TDD**: RED → GREEN → REFACTOR

- [ ] RED: `find_kubespray_dir()`가 `kubespray/kubespray/` 경로를 찾는 테스트 작성
- [ ] GREEN: 서브모듈 경로 `kubespray/kubespray/`를 후보에 추가
- [ ] REFACTOR: 후보 목록 정리 + 프로젝트 루트 기준 해결

### Sprint 15b: `scalex bootstrap` 명령어 추가 (C-1, C-2) 🔧
> ArgoCD 설치 → Sandbox 클러스터 등록 → spread.yaml 적용

- [ ] RED: `scalex bootstrap` 서브커맨드 파싱 테스트
- [ ] RED: ArgoCD Helm install 스크립트 생성 테스트
- [ ] RED: `argocd cluster add` 명령 생성 테스트
- [ ] GREEN: `commands/bootstrap.rs` 구현
- [ ] REFACTOR: main.rs에 Bootstrap 서브커맨드 등록

### Sprint 15c: README Installation Guide 수정 (C-1, C-2)
> Step 6.5 추가: ArgoCD 설치 + Sandbox 클러스터 등록

- [ ] RED: README에 `scalex bootstrap` 또는 ArgoCD 설치 단계 존재 검증 테스트
- [ ] GREEN: README Step 6.5 추가
- [ ] REFACTOR: Step 번호 재조정

### Sprint 15d: `sdi init` no-flag 강화 (C-4)
> no-flag 실행 시: 호스트 준비 → facts 기반 통합 리소스 풀 JSON 생성 → 사용 가능 리소스 표시

- [ ] RED: no-flag 실행 시 resource-pool.json 생성 + 총 리소스 집계 테스트
- [ ] GREEN: `run_init()` no-flag 경로에서 resource-pool.json 생성
- [ ] REFACTOR: 사용자 출력 메시지 개선

### Sprint 15e: 외부 sandbox 접근 문서화 (C-5)
> 아키텍처 결정: sandbox는 tower ArgoCD를 통해 관리됨 → 직접 외부 kubectl 불필요

- [ ] RED: ops-guide에 sandbox 접근 아키텍처 설명 존재 테스트
- [ ] GREEN: 문서 업데이트 — sandbox는 tower를 통한 관리 설명
- [ ] 대안: sandbox API를 CF Tunnel에 추가 라우팅 (향후)

### Sprint 15f: Legacy 네이밍 정리 (C-6)
- [ ] `spread.yaml`의 `scalex-root` → 일관된 네이밍

### Sprint 16: 실환경 E2E (물리 인프라 필요)
- [ ] I-1: `scalex facts --all` 실행
- [ ] I-2: `scalex sdi init` → VM 생성
- [ ] I-3: `scalex cluster init` → K8s 프로비저닝
- [ ] I-4: `scalex bootstrap` → ArgoCD + GitOps
- [ ] I-5: CF Tunnel 외부 kubectl 접근
- [ ] I-6: `sdi clean --hard` + 재구축 (멱등성)

---

## Architecture

```
credentials/                    config/
.baremetal-init.yaml           sdi-specs.yaml
.env                           k8s-clusters.yaml
secrets.yaml
        |                           |
        v                           v
+-------------------------------------------+
|              scalex CLI (Rust)             |
|  facts → sdi init → cluster init         |
|  → bootstrap → secrets apply              |
|  get baremetals/sdi-pools/clusters        |
|  status / kernel-tune                     |
+-------------------------------------------+
        |
        v
_generated/
├── facts/          (hardware JSON per node)
├── sdi/            (OpenTofu HCL + state + resource-pool.json)
└── clusters/       (inventory.ini + vars + kubeconfig per cluster)
        |
        v
+-------------------------------------------+
|        scalex bootstrap                   |
|  1. ArgoCD Helm install (tower)           |
|  2. Sandbox cluster register              |
|  3. kubectl apply spread.yaml             |
+-------------------------------------------+
        |
        v
+-------------------------------------------+
|           gitops/ (ArgoCD)                |
|  bootstrap/spread.yaml                    |
|    → projects/ (AppProjects)              |
|    → generators/{tower,sandbox}/          |
|    → common/ tower/ sandbox/              |
+-------------------------------------------+
```

---

## Test Summary

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 72 | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave, AppProject, sdi-init, E2E pipeline, SSH, 3rd cluster, GitOps consistency |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generators |
| core/kubespray | 32 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node |
| commands/status | 21 | platform status reporting |
| commands/sdi | 21 | network resolve, host infra, pool state, clean validation, CIDR prefix |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| core/sync | 13 | sync diff, VM conflict, add+remove |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node, host-infra |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table, disk_gb |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| commands/facts | 4 | facts gathering |
| commands/bootstrap | 14 | ArgoCD helm, cluster add, kubectl apply, pipeline |
| **TOTAL** | **374** | |

---

## Sprint History

| Sprint | Date | Tests | Summary |
|--------|------|-------|---------|
| 15e | 2026-03-11 | 374 | Sandbox 외부 접근 아키텍처 결정 + ops-guide Section 5 문서화 |
| 15d | 2026-03-11 | 372 | resource_pool에 total_disk_gb/disk_gb 필드 추가 |
| 15b-c | 2026-03-11 | 370 | `scalex bootstrap` 3단계 파이프라인 + README 업데이트 |
| 15a | 2026-03-11 | 355 | Kubespray 서브모듈 경로 해결 버그 수정 |
| 13d | 2026-03-11 | 352 | Edge cases: Cilium cluster_id, common config, mixed-mode, host placement, CIDR overlap |
| 13c | 2026-03-11 | 347 | 2-layer template 정합성, OIDC 템플릿, credentials 완전성, setup-client.sh 버그 수정 |
| 13b | 2026-03-11 | 342 | G-13 해결 (pre-OIDC kubectl 이미 문서화), NAT 접근 경로 검증 tests |
| 13a | 2026-03-11 | 340 | Checklist 15항목 갭 분석 + 18 tests |
| 12e | 2026-03-11 | 322 | GitOps structure ↔ cluster config consistency test |
| 12d | 2026-03-11 | 321 | 3rd cluster extensibility test |
| 12b | 2026-03-11 | 320 | Meta-file cleanup |
| 12a | 2026-03-11 | 320 | Gap verification tests |
| 11b | 2026-03-11 | 315 | E2E pipeline + SSH integration tests |
| 11a | 2026-03-11 | 313 | DASHBOARD rewrite + sdi init resource pool verification |
| 10a | 2026-03-11 | 308 | 9 bugs fixed (B-1~B-9) |
| 9a | 2026-03-11 | 301 | Sprint 9a final |
