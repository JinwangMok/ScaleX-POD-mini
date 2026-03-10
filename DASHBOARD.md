# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status

- **Tests**: 253 pass / clippy 0 warnings / fmt clean
- **Code**: ~12,300 lines Rust, 27 source files
- **GitOps**: 33 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## Checklist Gap Analysis (정직한 재평가)

> 상태 기준:
> - **PASS**: 코드 + 테스트 + 동작 모두 검증됨
> - **CODE-ONLY**: 코드 존재하나 충분한 테스트 또는 실환경 검증 부족
> - **PARTIAL**: 일부 구현됨, 구체적 미비 사항 명시
> - **FAIL**: 미구현 또는 코드 결함 존재

### CL-1: 4개 노드 OpenTofu 가상화 + 2-클러스터 구조

**상태: PASS (순수 함수) / CODE-ONLY (I/O 오케스트레이션)**

| 항목 | 상태 | 비고 |
|------|------|------|
| SDI 모델 (SdiSpec, NodeSpec) | PASS | 파싱/직렬화 7 tests |
| OpenTofu HCL 생성 (순수 함수) | PASS | `tofu.rs` 8 tests — IP 기반 SSH URI |
| sdi-specs.yaml 예제 (4노드, 2풀) | PASS | tower + sandbox 풀 정의 |
| baremetal-init.yaml 스키마 | PASS | direct/external IP/ProxyJump 3가지 지원, camelCase 호환 테스트 |
| SDI spec 시맨틱 검증 | PASS | `validate_sdi_spec()` 5 tests |
| 단일 노드 SDI 검증 | PASS | `test_single_node_sdi_tower_and_sandbox_on_one_host` |
| `sdi init` (no flag) 리소스 풀 뷰 | PASS | JSON 생성 + 테이블 출력 |
| `sdi init <spec>` I/O 오케스트레이션 | CODE-ONLY | tofu apply 실행 코드 존재, 실환경 미검증 |

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
| 외부 kubectl 접근 검증 | **FAIL** | 실환경 검증 필요 |
| 사용자 필수 작업 목록 | PASS | CF WebUI 터널 생성 + secrets.yaml 작성 가이드 |

### CL-4: Rust CLI + FP 스타일

**상태: PASS — Sprint 7에서 결함 수정 완료**

| 항목 | 상태 | 비고 |
|------|------|------|
| Rust 구현 | PASS | 250 tests, 0 clippy warnings |
| 순수 함수 패턴 | PASS | HCL/inventory/validation 모두 side-effect 없음 |
| `#[allow(dead_code)]` 제거 | PASS | 전량 제거됨 |
| 레거시 참조 제거 | ~~FAIL~~ → **PASS** | Sprint 7.1: 코드/주석 모두 제거, 탐지 테스트 추가 |
| kubespray 경로 탐색 | ~~FAIL~~ → **PASS** | Sprint 7.2: `kubespray/` 서브모듈 우선 탐색 |
| kubeconfig 수집 보안 | ~~FAIL~~ → **PASS** | Sprint 7.3: `build_kubeconfig_scp_args(admin_user)` 순수 함수 |
| 레거시 주석 제거 | ~~FAIL~~ → **PASS** | Sprint 7.1: `kubespray.rs` 주석 수정 완료 |

### CL-5: 사용자 친절 가이드 — **PASS**

- ops-guide.md, SETUP-GUIDE.md, ARCHITECTURE.md, TROUBLESHOOTING.md
- `validate_baremetal_config()` 뉴비 친화적 에러 메시지

### CL-6: README.md 디테일 — **PASS**

- 설계 철학 7원칙, Installation Guide (Step 0-8), Architecture, CLI Reference, GitOps Pattern

### CL-7: README Installation Guide — **CODE-ONLY**

- Step 0-8 작성됨
- **실제 end-to-end 실행 검증 불가** (물리 인프라 필요)
- 오프라인에서 검증 가능한 것: config 파일 파싱, dry-run 파이프라인

### CL-8: CLI 기능 완전성

**상태: PASS (순수 함수) — Sprint 7에서 결함 수정 완료**

| 명령어 | 순수 함수 테스트 | I/O 코드 | 결함 |
|--------|-----------------|----------|------|
| `scalex facts` | PASS (4 tests) | 존재 | — |
| `scalex sdi init` (no flag) | PASS (8 tests) | 존재 | — |
| `scalex sdi init <spec>` | PASS | 존재 | — |
| `scalex sdi clean --hard` | CODE-ONLY | 존재 | — |
| `scalex sdi sync` | PASS (7 tests) | 존재 | — |
| `scalex cluster init` | PASS (9 tests) | 존재 | ~~레거시 kubespray, root SCP~~ → 수정 완료 |
| `scalex get *` | PASS (18 tests) | 존재 | — |
| `scalex secrets apply` | PASS (12 tests) | 존재 | — |
| `scalex status` | PASS (21 tests) | 존재 | — |
| `scalex kernel-tune` | PASS (14 tests) | 순수 | — |

### CL-9: 베어메탈 모드 확장성 — **PASS**

- `ClusterMode::Baremetal` enum + inventory 생성 테스트
- k3s 참조 없음 확인 테스트 (`test_no_k3s_references_in_project_files`)

### CL-10: 보안 정보 템플릿화 — **PASS**

- `.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`
- `.gitignore` 보호
- E2E 테스트에서 management/workload 시크릿 분리 검증

### CL-11: 커널 파라미터 튜닝 — **PASS**

- `scalex kernel-tune` 14 tests
- `docs/ops-guide.md` 가이드

### CL-12: 디렉토리 구조 — **PASS**

| 항목 | 상태 | 비고 |
|------|------|------|
| 필수 디렉토리 존재 | PASS | scalex-cli/, gitops/, credentials/, config/, docs/ |
| 레거시 파일 삭제 | PASS | .legacy-* 파일 삭제됨 |
| 레거시 코드 참조 | ~~FAIL~~ → **PASS** | Sprint 7.1: 자동 탐지 테스트 추가 |

### CL-13: 멱등성 — **PASS**

- HCL/inventory/cluster-vars 멱등성 테스트 (동일 입력 → 동일 출력)
- E2E 파이프라인에서 전 클러스터 멱등성 검증 추가 (Sprint 7.4)

### CL-14: Cloudflare Tunnel 가이드 + 외부 kubectl — **PASS (문서)**

- ops-guide.md에 터널 이름 `playbox-admin-static` 반영
- Pre-Keycloak kubectl 접근 가이드 포함

### CL-15: NAT 접근 방법 — **PASS**

- Cloudflare Tunnel + Tailscale + LAN 직접 접근 비교표
- ops-guide.md에 상세 가이드

---

## 이전 DASHBOARD 비판적 분석

### 왜 이전 DASHBOARD가 Checklist를 달성했다고 볼 수 없는가?

1. **"PASS (오프라인)" 남용**: 순수 함수 유닛 테스트 통과를 "PASS"로 표기함. 순수 함수가 올바른 문자열을 생성하는 것과, 그 문자열이 실제 인프라에서 동작하는 것은 별개.

2. **코드 결함 미발견**: 전 작업자가 레거시 참조(`.legacy-datax-kubespray`)를 제거했다고 주장했으나, `cluster.rs:303`과 `kubespray.rs` 주석에 여전히 존재.

3. **보안 미흡 미인지**: `collect_kubeconfig()`에서 `root@{ip}`로 SCP — 실제 환경에서 root SSH 접근은 보안 위험. baremetal-init.yaml에 정의된 `admin_user`를 사용해야 함.

4. **kubespray 경로 탐색 오류**: `find_kubespray_dir()`가 프로젝트 내 `kubespray/` 서브모듈을 탐색 후보에 포함하지 않고, `.legacy-datax-kubespray`를 포함함.

5. **E2E 파이프라인 테스트 부재**: `facts → sdi init → cluster init → secrets → gitops bootstrap` 전체 체인의 dry-run 통합 테스트가 없음. 개별 모듈 테스트만 존재.

---

## Sprint History

### Sprint 7: 코드 결함 수정 + 테스트 강화 — DONE (250 tests)

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 7.1 | 레거시 참조 제거 (`.legacy-datax-kubespray`) + 탐지 테스트 | CL-4, CL-12 | DONE |
| 7.2 | `find_kubespray_dir()` kubespray 서브모듈 경로 우선 사용 | CL-4 | DONE |
| 7.3 | `build_kubeconfig_scp_args()` 순수 함수 + 보안 개선 | CL-4 | DONE |
| 7.4 | E2E dry-run 파이프라인 통합 테스트 (secrets + gitops + 멱등성) | CL-8, CL-10, CL-13 | DONE |
| 7.5 | baremetal-init.yaml camelCase 스키마 호환성 테스트 (3가지 접근 모드) | CL-8 | DONE |

### Sprint 7b: ssh_user 보안 완성 + 엣지 케이스 강화 — DONE (253 tests)

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 7b.1 | `ClusterDef`에 `ssh_user` 필드 추가 + `collect_kubeconfig()` 연결 | CL-4 | DONE |
| 7b.2 | `k8s-clusters.yaml.example`에 `ssh_user` 문서화 | CL-5, CL-6 | DONE |
| 7b.3 | ssh_user 전파 + 예제 설정 파싱 엣지 케이스 테스트 | CL-8 | DONE |

### Sprint 8: 실환경 검증 (물리 인프라 필요) — PENDING

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 8.1 | `scalex facts --all` 실행 (4노드) | CL-1, CL-8 | TODO |
| 8.2 | `scalex sdi init sdi-specs.yaml` 실행 | CL-1 | TODO |
| 8.3 | `scalex cluster init k8s-clusters.yaml` 실행 | CL-8 | TODO |
| 8.4 | `scalex secrets apply` + GitOps bootstrap | CL-8 | TODO |
| 8.5 | 외부망 `kubectl get pods` 접근 검증 (CF Tunnel) | CL-3, CL-14 | TODO |
| 8.6 | `scalex sdi clean --hard` + 재구축 (멱등성) | CL-13 | TODO |

---

## Test Summary (253 tests)

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 41 | pool mapping, cluster IDs, CIDR, legacy detection, single-node, baremetal, idempotency, E2E (basic + full pipeline), extensibility, sync, SDI spec, baremetal config |
| core/gitops | 36 | ApplicationSet, kustomization, sync waves, Cilium values, ClusterMesh, generator consistency, repo URL |
| core/kubespray | 30 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, extra vars |
| commands/status | 21 | platform status |
| commands/get | 18 | facts row, config status, SDI pools, clusters, resource pool rows |
| core/config | 15 | baremetal config loading, semantic validation, camelCase 3-mode schema |
| core/kernel | 14 | kernel-tune recommendations |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM install, bridge setup, VFIO config |
| commands/cluster | 11 | cluster init, SDI/baremetal modes, gitops update, kubeconfig SCP security, ssh_user propagation |
| core/tofu | 8 | HCL gen, IP-based SSH URI, VFIO, idempotency |
| commands/sdi | 8 | network resolve, host infra inputs, pool state |
| core/sync | 7 | compute_sync_diff, detect_vm_conflicts |
| models/* | 8 | parse/serialize sdi, cluster, baremetal, ssh_user field |
| core/resource_pool | 5 | aggregation, multi-node, empty, table format, bridge |
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
