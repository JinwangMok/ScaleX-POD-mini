# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Critical Analysis (2026-03-11 Sprint 16 재평가)

### 이전 DASHBOARD의 근본적 문제

> **이전 DASHBOARD는 "코드가 존재하고 단위 테스트가 통과하면 VERIFIED"라는 기준을 사용했다.**
> 이는 순수 함수의 문자열 출력 검증일 뿐이며, 다음과 같은 근본적 한계가 있다:
>
> 1. **375개 테스트 전부 오프라인 순수 함수 테스트** — 실제 SSH/libvirt/Kubespray/ArgoCD 통합 테스트는 0건
> 2. **"FIXED" 판정은 코드 추가를 의미할 뿐, 실제 동작 검증이 아님** — 물리 인프라 없이 검증 불가능한 항목이 다수
> 3. **워크플로우 통합 검증 부재** — 개별 명령어가 통과해도 `sdi init → cluster init → bootstrap` 파이프라인이 실제로 작동하는지 확인되지 않음
>
> Sprint 16에서는 이를 인정하고, **코드 레벨에서 발견 가능한 실제 버그/갭**을 식별하여 수정한다.

### Sprint 16에서 발견한 새로운 갭 (코드 레벨)

| ID | 심각도 | 갭 | 상세 |
|----|--------|-----|------|
| **G-1** | ~~HIGH~~ | ~~`sdi clean` host-infra tofu 미파괴~~ | ✅ 해결: `TofuDestroyHostInfra` variant 추가, host-infra tofu destroy를 main destroy 전에 실행 |
| **G-2** | ~~MEDIUM~~ | ~~`sdi init <spec>` resource-pool-summary 미생성~~ | ✅ 해결: summary 생성을 공통 경로(spec/no-spec 분기 전)로 이동 |
| **G-3** | ~~MEDIUM~~ | ~~`sdi init` spec 파일 미캐싱~~ | ✅ 해결: spec 경로에서 `sdi-spec-cache.yaml` 자동 생성 |
| **G-4** | ~~LOW~~ | ~~`sdi init` no-flag 중복 facts 로드~~ | ✅ 검증: `load_all_facts()` 호출이 이미 1회만 수행됨 확인 |
| **G-5** | ~~LOW~~ | ~~README 테스트 수 미반영~~ | ✅ 해결: README/CLAUDE.md "380 tests"로 갱신 |

### 이전에 해결된 갭 (Sprint 15 시리즈)

| ID | 상태 | 갭 | 해결 내역 |
|----|------|-----|----------|
| C-1 | **FIXED** | ArgoCD 부트스트랩 누락 | `scalex bootstrap` 3-phase pipeline 구현 |
| C-2 | **FIXED** | Sandbox 클러스터 ArgoCD 미등록 | `scalex bootstrap` Phase 2에서 자동 등록 |
| C-3 | **FIXED** | Kubespray 경로 해결 버그 | `kubespray/kubespray/` 서브모듈 경로 우선 |
| C-4 | **VERIFIED** | `sdi init` no-flag 구현 | 호스트 준비 + resource-pool + host-infra HCL + tofu apply |
| C-5 | **RESOLVED** | 외부 sandbox kubectl | 아키텍처 결정: Tower 경유 관리 |
| C-6 | **FIXED** | Legacy 네이밍 | `scalex-root` 통일 |

---

## Checklist Status (15 Items) — Sprint 16 재평가

> **판정 기준 (엄격 적용)**:
> - **VERIFIED**: 순수 함수 테스트 통과 + 코드 로직 검토 완료 (오프라인 레벨)
> - **BUG**: 코드에 버그가 있어 수정 필요
> - **GAP**: 요구사항 대비 코드/기능 부재
> - **NEEDS-INFRA**: 물리 인프라에서만 검증 가능
> - **PARTIAL**: 부분 구현 — 추가 작업 필요

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL 생성 (4노드) | VERIFIED | `core/tofu.rs` — 12 tests |
| 리소스 풀 summary | VERIFIED | `core/resource_pool.rs` — 7 tests |
| 2 클러스터 분할 | VERIFIED | `core/validation.rs` — pool mapping |
| `sdi init` no-flag 통합 리소스 풀 | VERIFIED | host 준비 + resource-pool-summary + host-infra HCL + tofu apply |
| `sdi init <spec>` resource-pool-summary | VERIFIED | G-2 해결: 공통 경로에서 항상 생성 |
| `sdi clean` host-infra 정리 | VERIFIED | G-1 해결: host-infra tofu destroy 추가 |
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
| Sandbox 외부 접근 | RESOLVED | 아키텍처 결정 문서화 (ops-guide Section 5) |

### #4. CLI Rust 구현 + FP 원칙

| 항목 | 상태 | 비고 |
|------|------|------|
| Rust 구현 | VERIFIED | 26 .rs files |
| FP 원칙 (Pure Function) | VERIFIED | 모든 generator 순수 함수, I/O 분리 |
| thiserror + clap derive | VERIFIED | 에러 처리 + CLI 파싱 |
| 코드 구조 | VERIFIED | commands/core/models 3계층 |

### #5-7. 문서화 + Installation Guide

| 항목 | 상태 | 비고 |
|------|------|------|
| README Step 0-8 | VERIFIED | Pre-flight 포함 9단계 가이드 |
| docs/ 7개 문서 | VERIFIED | architecture, ops-guide, troubleshooting 등 |
| CLI 레퍼런스 | VERIFIED | README에 core + query 전체 문서화 |
| README 테스트 수 | VERIFIED | 380 tests (G-5 해결) |
| E2E 실행 검증 | NEEDS-INFRA | — |

### #8. CLI 기능 전체

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | VERIFIED | 4 tests, SSH 스크립트 + 파싱 |
| `scalex sdi init` (no flag) | VERIFIED | host 준비 + resource-pool + host-infra HCL + tofu apply |
| `scalex sdi init <spec>` | VERIFIED | HCL+state 생성, resource-pool-summary 생성 (G-2 해결), spec 캐싱 (G-3 해결) |
| `scalex sdi clean --hard` | VERIFIED | host-infra tofu 리소스 파괴 포함 (G-1 해결) |
| `scalex sdi sync` | VERIFIED | 13 tests (diff, conflict, add/remove) |
| `scalex cluster init` | VERIFIED | inventory + vars + kubespray 실행 |
| `scalex get` (4 subcommands) | VERIFIED | 18 tests |
| `scalex bootstrap` | VERIFIED | ArgoCD Helm + 클러스터 등록 + spread 적용 — 14 tests |

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
| 불필요 파일 없음 | VERIFIED | `.omc/` 상태 파일은 gitignored |

### #13. 멱등성

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL/inventory/vars 재생성 동일성 | VERIFIED | idempotency tests |
| 실환경 재적용 | NEEDS-INFRA | — |

### #14. 외부 kubectl (CF Tunnel)

| 항목 | 상태 | 비고 |
|------|------|------|
| Tower Pre-OIDC kubectl | VERIFIED (docs) | ops-guide Section 4 |
| Sandbox 외부 kubectl | RESOLVED | Tower 경유 관리 (ops-guide Section 5) |
| SOCKS5 프록시 | VERIFIED | 배포 manifest 존재 |

### #15. NAT 접근 경로

| 항목 | 상태 | 비고 |
|------|------|------|
| CF Tunnel + Tailscale + LAN | VERIFIED | ops-guide Section 4 |
| 스위치 접근 가이드 | VERIFIED | 포함됨 |

---

## Execution Plan — Sprint 16 시리즈

### Sprint 16a: `sdi clean` host-infra tofu destroy 수정 (G-1) — HIGH

> **TDD**: RED → GREEN → REFACTOR

- [x] RED: `plan_clean_hard`가 host-infra 디렉토리의 tofu destroy를 포함하는 테스트
- [x] RED: `plan_clean_soft`도 host-infra main.tf 존재 시 destroy 포함 테스트
- [x] GREEN: `run_clean()`에 host-infra 디렉토리 tofu destroy 로직 추가
- [x] REFACTOR: `TofuDestroyHostInfra` variant + `plan_clean_operations` 5th param

### Sprint 16b: `sdi init <spec>` resource-pool-summary 생성 (G-2) — MEDIUM

> **TDD**: RED → GREEN → REFACTOR

- [x] RED: spec 기반 init에서도 resource-pool-summary.json이 생성되는 테스트
- [x] GREEN: summary 생성을 공통 경로(spec/no-spec 분기 전)로 이동
- [x] REFACTOR: no-spec 분기의 중복 summary 생성 제거

### Sprint 16c: `sdi init` spec 캐싱 (G-3) — MEDIUM

> **TDD**: RED → GREEN → REFACTOR

- [x] RED: `sdi init <spec>` 실행 후 `sdi-spec-cache.yaml` 존재 테스트
- [x] GREEN: `run_init()` spec 경로에서 sdi-spec-cache.yaml 저장
- [x] cluster.rs의 `load_sdi_spec_from_options()`이 캐시 폴백 경로 이미 구현 확인

### Sprint 16d: `sdi init` no-flag 중복 facts 로드 수정 (G-4) — LOW

- [x] 검증: `load_all_facts()` 호출이 `run_init()`에서 1회만 수행됨 확인 — 이미 해결됨

### Sprint 16e: README 테스트 수 업데이트 (G-5) — LOW

- [x] README.md의 "352 tests" → 380 tests로 갱신
- [x] DASHBOARD.md 최종 테스트 수 380으로 업데이트

### Sprint 17: 실환경 E2E (물리 인프라 필요)

- [ ] I-1: `scalex facts --all` 실행 → 4노드 HW 정보 수집
- [ ] I-2: `scalex sdi init` → host-infra 구성 + VM 생성
- [ ] I-3: `scalex cluster init` → K8s 프로비저닝 (tower + sandbox)
- [ ] I-4: `scalex bootstrap` → ArgoCD + GitOps
- [ ] I-5: CF Tunnel 외부 kubectl 접근 검증
- [ ] I-6: `sdi clean --hard --yes-i-really-want-to` + 재구축 (멱등성)

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
│   ├── host-infra/ (no-flag: host-level libvirt infra)
│   └── main.tf     (spec: VM pool HCL)
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
| core/validation | 74 | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave, AppProject, sdi-init, E2E pipeline, SSH, 3rd cluster, GitOps consistency, spec caching |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generators |
| core/kubespray | 32 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node |
| commands/status | 21 | platform status reporting |
| commands/sdi | 24 | network resolve, host infra, pool state, clean validation, CIDR prefix, host-infra clean |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14 | ArgoCD helm, cluster add, kubectl apply, pipeline |
| core/sync | 13 | sync diff, VM conflict, add+remove |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node, host-infra |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table, disk_gb |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| commands/facts | 4 | facts gathering |
| **TOTAL** | **380** | |

---

## Sprint History

| Sprint | Date | Tests | Summary |
|--------|------|-------|---------|
| 16a | 2026-03-11 | 379 | `sdi clean` host-infra tofu destroy 수정 (G-1) — 3 tests 추가 |
| 16b | 2026-03-11 | 379 | `sdi init <spec>` resource-pool-summary 공통 경로 이동 (G-2) — 1 test 추가 |
| 16c | 2026-03-11 | 380 | `sdi init` spec 캐싱 for cluster init (G-3) — 1 test 추가 |
| 16d | 2026-03-11 | 380 | 중복 facts 로드 — 이미 해결됨 확인 (G-4) |
| 16e | 2026-03-11 | 380 | README/DASHBOARD 테스트 수 업데이트 (G-5) |
| 15f | 2026-03-11 | 375 | playbox-root → scalex-root rename |
| 15e | 2026-03-11 | 374 | Sandbox 외부 접근 아키텍처 결정 |
| 15d | 2026-03-11 | 372 | resource_pool에 disk_gb 필드 추가 |
| 15b-c | 2026-03-11 | 370 | `scalex bootstrap` + README 업데이트 |
| 15a | 2026-03-11 | 355 | Kubespray 서브모듈 경로 해결 |
| 13d | 2026-03-11 | 352 | Edge cases: Cilium cluster_id, CIDR overlap |
| 13c | 2026-03-11 | 347 | 2-layer template, OIDC, credentials |
| 13b | 2026-03-11 | 342 | pre-OIDC kubectl, NAT 접근 |
| 13a | 2026-03-11 | 340 | Checklist 15항목 갭 분석 |
