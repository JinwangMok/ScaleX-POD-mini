# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status

- **Tests**: 287 pass / clippy 0 warnings / fmt clean
- **Code**: ~12,300 lines Rust, 27 source files
- **GitOps**: 41 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## 이전 DASHBOARD 비판적 분석

### 왜 이전 DASHBOARD가 Checklist를 달성했다고 볼 수 없는가?

#### 근본 원인 1: "PASS" 기준의 남용

이전 DASHBOARD는 **순수 함수 유닛 테스트 통과 = PASS**로 표기했다. 그러나:
- 순수 함수가 올바른 문자열을 생성하는 것과, 그 문자열이 실제 인프라에서 동작하는 것은 **별개의 문제**
- "CODE-ONLY"와 "PASS"의 경계가 모호하게 처리됨
- 특히 `sdi clean --hard` 같은 위험한 명령어의 로직 분기에 대한 테스트가 전무

#### 근본 원인 2: Sprint 8 방어적 설계

Sprint 8을 "물리 인프라 필요"라는 이유로 전체를 PENDING으로 묶어, 오프라인에서 가능한 개선을 모두 차단했다:
- `sdi clean` 로직 분기 테스트 (confirm 플래그, dry-run, 출력 디렉토리 존재 여부)
- `clean → rebuild` 사이클의 멱등성 dry-run 테스트
- cross-config 검증 (SDI spec의 pool_name이 k8s-clusters에서 참조될 때의 정합성)
- `scalex get config-files`의 상세 검증 로직 테스트

#### 근본 원인 3: E2E 파이프라인 불완전

기존 E2E dry-run 테스트(`test_e2e_full_pipeline_secrets_and_gitops`)는 `facts → sdi → cluster → secrets → gitops` 체인을 커버하지만:
- `clean → rebuild` 사이클은 테스트하지 않음
- `sdi clean --hard` 후 상태가 실제로 초기화되는지 검증 없음
- 노드 cleanup 스크립트의 내용 검증 부재

#### 근본 원인 4: 코드 결함은 수정했으나 미래 결함 방지 부재

Sprint 7에서 레거시 참조, root SCP, kubespray 경로 등을 수정했으나, 이러한 결함이 재발하지 않도록 하는 **방어적 테스트**가 일부만 존재 (탐지 테스트는 추가됨, 그러나 clean 명령 관련 방어 테스트 부재)

---

## Checklist Gap Analysis (정직한 재평가)

> 상태 기준:
> - **PASS**: 코드 + 테스트 + 로직 분기 모두 검증됨
> - **CODE-ONLY**: 코드 존재하나 로직 분기 테스트 또는 실환경 검증 부족
> - **PARTIAL**: 일부 구현됨, 구체적 미비 사항 명시
> - **FAIL**: 미구현 또는 코드 결함 존재
> - **INFRA-BLOCKED**: 물리 인프라 없이는 검증 불가능한 항목

### CL-1: 4개 노드 OpenTofu 가상화 + 2-클러스터 구조

**상태: PASS (순수 함수) / INFRA-BLOCKED (I/O 실행)**

| 항목 | 상태 | 비고 |
|------|------|------|
| SDI 모델 (SdiSpec, NodeSpec) | PASS | 파싱/직렬화 7 tests |
| OpenTofu HCL 생성 (순수 함수) | PASS | `tofu.rs` 8 tests — IP 기반 SSH URI |
| sdi-specs.yaml 예제 (4노드, 2풀) | PASS | tower + sandbox 풀 정의 |
| baremetal-init.yaml 스키마 | PASS | direct/external IP/ProxyJump 3가지 지원, camelCase |
| SDI spec 시맨틱 검증 | PASS | `validate_sdi_spec()` 5 tests |
| 단일 노드 SDI 검증 | PASS | `test_single_node_sdi_tower_and_sandbox_on_one_host` |
| `sdi init` (no flag) 리소스 풀 뷰 | PASS | JSON 생성 + 테이블 출력 |
| `sdi init <spec>` I/O 실행 | INFRA-BLOCKED | tofu apply 코드 존재, 실환경 미검증 |

### CL-2: Cloudflare Tunnel ArgoCD/GitOps 방식 — **PASS**

- `gitops/tower/cloudflared-tunnel/` — Helm kustomization + values.yaml
- `playbox-admin-static` 터널 이름 설정됨
- sync wave 3 자동 배포

### CL-3: Cloudflare Tunnel 설정 완료 여부

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| GitOps 배포 자동화 (YAML) | PASS | kustomization 존재 |
| 사용자 매뉴얼 가이드 | PASS | `docs/ops-guide.md` |
| 외부 kubectl 접근 검증 | INFRA-BLOCKED | 실환경 검증 필요 |
| 사용자 필수 작업 목록 | PASS | CF WebUI 터널 생성 + secrets.yaml 작성 가이드 |

### CL-4: Rust CLI + FP 스타일 — **PASS**

| 항목 | 상태 | 비고 |
|------|------|------|
| Rust 구현 | PASS | 287 tests, 0 clippy warnings |
| 순수 함수 패턴 | PASS | HCL/inventory/validation 모두 side-effect 없음 |
| 레거시 참조 제거 | PASS | Sprint 7.1 완료, 탐지 테스트 존재 |
| kubespray 경로 탐색 | PASS | Sprint 7.2: `kubespray/` 서브모듈 우선 |
| kubeconfig 수집 보안 | PASS | Sprint 7.3: `ssh_user` 필드 사용 |

### CL-5: 사용자 친절 가이드 — **PASS**

- ops-guide.md, SETUP-GUIDE.md, ARCHITECTURE.md, TROUBLESHOOTING.md
- `validate_baremetal_config()` 뉴비 친화적 에러 메시지

### CL-6: README.md 디테일 — **PASS**

- 설계 철학 7원칙, Installation Guide (Step 0-8), Architecture, CLI Reference, GitOps Pattern

### CL-7: README Installation Guide 검증

**상태: PASS (오프라인 검증) / INFRA-BLOCKED (실환경 E2E)**

- Step 0-8 작성됨
- Sprint 8d: 참조 파일 존재, example config 파싱, bootstrap 파일, docs 파일 존재 모두 자동 검증 (4 tests)
- 오프라인 검증 가능: config 파싱, dry-run 파이프라인 ✓
- 실제 E2E 실행 검증: INFRA-BLOCKED

### CL-8: CLI 기능 완전성

**상태: PASS (Sprint 8a 완료)**

| 명령어 | 순수 함수 테스트 | I/O 코드 | 로직 분기 테스트 | 결함 |
|--------|-----------------|----------|----------------|------|
| `scalex facts` | PASS (4 tests) | 존재 | N/A | — |
| `scalex sdi init` (no flag) | PASS (8 tests) | 존재 | N/A | — |
| `scalex sdi init <spec>` | PASS | 존재 | N/A | — |
| `scalex sdi clean --hard` | PASS | 존재 | PASS | Sprint 8a: validate_clean_args + plan_clean_operations 11 tests |
| `scalex sdi sync` | PASS (13 tests) | 존재 | N/A | Sprint 8c: 엣지 케이스 6 tests 추가 |
| `scalex cluster init` | PASS (11 tests) | 존재 | N/A | — |
| `scalex get *` | PASS (18 tests) | 존재 | N/A | — |
| `scalex secrets apply` | PASS (12 tests) | 존재 | N/A | — |
| `scalex status` | PASS (21 tests) | 존재 | N/A | — |
| `scalex kernel-tune` | PASS (14 tests) | 순수 | N/A | — |

### CL-9: 베어메탈 모드 확장성 — **PASS**

- `ClusterMode::Baremetal` enum + inventory 생성 테스트
- k3s 참조 없음 확인 테스트
- Sprint 8c: 단일 노드 dual-role + control-plane 없음 거부 검증

### CL-10: 보안 정보 템플릿화 — **PASS**

- `.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`
- `.gitignore` 보호, E2E 시크릿 분리 검증

### CL-11: 커널 파라미터 튜닝 — **PASS**

- `scalex kernel-tune` 14 tests
- `docs/ops-guide.md` 가이드

### CL-12: 디렉토리 구조 — **PASS**

- 필수 디렉토리 존재: scalex-cli/, gitops/, credentials/, config/, docs/
- 레거시 파일 삭제 확인 테스트 존재

### CL-13: 멱등성

**상태: PASS (Sprint 8a 완료)**

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL/inventory/cluster-vars 멱등성 | PASS | 동일 입력 → 동일 출력 테스트 |
| E2E 파이프라인 멱등성 | PASS | Sprint 7.4 추가 |
| clean→rebuild 멱등성 | PASS | Sprint 8a: byte-for-byte 출력 비교 테스트 |

### CL-14: Cloudflare Tunnel 가이드 + 외부 kubectl — **PASS (문서) / INFRA-BLOCKED (검증)**

- ops-guide.md에 터널 이름 `playbox-admin-static` 반영
- Pre-Keycloak kubectl 접근 가이드 포함
- 실제 외부 접근 검증: INFRA-BLOCKED

### CL-15: NAT 접근 방법 — **PASS**

- Cloudflare Tunnel + Tailscale + LAN 직접 접근 비교표
- ops-guide.md에 상세 가이드

---

## Sprint History

### Sprint 7: 코드 결함 수정 + 테스트 강화 — DONE (250 tests)

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 7.1 | 레거시 참조 제거 + 탐지 테스트 | CL-4, CL-12 | DONE |
| 7.2 | kubespray 서브모듈 경로 우선 | CL-4 | DONE |
| 7.3 | kubeconfig SCP 보안 개선 | CL-4 | DONE |
| 7.4 | E2E dry-run 파이프라인 통합 테스트 | CL-8, CL-10, CL-13 | DONE |
| 7.5 | baremetal-init.yaml camelCase 호환성 | CL-8 | DONE |

### Sprint 7b: ssh_user 보안 완성 — DONE (253 tests)

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 7b.1 | `ClusterDef`에 `ssh_user` 필드 추가 | CL-4 | DONE |
| 7b.2 | 예제 설정에 `ssh_user` 문서화 | CL-5, CL-6 | DONE |
| 7b.3 | ssh_user 전파 + 엣지 케이스 테스트 | CL-8 | DONE |

### Sprint 8a: 오프라인 테스트 강화 — DONE (271 tests)

> 목표: 물리 인프라 없이 검증 가능한 모든 로직 분기와 cross-config 정합성을 TDD로 커버

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 8a.1 | `sdi clean --hard` 로직 분기 테스트 (validate_clean_args + plan_clean_operations, 11 tests) | CL-8, CL-13 | DONE |
| 8a.2 | `clean → rebuild` 멱등성 E2E dry-run 테스트 (byte-for-byte 비교) | CL-13 | DONE |
| 8a.3 | SDI↔K8s cross-config 검증 (pool refs, Cilium IDs, CIDR overlap, DNS uniqueness) | CL-1, CL-8 | DONE |
| 8a.4 | `scalex get config-files` 상세 검증 테스트 | CL-8 | DONE |
| 8a.5 | 노드 cleanup 스크립트 내용 검증 (기존 host_prepare.rs 테스트로 충분 확인) | CL-8, CL-13 | DONE |

### Sprint 8c: 엣지 케이스 + 2-Layer 정합성 — DONE (283 tests)

> 목표: sync/baremetal 엣지 케이스 커버 + Infrastructure↔GitOps 레이어 간 정합성 검증

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 8c.1 | Sync 동시 추가+삭제, 전체 삭제, 완전 교체, 빈 입력 엣지 케이스 (6 tests) | CL-8, CL-13 | DONE |
| 8c.2 | Baremetal 단일 노드 dual-role + control-plane 없음 거부 검증 (2 tests) | CL-9 | DONE |
| 8c.3 | VM 충돌: 다중 호스트 제거 + 빈 제거 목록 (2 tests) | CL-8 | DONE |
| 8c.4 | 2-Layer 정합성: SDI 호스트⊆BM 노드, gitops 디렉토리 존재, tower_manages 유효성, CP 노드 존재 (4 tests) | CL-1, CL-12 | DONE |

### Sprint 8d: README/문서 검증 + 정확성 개선 — DONE (287 tests)

> 목표: README Installation Guide의 참조 파일/경로/문서가 실제로 존재하고 파싱 가능한지 자동 검증

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 8d.1 | README 참조 credential/config example 파일 존재 검증 테스트 | CL-6, CL-7 | DONE |
| 8d.2 | 모든 example YAML 파싱 성공 검증 테스트 (sdi, k8s, baremetal) | CL-7 | DONE |
| 8d.3 | gitops bootstrap spread.yaml 존재 검증 | CL-7, CL-12 | DONE |
| 8d.4 | README 문서 섹션 참조 docs 파일 존재 검증 | CL-5, CL-6 | DONE |
| 8d.5 | README 테스트 카운트 업데이트 + argocd config 경로 수정 | CL-6 | DONE |

### Sprint 8b: 실환경 검증 (물리 인프라 필요) — PENDING

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 8b.1 | `scalex facts --all` 실행 (4노드) | CL-1, CL-8 | TODO |
| 8b.2 | `scalex sdi init sdi-specs.yaml` 실행 | CL-1 | TODO |
| 8b.3 | `scalex cluster init k8s-clusters.yaml` 실행 | CL-8 | TODO |
| 8b.4 | `scalex secrets apply` + GitOps bootstrap | CL-8 | TODO |
| 8b.5 | 외부망 `kubectl get pods` 접근 검증 (CF Tunnel) | CL-3, CL-14 | TODO |
| 8b.6 | `scalex sdi clean --hard` + 재구축 (멱등성) | CL-13 | TODO |

---

## Test Summary (287 tests — Sprint 8d 기준)

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 57 | pool mapping, cluster IDs, CIDR overlap, DNS uniqueness, legacy detection, single-node, baremetal, idempotency, E2E pipeline, clean→rebuild, cross-config, sync, 2-layer consistency, **README verification (example files, config parsing, bootstrap, docs)** |
| core/gitops | 36 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generator consistency |
| core/kubespray | 32 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, extra vars, **single-node dual-role, no-CP rejection** |
| commands/status | 21 | platform status reporting |
| commands/sdi | 19 | network resolve, host infra, pool state, clean arg validation, plan_clean_operations |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| core/sync | 13 | sync diff, VM conflict detection, **simultaneous add+remove, empty desired, complete replacement, multi-host removal** |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM install, bridge setup, VFIO config, cleanup script validation |
| commands/cluster | 11 | cluster init, SDI/baremetal modes, gitops, ssh_user |
| core/tofu | 8 | HCL gen, IP-based SSH URI, VFIO, idempotency |
| models/* | 8 | parse/serialize sdi, cluster, baremetal |
| core/resource_pool | 5 | aggregation, table format |
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
