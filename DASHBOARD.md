# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status

- **Tests**: 244 pass / clippy 0 warnings / fmt clean
- **Code**: ~11,700 lines Rust, 27 source files
- **GitOps**: 33 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)
- **Repo URL**: All GitOps YAMLs correctly reference `ScaleX-POD-mini.git`
- **Dead code**: 0 `#[allow(dead_code)]` — all validation functions wired into CLI

---

## Checklist Gap Analysis

> 상태: PASS = 완료 및 검증됨, PARTIAL = 부분 구현 (사유 명시), FAIL = 미구현 또는 미검증

### CL-1: 4개 노드 OpenTofu 가상화 + 2-클러스터 구조

**상태: PASS (오프라인) — 실환경 테스트만 남음**

| 항목 | 상태 | 비고 |
|------|------|------|
| SDI 모델 (SdiSpec, NodeSpec) | PASS | `models/sdi.rs` — 파싱/직렬화 테스트 통과 |
| OpenTofu HCL 생성 | PASS | `core/tofu.rs` — 호스트 인프라(IP 기반 SSH URI) + VM 생성 HCL 순수 함수 |
| sdi-specs.yaml 예제 (4노드, 2풀) | PASS | `config/sdi-specs.yaml.example` — tower + sandbox 풀 정의 |
| baremetal-init.yaml 스키마 (3가지 접근 방식) | PASS | direct / external IP / ProxyJump 모두 지원 |
| SDI spec 시맨틱 검증 | PASS | `validate_sdi_spec()` — 풀 이름/IP/VM 이름 중복, 리소스 범위, 역할 검증 |
| 단일 노드 SDI 검증 | PASS | `test_single_node_sdi_tower_and_sandbox_on_one_host` |
| `scalex sdi init` (no flag) 동작 | PASS | 리소스 풀 요약 JSON 생성 + 호스트 인프라 HCL |
| `scalex get sdi-pools` 통합 뷰 | PASS | `resource-pool-summary.json` 폴백 지원 |
| 실제 HW 테스트 | **FAIL** | 물리 인프라 접근 필요 — Sprint 5 |

### CL-2: Cloudflare Tunnel ArgoCD/GitOps 방식 — **PASS**

- `gitops/tower/cloudflared-tunnel/` — Helm 차트 + values.yaml + kustomization.yaml
- `playbox-admin-static` 터널 이름 설정
- sync wave 3으로 자동 배포

### CL-3: Cloudflare Tunnel 설정 완료 여부

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| GitOps 배포 자동화 | PASS | cloudflared-tunnel kustomization 존재 |
| 사용자 매뉴얼 가이드 | PASS | `docs/ops-guide.md` 터널 이름, Pre-OIDC kubectl 접근 가이드 포함 |
| 외부 kubectl 접근 검증 | **FAIL** | 실환경 검증 필요 — Sprint 5 |

### CL-4: Rust CLI + FP 스타일 — **PASS**

- 244 tests, 0 clippy warnings, fmt clean
- 순수 함수: HCL 생성, inventory 생성, validation 모두 side-effect 없음
- `#[allow(dead_code)]` 전량 제거 — 모든 validation 함수가 CLI 파이프라인에 통합됨
- `cluster init` 시 자동으로 cluster ID 중복, SDI pool 매핑, SDI spec 시맨틱 검증 수행

### CL-5: 사용자 친절 가이드 — **PASS**

- ops-guide.md: kernel-tune CLI 사용법, Pre-Keycloak kubectl 접근, Tailscale 설치/설정, 접근 방법 비교표
- SETUP-GUIDE.md, ARCHITECTURE.md, TROUBLESHOOTING.md 존재
- `validate_baremetal_config()` — 뉴비 친화적 에러 메시지 (누락 필드, 잘못된 참조 등)

### CL-6: README.md 디테일 — **PASS**

- 설계 철학 7원칙 섹션
- Installation Guide (Step 0-8)
- Architecture, CLI Reference, GitOps Pattern, Project Structure 포함
- 모든 repo URL `ScaleX-POD-mini.git`으로 정합

### CL-7: README Installation Guide — **PASS (오프라인)**

- Step 0-8 단계별 Installation Guide 작성
- 실환경 end-to-end 검증은 Sprint 5

### CL-8: CLI 기능 완전성

**상태: PASS (오프라인 검증 완료)**

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | PASS | SSH로 HW 정보 수집 |
| `scalex sdi init` (no flag) | PASS | 리소스 풀 요약 + 호스트 인프라 HCL 생성 |
| `scalex sdi init <spec>` | PASS | SDI 풀 생성 HCL + VM 프로비저닝 |
| `scalex sdi clean --hard` | PASS | 전체 초기화 로직 |
| `scalex sdi sync` | PASS | `sync::compute_sync_diff` + `sync::detect_vm_conflicts` 사용 |
| `scalex cluster init` | PASS | Kubespray inventory/vars 생성, OIDC, cross-config 검증 통합 |
| `scalex get baremetals` | PASS | facts JSON → 테이블 |
| `scalex get sdi-pools` | PASS | VM 풀 + 베어메탈 리소스 풀 통합 뷰 |
| `scalex get clusters` | PASS | 클러스터 인벤토리 |
| `scalex get config-files` | PASS | 설정 파일 검증 (`classify_config_status` 순수 함수) |
| `scalex secrets apply` | PASS | K8s secret 생성 |
| `scalex status` | PASS | 5-layer 상태 |
| `scalex kernel-tune` | PASS | 커널 파라미터 권장 (--role, --format, --diff-node) |
| 3번째 클러스터 확장성 | PASS | 3-pool/3-cluster 검증 + 중복 ID 거부 |

### CL-9: 베어메탈 모드 확장성 — **PASS**

- `ClusterMode::Baremetal` enum + inventory 생성 테스트
- k3s 완전 제거 확인

### CL-10: 보안 정보 템플릿화 — **PASS**

- `.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example` 존재
- `.gitignore`로 실제 credential 파일 보호

### CL-11: 커널 파라미터 튜닝 — **PASS**

- `scalex kernel-tune` CLI 명령 완전 구현 (14 tests)
- `docs/ops-guide.md` Section 3에 CLI 사용법 + 수동 튜닝 가이드

### CL-12: 디렉토리 구조 — **PASS**

- 모든 필수 디렉토리 존재 (scalex-cli, gitops, credentials, config, docs, ansible, kubespray, client, tests)
- `.legacy-*` 파일 삭제 완료 (커밋됨)
- 미추적 파일 (DIRECTION.md, PROMPT.md, REQUEST-TO-USER.md) — 작업 문서로 .gitignore 대상

### CL-13: 멱등성 — **PASS (오프라인)**

- HCL/inventory/cluster-vars 멱등성 테스트 (동일 입력 → 동일 출력)
- I/O 오케스트레이션 멱등성은 Sprint 5

### CL-14: Cloudflare Tunnel 가이드 + 외부 kubectl — **PASS (오프라인)**

- ops-guide.md 터널 이름 `playbox-admin-static` 반영
- Pre-Keycloak kubectl 접근 가이드 추가 (admin kubeconfig + CF Tunnel URL)
- 외부 kubectl 접근 실증은 Sprint 5

### CL-15: NAT 접근 방법 — **PASS**

- Cloudflare Tunnel (설치 불필요, CDN 경유)
- Tailscale VPN (설치/설정 가이드 + kubectl kubeconfig 예시 추가)
- LAN 직접 접근 (SSH ProxyJump, kubectl, 스위치 설정)
- 접근 방법 비교표 (ops-guide.md)

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
| 3.1 | `sdi init` (no flag) 통합 리소스 풀 상태 JSON 생성 | CL-1, CL-8 | ✅ DONE |
| 3.2 | `scalex get sdi-pools` 통합 뷰 (resource-pool-summary.json 폴백) | CL-8 | ✅ DONE |
| 3.3 | 단일 노드 전체 파이프라인 dry-run 테스트 | CL-1 | ✅ DONE |

### Sprint 4: 확장성 검증 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 4.1 | 3번째 클러스터 추가 테스트 (3-pool, 3-cluster) | CL-8 | ✅ DONE |
| 4.2 | 2-Layer 템플릿 관리 검증 | CL-6 | ✅ DONE |
| 4.3 | `scalex sdi sync` 사이드이펙트 테스트 | CL-8 | ✅ DONE |

### Sprint 5: Validation + 코드 품질 강화 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 5.1 | Baremetal config 시맨틱 검증 (7 tests) | CL-5, CL-8 | ✅ DONE |
| 5.2 | SDI spec 시맨틱 검증 (5 tests) | CL-8 | ✅ DONE |
| 5.3 | Validation 함수 CLI 파이프라인 통합 (cluster init) | CL-4, CL-8 | ✅ DONE |
| 5.4 | `#[allow(dead_code)]` 전량 제거, 코드 정리 | CL-4 | ✅ DONE |
| 5.5 | GitOps repo URL 정합 (k8s-playbox → ScaleX-POD-mini) | CL-6, CL-12 | ✅ DONE |
| 5.6 | sync 모듈 CLI 통합 (`sdi sync`가 순수 함수 사용) | CL-8 | ✅ DONE |
| 5.7 | `get config-files`가 `classify_config_status` 순수 함수 사용 | CL-4 | ✅ DONE |
| 5.8 | HCL 생성에 IP 기반 SSH URI 사용 | CL-1 | ✅ DONE |

### Sprint 6: 문서 보강 ✅ DONE

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 6.1 | ops-guide kernel-tune 스탈 참조 수정 | CL-11 | ✅ DONE |
| 6.2 | Pre-Keycloak kubectl 접근 가이드 추가 | CL-14 | ✅ DONE |
| 6.3 | Tailscale 설치/설정/kubectl 가이드 추가 | CL-15 | ✅ DONE |
| 6.4 | 접근 방법 비교표 추가 | CL-15 | ✅ DONE |
| 6.5 | DASHBOARD.md 전면 재작성 (정확한 테스트 수, 갭 분석) | — | ✅ DONE |

### Sprint 7: 실환경 검증 (물리 인프라 필요) — PENDING

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 7.1 | `scalex facts --all` 실행 (4노드) | CL-1, CL-8 | TODO |
| 7.2 | `scalex sdi init sdi-specs.yaml` 실행 | CL-1 | TODO |
| 7.3 | `scalex cluster init k8s-clusters.yaml` 실행 | CL-8 | TODO |
| 7.4 | `scalex secrets apply` + `kubectl apply -f gitops/bootstrap/spread.yaml` | CL-8 | TODO |
| 7.5 | 외부망에서 `kubectl get pods` 접근 검증 (CF Tunnel) | CL-3, CL-14 | TODO |
| 7.6 | `scalex sdi clean --hard` + 재구축 (멱등성) | CL-13 | TODO |

---

## Test Summary (244 tests)

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 39 | pool mapping, cluster IDs, CIDR, k3s, legacy, single-node, baremetal, idempotency, E2E, extensibility, sync, SDI spec, baremetal config |
| core/gitops | 36 | ApplicationSet, kustomization, sync waves, Cilium values, ClusterMesh, generator consistency, repo URL |
| core/kubespray | 30 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, extra vars |
| commands/status | 21 | platform status |
| commands/get | 18 | facts row, config status, SDI pools, clusters, resource pool rows |
| core/kernel | 14 | kernel-tune recommendations (roles, formats, diff) |
| core/config | 14 | baremetal config loading, semantic validation |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM install, bridge setup, VFIO config scripts |
| core/tofu | 8 | HCL gen (host infra + VM), IP-based SSH URI, VFIO, idempotency |
| commands/sdi | 8 | network resolve, host infra inputs, pool state |
| core/sync | 7 | compute_sync_diff, detect_vm_conflicts |
| commands/cluster | 7 | cluster init, SDI/baremetal modes, gitops update |
| models/* | 7 | parse/serialize sdi, cluster, baremetal |
| core/resource_pool | 5 | aggregation, multi-node, empty, table format, bridge detection |
| commands/facts | 4 | facts gathering, script building |
| core/ssh | 2 | SSH command building |

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
|                                           |
|  Validation Pipeline (pure functions):    |
|  validate_baremetal_config                |
|  validate_sdi_spec                        |
|  validate_cluster_sdi_pool_mapping        |
|  validate_unique_cluster_ids              |
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
