# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status (Sprint 12e COMPLETE — 2026-03-11)

- **Tests**: 322 pass / clippy 0 warnings / fmt clean
- **Code**: ~14,000 lines Rust, 29 source files
- **GitOps**: 42 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)
- **Last stable commit**: Sprint 12e — 322 tests
- **All offline GAPs resolved** (G-1~G-6) + 3rd cluster extensibility verified. Next: Sprint 12c (물리 인프라 E2E)

---

## 이전 DASHBOARD의 근본적 한계 — 비판적 분석

### 1. "검증 극장" (Verification Theater)

이전 DASHBOARD(Sprint 11b)는 315개 테스트와 "COMPLETE" 상태를 보고했으나, **모든 테스트가 dry-run/unit 테스트**였다. 실제 물리 인프라에서 단 한 번도 실행된 적이 없다. "315 tests passing"이라는 수치가 "프로젝트가 동작한다"는 착각을 만들었다.

**실제 테스트 분포:**
- 순수 함수 테스트 (파싱, 생성, 검증): ~290개
- 통합 경로 dry-run 테스트: ~25개
- 실제 I/O 테스트 (SSH, tofu apply, kubespray): **0개**

### 2. "CODE EXISTS ≠ WORKS" 오류

체크리스트는 **"동작하는가?"**를 묻고 있으나, 이전 분석은 **"코드가 존재하는가?"**만 확인했다. 예를 들어:
- `scalex facts`: 파싱 로직은 테스트했으나 **실제 SSH로 노드에 접속하여 정보를 수집하는 것**은 미검증
- `scalex sdi init <spec>`: HCL 생성은 테스트했으나 **tofu apply로 VM이 생성되는 것**은 미검증
- `scalex cluster init`: inventory/vars 생성은 테스트했으나 **kubespray로 클러스터가 프로비저닝되는 것**은 미검증

### 3. Checklist 항목의 의미론적 누락

- CL-9 (베어메탈 확장성): `ClusterMode::Baremetal` enum은 존재하나, **baremetal 모드에서의 전체 파이프라인 dry-run E2E 테스트가 없음**
- CL-14 (외부 kubectl): Cloudflare tunnel values.yaml은 존재하나, **라우팅 규칙이 문서화된 도메인과 일치하는지 검증하는 테스트가 없음**
- CL-1 (단일 노드): `test_single_node_sdi_*` 테스트는 존재하나, **단일 노드에서 전체 파이프라인이 동작하는지의 E2E dry-run은 없음**

### 4. 구조적 불필요 파일

- `PROMPT.md` (250줄): 체크리스트 + 철학. DASHBOARD.md와 내용 중복
- `DIRECTION.md` (55줄): 프로젝트 방향. DASHBOARD.md에 통합 가능
- 이 파일들은 참고 자료로서의 가치가 있으나, CL-12 "불필요한 내용이 포함되어 있지는 않은가?"에 해당

---

## Checklist 심층 검증 (Sprint 12a)

### CL-1: 4개 노드 OpenTofu 가상화 + 리소스 풀 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 (multi-host libvirt) | TESTED | `generate_tofu_main()` — 12 tests (단일/다중 호스트, VFIO, 멱등성) |
| Host-infra HCL 생성 (no-spec 경로) | TESTED | `generate_tofu_host_infra()` — 4 tests (단일/다중 노드, SSH user, 멱등성) |
| KVM/bridge/VFIO 설치 스크립트 | TESTED | `host_prepare` — 12 tests |
| `sdi init` (no flag) 전체 오케스트레이션 | PARTIAL | 코드 존재(facts확인→config로드→호스트준비→HCL생성→tofu apply), **but 오케스트레이션 흐름 자체의 테스트 없음** |
| `sdi init <spec>` VM 생성 | CODE ONLY | HCL 생성 테스트만, **실제 tofu apply 미검증** |
| 리소스 풀 관측 (`get sdi-pools`) | TESTED | `resource_pool_to_rows()` — 2 tests |
| **단일 노드 E2E dry-run** | **MISSING** | 단일 노드 SDI 파이프라인 전체 dry-run 테스트 없음 |
| **실제 4노드 tofu apply** | **NEVER** | 물리 인프라 필요 |

### CL-2: Cloudflare tunnel — ArgoCD GitOps

| 항목 | 상태 | 근거 |
|------|------|------|
| GitOps YAML | OK | `gitops/tower/cloudflared-tunnel/` (values.yaml + kustomization.yaml) |
| ApplicationSet 등록 | OK | `tower-generator.yaml` — syncWave: "3" |
| tunnel name 일치 | OK | `values.yaml`: `playbox-admin-static` |
| **라우팅 규칙 ↔ 문서 정합성 검증** | **MISSING** | values.yaml의 ingress 규칙이 docs의 도메인과 일치하는지 테스트 없음 |

### CL-3: Cloudflare tunnel 완료 + 사용자 작업

| 항목 | 상태 | 근거 |
|------|------|------|
| 사용자 수동 작업 문서화 | OK | `docs/ops-guide.md` Section 1 (6단계), `REQUEST-TO-USER.md` |
| credentials 템플릿 | OK | `credentials/cloudflare-tunnel.json.example` |
| `scalex secrets apply` | OK | tunnel-credentials Secret 생성 |
| **실제 동작 검증** | **NEVER** | 물리 인프라 필요 |

### CL-4: CLI — Rust + FP 스타일

| 항목 | 상태 | 근거 |
|------|------|------|
| Rust 구현 | OK | 29 source files, clap derive, thiserror |
| Pure functions (I/O 분리) | OK | 모든 generator/validator는 순수 함수 |
| Clippy 0 warnings | OK | CI 수준 검증 |
| Immutability | OK | 입력 데이터 변경 없음, 결과만 반환 |

### CL-5: 사용자 친절한 가이드

| 항목 | 상태 | 근거 |
|------|------|------|
| README Installation Guide (Step 0~8) | OK | 단계별 가이드 + 트러블슈팅 |
| Pre-flight SSH 점검 | OK | Step 1.5 |
| 에러 메시지 뉴비 친화적 | OK | `validate_baremetal_config()` — 구체적 해결방법 포함 |

### CL-6: README 상세 내용

| 항목 | 상태 | 근거 |
|------|------|------|
| Architecture/Philosophy/CLI/GitOps/Structure | OK | 482줄, 모두 포함 |
| **README 참조 파일 존재 검증** | **MISSING** | README에서 참조하는 파일들이 실제로 존재하는지 테스트 없음 |

### CL-7: Installation Guide E2E 보장

| 항목 | 상태 | 근거 |
|------|------|------|
| Step 0~8 문서 | OK | README에 포함 |
| **전체 E2E 실행** | **NEVER** | 물리 인프라 필요. 오프라인에서는 dry-run 경로만 검증 가능 |

### CL-8: CLI 기능 완성도

| 기능 | 코드 | 테스트 | 실제검증 |
|------|------|--------|----------|
| `scalex facts` | OK | 4 tests (파싱) | NEVER |
| `scalex sdi init` (no flag) | OK | 간접 테스트만 | NEVER |
| `scalex sdi init <spec>` | OK | 12 tests (HCL 생성) | NEVER |
| `scalex sdi clean --hard --yes-i-really-want-to` | OK | 4 tests (clean args) | NEVER |
| `scalex sdi sync` | OK | 13 tests (diff/conflict) | NEVER |
| `scalex cluster init <config>` | OK | 11 tests (inventory/vars) | NEVER |
| `scalex get baremetals` | OK | 3 tests | N/A (순수 파싱) |
| `scalex get sdi-pools` | OK | 4 tests | N/A (순수 파싱) |
| `scalex get clusters` | OK | 3 tests | N/A (순수 파싱) |
| `scalex get config-files` | OK | 6 tests | N/A (순수 파싱) |

### CL-9: 베어메탈 직접 사용 확장성

| 항목 | 상태 | 근거 |
|------|------|------|
| `ClusterMode::Baremetal` enum | OK | `models/cluster.rs` |
| `generate_inventory_baremetal()` | OK | 4 tests |
| No k3s references | OK | `test_no_k3s_references_in_project_files` |
| **Baremetal 모드 E2E dry-run** | **MISSING** | config→inventory→vars→gitops 전체 파이프라인 테스트 없음 |

### CL-10: 보안 정보 관리

| 항목 | 상태 | 근거 |
|------|------|------|
| `credentials/` gitignored | OK | `.gitignore` 확인 |
| `.example` 템플릿 4종 | OK | baremetal-init, .env, secrets, cloudflare-tunnel |
| `scalex secrets apply` | OK | K8s Secret YAML 생성 — 12 tests |

### CL-11: 커널 파라미터 튜닝

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex kernel-tune` | OK | 14 tests, 역할별 권장값 |
| docs 가이드 | OK | `docs/ops-guide.md` Section 3 |

### CL-12: 디렉토리 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex-cli/`, `gitops/{common,tower,sandbox}` | OK | 구조 일치 |
| Dead code 검증 | OK | `test_no_gitops_dead_code_directories` |
| **불필요 meta-files** | **GAP** | PROMPT.md, DIRECTION.md — DASHBOARD.md와 중복 |

### CL-13: 멱등성

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 멱등성 | TESTED | `test_generate_tofu_main_idempotent` |
| Host-infra HCL 멱등성 | TESTED | `test_generate_tofu_host_infra_idempotent` |
| Inventory 멱등성 | TESTED | `test_generate_inventory_idempotent` |
| Cluster-vars 멱등성 | TESTED | `test_generate_cluster_vars_idempotent` |
| Clean→rebuild 멱등성 | TESTED | `test_e2e_clean_rebuild_idempotency` |
| **실제 인프라 멱등성** | **NEVER** | 물리 인프라 필요 |

### CL-14: 외부 kubectl 접근

| 항목 | 상태 | 근거 |
|------|------|------|
| CF Tunnel ingress 설정 | CONFIG OK | `values.yaml` — `api.k8s.jinwang.dev` |
| SOCKS5 proxy | CONFIG OK | `socks5-proxy/manifest.yaml` |
| kubeconfig 생성 가이드 | OK | `client/` + `docs/ops-guide.md` |
| **실제 외부 접근** | **NEVER** | 물리 인프라 필요 |

### CL-15: NAT 접근 방법

| 항목 | 상태 | 근거 |
|------|------|------|
| Tailscale / CF Tunnel / LAN 비교표 | OK | `docs/ops-guide.md` Section 4 |
| LAN 스위치 접근 가이드 | OK | Section 4에 포함 |

---

## 오프라인 GAP 목록 (해결 가능)

| # | GAP | Checklist | 해결 방법 |
|---|-----|-----------|-----------|
| G-1 | Baremetal 모드 E2E dry-run 테스트 없음 | CL-9 | config→inventory→vars→gitops 전체 파이프라인 테스트 추가 |
| G-2 | CF tunnel 라우팅 ↔ 문서 도메인 정합성 테스트 없음 | CL-2, CL-14 | values.yaml ingress 규칙 검증 테스트 추가 |
| G-3 | 단일 노드 SDI 전체 파이프라인 dry-run 테스트 없음 | CL-1 | 단일 호스트에서 tower+sandbox 생성 E2E 테스트 추가 |
| G-4 | `sdi init` no-spec 오케스트레이션 흐름 테스트 없음 | CL-1, CL-8 | facts→host_prep→resource_pool→host_infra_hcl 전체 흐름 테스트 |
| G-5 | README 참조 파일 존재 검증 테스트 없음 | CL-6 | README 내 참조되는 파일 경로 실존 여부 테스트 |
| G-6 | 불필요 meta-files (PROMPT.md, DIRECTION.md, REQUEST-TO-USER.md) | CL-12 | DASHBOARD.md에 통합 후 삭제 |

## 오프라인 해결 불가 항목 (물리 인프라 필요)

| # | 항목 | 필요 조건 |
|---|------|-----------|
| E-1 | `tofu apply` 실제 실행 (CL-1) | 4개 bare-metal 노드 + KVM/libvirt |
| E-2 | `kubespray` 실제 실행 (CL-7) | SDI VM들이 생성된 상태 |
| E-3 | GitOps bootstrap (CL-7) | K8s 클러스터가 동작하는 상태 |
| E-4 | CF Tunnel 외부 접근 (CL-14) | ArgoCD + cloudflared Pod 동작 상태 |
| E-5 | 멱등성 실증 (CL-13) | clean → rebuild 실제 실행 |
| E-6 | `sdi sync` 실증 | 노드 추가/제거 시나리오 |

---

## Sprint Plan

### Sprint 12a: 테스트 강화 + 오프라인 GAP 해소 (TDD)

> **목표**: G-1~G-5 해결. 체크리스트의 오프라인 검증 가능 항목을 모두 테스트로 커버.

| # | Task | GAP | TDD 테스트 | 상태 |
|---|------|-----|-----------|------|
| 12a-1 | Baremetal 모드 E2E dry-run | G-1 | `test_baremetal_mode_e2e_pipeline` | DONE |
| 12a-2 | CF tunnel 라우팅 검증 | G-2 | `test_cloudflare_tunnel_routes_match_docs` | DONE |
| 12a-3 | 단일 노드 SDI 전체 파이프라인 | G-3 | `test_single_node_sdi_full_pipeline` | DONE |
| 12a-4 | `sdi init` no-spec 오케스트레이션 | G-4 | `test_sdi_init_no_spec_full_orchestration` | DONE |
| 12a-5 | README 참조 파일 검증 | G-5 | `test_readme_referenced_files_exist` | DONE |
| 12a-6 | 전체 테스트 통과 + Commit + Push | | | DONE |

### Sprint 12b: 구조 정리

| # | Task | GAP | 상태 |
|---|------|-----|------|
| 12b-1 | PROMPT.md/DIRECTION.md/REQUEST-TO-USER.md → DASHBOARD.md 통합 | G-6 | DONE |
| 12b-2 | 3개 meta-files 삭제 | G-6 | DONE |
| 12b-3 | Commit + Push | | DONE |

### Sprint 12c: 실환경 E2E (물리 인프라 필요)

| # | Task | 상태 |
|---|------|------|
| 12c-1 | `scalex facts --all` 실행 (4노드) | TODO |
| 12c-2 | `scalex sdi init` (no flag) | TODO |
| 12c-3 | `scalex sdi init config/sdi-specs.yaml` | TODO |
| 12c-4 | `scalex cluster init config/k8s-clusters.yaml` | TODO |
| 12c-5 | `scalex secrets apply` | TODO |
| 12c-6 | GitOps bootstrap + ArgoCD 동작 확인 | TODO |
| 12c-7 | 외부 kubectl 접근 (CF Tunnel) 검증 | TODO |
| 12c-8 | `sdi clean --hard` + 재구축 (멱등성) | TODO |

---

## User Action Required (수동 작업 필요 항목)

자동화할 수 없는 사용자 고유 환경 정보가 필요한 항목:

### 1. Credentials & Config Files

```bash
cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml
cp credentials/.env.example credentials/.env
cp credentials/secrets.yaml.example credentials/secrets.yaml
cp config/sdi-specs.yaml.example config/sdi-specs.yaml
cp config/k8s-clusters.yaml.example config/k8s-clusters.yaml
```

실제 노드 IP, SSH 자격증명, 서비스 비밀번호로 편집. `scalex get config-files`로 검증.

### 2. Cloudflare Tunnel WebUI Setup

Cloudflare Zero Trust 대시보드에서 터널 생성 + Public Hostname 설정 필요.
- Hostnames: `api.k8s.jinwang.dev`, `auth.jinwang.dev`, `cd.jinwang.dev`
- Guide: `docs/ops-guide.md` Section 1
- 향후 `scalex tunnel init` 명령으로 자동화 검토 가능 (Cloudflare API 토큰 필요)

### 3. Keycloak Realm/Client Configuration

Keycloak 관리 콘솔에서 Realm(`kubernetes`), Client(`kubernetes`), Group(`cluster-admins`) 수동 생성.
- Guide: `docs/ops-guide.md` Section 2
- 자동화 후보: `KeycloakRealmImport` CRD를 `gitops/tower/keycloak/`에 추가하면 ArgoCD 자동 배포 가능

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
|  get baremetals/sdi-pools/clusters        |
|  secrets apply / status / kernel-tune     |
+-------------------------------------------+
        |
        v
_generated/
├── facts/          (hardware JSON per node)
├── sdi/            (OpenTofu HCL + state + resource-pool.json)
└── clusters/       (inventory.ini + vars per cluster)
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
| core/validation | 65 | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave, AppProject, sdi-init resource pool, E2E pipeline, SSH |
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
| core/resource_pool | 5 | aggregation, table |
| commands/facts | 4 | facts gathering |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| core/validation (12a) | 5 | baremetal E2E, CF tunnel routes, single-node SDI, sdi-init orchestration, README refs |
| core/validation (12d) | 1 | 3rd cluster extensibility (tower+sandbox+datax) pipeline |
| core/validation (12e) | 1 | GitOps structure ↔ k8s-clusters.yaml consistency |
| **TOTAL** | **322** | |

---

## Sprint History

| Sprint | Date | Tests | Summary |
|--------|------|-------|---------|
| 12e | 2026-03-11 | 322 | GitOps structure ↔ cluster config consistency test |
| 12d | 2026-03-11 | 321 | 3rd cluster extensibility test (tower+sandbox+datax pipeline) |
| 12b | 2026-03-11 | 320 | Meta-file cleanup (PROMPT.md, DIRECTION.md, REQUEST-TO-USER.md → DASHBOARD.md) |
| 12a | 2026-03-11 | 320 | Gap verification tests: baremetal E2E, CF tunnel, single-node SDI, orchestration flow, README refs |
| 11b | 2026-03-11 | 315 | E2E pipeline + SSH integration tests |
| 11a | 2026-03-11 | 313 | DASHBOARD rewrite + sdi init resource pool verification |
| 10a | 2026-03-11 | 308 | 9 bugs fixed (B-1~B-9) |
| 9a | 2026-03-11 | 301 | Sprint 9a final |
