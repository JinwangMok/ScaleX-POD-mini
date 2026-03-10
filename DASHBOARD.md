# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status

- **Tests**: 239 pass / clippy 0 warnings / fmt clean
- **Code**: ~11,000 lines Rust, 27 source files, ~200 pure functions
- **GitOps**: 33 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## Checklist Gap Analysis

> 상태: PASS = 완료 및 검증됨, PARTIAL = 부분 구현, FAIL = 미구현 또는 미검증

### CL-1: 4개 노드 OpenTofu 가상화 + 2-클러스터 구조

**상태: PARTIAL → 대부분 PASS (실환경 테스트만 남음)**

| 항목 | 상태 | 비고 |
|------|------|------|
| SDI 모델 (SdiSpec, NodeSpec) | PASS | `models/sdi.rs` — 파싱/직렬화 테스트 통과 |
| OpenTofu HCL 생성 | PASS | `core/tofu.rs` — 호스트 인프라 + VM 생성 HCL 순수 함수 |
| sdi-specs.yaml 예제 (4노드, 2풀) | PASS | `config/sdi-specs.yaml.example` — tower + sandbox 풀 정의 |
| baremetal-init.yaml 스키마 (3가지 접근 방식) | PASS | direct / external IP / ProxyJump 모두 지원 |
| 단일 노드 SDI 검증 | **PASS** | `test_single_node_sdi_tower_and_sandbox_on_one_host` (Sprint 1) |
| `scalex sdi init` (no flag) 동작 | **PASS** | 리소스 풀 요약 JSON 생성 + 호스트 인프라 HCL (Sprint 3.1) |
| `scalex get sdi-pools` 통합 뷰 | **PASS** | `resource-pool-summary.json` 폴백 지원 (Sprint 3.2) |
| 실제 HW 테스트 | **FAIL** | 물리 인프라 접근 필요 — Sprint 5 |

### CL-2: Cloudflare Tunnel ArgoCD/GitOps 방식 — **PASS**

### CL-3: Cloudflare Tunnel 설정 완료 여부

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| GitOps 배포 자동화 | PASS | cloudflared-tunnel kustomization 존재 |
| 사용자 매뉴얼 가이드 | PASS | `docs/ops-guide.md` Step 1-6, 터널 이름 `playbox-admin-static` 반영 완료 |
| 외부 kubectl 접근 검증 | **FAIL** | Keycloak 미설정 시 Cloudflare Tunnel만으로 kubectl 접근 미검증 — Sprint 5 |

### CL-4: Rust CLI + FP 스타일 — **PASS**

- 232 tests, 0 clippy warnings, fmt clean
- 순수 함수: HCL 생성, inventory 생성, validation 모두 side-effect 없음

### CL-5: 사용자 친절 가이드 — **PARTIAL**

- ops-guide.md, SETUP-GUIDE.md, ARCHITECTURE.md, TROUBLESHOOTING.md 존재
- "초보자도 따라할 수 있는" 수준까지는 에러 복구 가이드 보강 필요

### CL-6: README.md 디테일 — **PASS**

- 설계 철학 7원칙 섹션 추가 (Sprint 2)
- Installation Guide 작성 (Sprint 2)
- Architecture, CLI Reference, GitOps, Project Structure 모두 포함

### CL-7: README Installation Guide — **PASS (오프라인)**

- Step 0-8 단계별 Installation Guide 작성 (Sprint 2)
- 실환경 end-to-end 검증은 Sprint 5

### CL-8: CLI 기능 완전성

**상태: PASS (오프라인 검증 완료)**

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | PASS | SSH로 HW 정보 수집 |
| `scalex sdi init` (no flag) | **PASS** | 리소스 풀 요약 + 호스트 인프라 HCL 생성 |
| `scalex sdi init <spec>` | PASS | SDI 풀 생성 HCL + VM 프로비저닝 |
| `scalex sdi clean --hard` | PASS | 전체 초기화 로직 |
| `scalex sdi sync` | PASS | 노드 추가/제거 diff + 사이드이펙트 감지 (4 tests) |
| `scalex cluster init` | PASS | Kubespray inventory/vars 생성, OIDC 지원 |
| `scalex get baremetals` | PASS | facts JSON → 테이블 |
| `scalex get sdi-pools` | **PASS** | VM 풀 + 베어메탈 리소스 풀 통합 뷰 |
| `scalex get clusters` | PASS | 클러스터 인벤토리 |
| `scalex get config-files` | PASS | 설정 파일 검증 |
| `scalex secrets apply` | PASS | K8s secret 생성 |
| `scalex status` | PASS | 5-layer 상태 |
| `scalex kernel-tune` | PASS | 커널 파라미터 권장 |
| 3번째 클러스터 확장성 | **PASS** | 3-pool/3-cluster 검증 + 중복 ID 거부 (Sprint 4.1) |

### CL-9: 베어메탈 모드 확장성 — **PASS**

- `ClusterMode::Baremetal` enum + inventory 생성 테스트
- `test_baremetal_mode_inventory_generation` (Sprint 1)
- k3s 완전 제거 확인

### CL-10: 보안 정보 템플릿화 — **PASS**

### CL-11: 커널 파라미터 튜닝 — **PASS**

### CL-12: 디렉토리 구조 — **PARTIAL**

- 모든 필수 디렉토리 존재
- `.legacy-*` 파일 삭제가 git에 미커밋 (git status 'D' 상태)

### CL-13: 멱등성 — **PASS (오프라인)**

- HCL/inventory/cluster-vars 멱등성 테스트 (Sprint 1)
- I/O 오케스트레이션 멱등성은 Sprint 5

### CL-14: Cloudflare Tunnel 가이드 + 외부 kubectl — **PARTIAL**

- ops-guide.md 터널 이름 `playbox-admin-static` 반영 완료
- 외부 kubectl 접근 실증은 Sprint 5

### CL-15: NAT 접근 방법 — **PASS**

---

## Sprint Plan

> 최소 핵심 기능 단위로 분할. TDD: RED → GREEN → REFACTOR → COMMIT

### Sprint 1: 테스트 강화 + 레거시 정리 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 1.1 | 단일 노드 SDI 설정 테스트 | CL-1 | ✅ DONE |
| 1.2 | Baremetal 모드 inventory 생성 테스트 | CL-9 | ✅ DONE |
| 1.3 | 멱등성 종합 테스트 (HCL/inventory/vars 2x) | CL-13 | ✅ DONE |
| 1.4 | E2E dry-run 파이프라인 테스트 | CL-8 | ✅ DONE |
| 1.5 | 레거시 파일 삭제 커밋 | CL-12 | ✅ DONE |

### Sprint 2: README + 문서 강화 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 2.1 | README.md에 설계 철학 섹션 추가 | CL-6 | ✅ DONE |
| 2.2 | README.md에 상세 Installation Guide 작성 | CL-7 | ✅ DONE |
| 2.3 | ops-guide.md 터널 이름 `playbox-admin-static` 반영 | CL-14 | ✅ DONE |
| 2.4 | Cloudflare Tunnel kubectl 접근 시나리오 문서화 | CL-3, CL-14 | ✅ DONE |

### Sprint 3: `sdi init` (no flag) 리소스 풀 뷰 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 3.1 | `sdi init` (no flag) 통합 리소스 풀 상태 JSON 생성 | CL-1, CL-8 | ✅ DONE (이미 구현됨) |
| 3.2 | `scalex get sdi-pools` 통합 뷰 (resource-pool-summary.json 폴백) | CL-8 | ✅ DONE (2 tests) |
| 3.3 | 단일 노드 전체 파이프라인 dry-run 테스트 | CL-1 | ✅ DONE (Sprint 1.4에 포함) |

### Sprint 4: 확장성 검증 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 4.1 | 3번째 클러스터 추가 테스트 (3-pool, 3-cluster) | CL-8 | ✅ DONE (3 tests) |
| 4.2 | 2-Layer 템플릿 관리 검증 | CL-6 | ✅ DONE (E2E 파이프라인 테스트에서 검증) |
| 4.3 | `scalex sdi sync` 사이드이펙트 테스트 | CL-8 | ✅ DONE (4 tests) |

### Sprint 5: 실환경 검증 (물리 인프라 필요) — PENDING

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 5.1 | `scalex facts --all` 실행 (4노드) | CL-1, CL-8 | TODO |
| 5.2 | `scalex sdi init sdi-specs.yaml` 실행 | CL-1 | TODO |
| 5.3 | `scalex cluster init k8s-clusters.yaml` 실행 | CL-8 | TODO |
| 5.4 | `scalex secrets apply` + `kubectl apply -f gitops/bootstrap/spread.yaml` | CL-8 | TODO |
| 5.5 | 외부망에서 `kubectl get pods` 접근 검증 | CL-14 | TODO |
| 5.6 | `scalex sdi clean --hard` + 재구축 (멱등성) | CL-13 | TODO |

---

## Test Summary (232 tests)

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 30 | pool mapping, cluster IDs, CIDR, k3s, legacy, single-node, baremetal, idempotency, E2E, extensibility, sync |
| core/tofu | 15 | HCL gen (host infra + VM), VFIO XSLT, deduplication, idempotency |
| core/kubespray | 22 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, extra vars |
| core/resource_pool | 5 | aggregation, multi-node, empty, table format, bridge detection |
| core/kernel | 14 | kernel-tune recommendations |
| core/gitops | 7 | ApplicationSet, kustomization, sync waves |
| commands/get | 17 | facts row, config status, SDI pools, clusters, resource pool rows |
| commands/sdi | 12 | network resolve, host infra inputs, pool state |
| commands/cluster_mesh | 7 | ClusterMesh automation |
| commands/status | 1 | platform status |
| models/* | 7 | parse/serialize sdi, cluster, baremetal |
| Other | ~95 | facts, secrets, host_prepare, ssh, config |

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
|  facts -> sdi init -> cluster init        |
|  get baremetals/sdi-pools/clusters        |
|  secrets apply / status / kernel-tune     |
+-------------------------------------------+
        |
        v
_generated/
+-- facts/          (hardware JSON per node)
+-- sdi/            (OpenTofu HCL + state)
+-- clusters/       (inventory.ini + vars per cluster)
        |
        v
+-------------------------------------------+
|           gitops/ (ArgoCD)                |
|  bootstrap/spread.yaml                    |
|  generators/{tower,sandbox}/              |
|  common/ tower/ sandbox/                  |
+-------------------------------------------+
```
