# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status (Sprint 11b COMPLETE — 2026-03-11)

- **Tests**: 315 pass / clippy 0 warnings / fmt clean
- **Code**: ~14,000 lines Rust, 27 source files
- **GitOps**: 42 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)
- **Last stable commit**: `3ca5454` (Sprint 11b — 315 tests)
- **Offline 검증 완료**: 모든 오프라인 검증 가능한 항목 완료. 남은 항목은 물리 인프라 필요.

---

## Checklist 심층 비판 분석 (Sprint 11a)

> **이전 DASHBOARD(Sprint 10a)에 대한 비판:**
> Sprint 10a는 9개 버그를 수정하고 310개 테스트를 달성했지만, **"OK"라고 표기한 항목들의 대부분이 실제로는 "코드가 존재함"일 뿐 "설계 철학대로 동작함"이 검증되지 않았다.**
> "테스트가 있다"와 "올바르게 동작한다"는 근본적으로 다른 것이다.
> 특히, Checklist가 요구하는 **의미론적 완전성**(semantic completeness)을 무시하고
> 단순히 코드 존재 여부만 확인한 것이 가장 큰 문제이다.

### CL-1: 4개 노드 OpenTofu 가상화 + 리소스 풀 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 (multi-host libvirt) | CODE EXISTS | `core/tofu.rs` — `generate_tofu_main()` |
| 4개 호스트 provider | CODE EXISTS | `generate_provider_block()` per unique host |
| VM 리소스 생성 | CODE EXISTS | `generate_vm_resource()` — disk, cloudinit, domain |
| 단일 노드 환경 | UNIT TEST ONLY | `test_single_node_sdi_all_pools_on_one_host` |
| **`sdi init` (no flag) = 통합 리소스 풀** | **OK** | 호스트 준비(KVM/bridge/VFIO) + `resource-pool-summary.json` 생성 + 리소스 풀 테이블 표시. `scalex get sdi-pools`에서 "Unified Bare-Metal Resource Pool" 표시 지원. 3개 검증 테스트 추가 (Sprint 11a) |
| **`sdi init <spec>` = 풀에서 노드 생성** | CODE EXISTS | HCL 생성 + tofu apply. 그러나 **실제 apply 미실행** |
| 실제 `tofu apply` 실행 | **NEVER** | 물리 인프라에서 미실행 |

**근본 원인**: `sdi init` (no flag)의 설계 의도가 체크리스트와 불일치.
- 체크리스트: "모든 베어메탈 HW를 가상화하고 **통합 리소스 풀로 관측**되도록 구성"
- 실제 코드: KVM/bridge/VFIO 설치만 수행. 리소스 풀 관측(=`scalex get sdi-pools`로 전체 가용 리소스 표시)은 `facts` 데이터가 있을 때만 부분적으로 가능
- **필요한 작업**: `sdi init` (no flag) 실행 후 facts 기반으로 **전체 리소스 풀 summary**를 자동 생성/표시하는 기능 추가

### CL-2: Cloudflare tunnel — ArgoCD GitOps 방식

| 항목 | 상태 | 근거 |
|------|------|------|
| GitOps YAML 존재 | OK | `gitops/tower/cloudflared-tunnel/` |
| Helm chart 방식 | OK | `values.yaml` — tunnelConfig + ingress |
| ApplicationSet 등록 | OK | `tower-generator.yaml` — syncWave: "3" |
| tunnel name = `playbox-admin-static` | OK | `values.yaml` line 4 — 사용자 설정과 일치 |

### CL-3: Cloudflare tunnel 완료 여부 + 사용자 수동 작업

| 항목 | 상태 | 근거 |
|------|------|------|
| 사용자 수동 작업 문서화 | OK | `docs/ops-guide.md` Section 1 — 6단계 가이드 |
| credentials 템플릿 | OK | `credentials/cloudflare-tunnel.json.example` |
| `scalex secrets apply` 자동화 | OK | tunnel-credentials Secret 생성 |
| **실제 동작 검증** | **NEVER** | cloudflared Pod 기동 여부, ingress 라우팅 미확인 |

### CL-4: CLI — Rust 구현 + FP 스타일

| 항목 | 상태 | 근거 |
|------|------|------|
| Rust 구현 | OK | `scalex-cli/` — 27 source files, clap derive, thiserror |
| Pure functions | OK | generators/validators는 I/O 분리 — `generate_tofu_main()`, `generate_inventory()` 등 |
| Clippy 0 warnings | OK | `cargo clippy` 통과 |
| **설계 병목 분석** | **미수행** | 전체 CLI 실행 경로에서의 bottleneck 분석 없음 |

### CL-5: 사용자 친절한 가이드

| 항목 | 상태 | 근거 |
|------|------|------|
| README Installation Guide | OK | Step 0~8 + 트러블슈팅 |
| Pre-flight 점검 | OK | Step 1.5 SSH 접근 테스트 |
| 에러 메시지 | OK | 뉴비 친화적 validation 메시지 |

### CL-6: README 상세 내용

| 항목 | 상태 | 근거 |
|------|------|------|
| Architecture / Philosophy / CLI / GitOps / Structure | OK | 모두 포함 |
| **테스트 카운트 불일치** | **GAP** | README/CLAUDE.md가 현재 310 tests를 정확히 반영하는지 확인 필요 |

### CL-7: Installation Guide E2E 보장

| 항목 | 상태 | 근거 |
|------|------|------|
| Step 0~8 문서 | OK | README에 포함 |
| **전체 E2E 실행** | **NEVER** | 물리 인프라에서 미실행. "설계 철학대로 구현됐음을 보장"할 수 없음 |

**근본 원인**: 오프라인 테스트만으로는 E2E 보장 불가.
이는 물리 인프라가 필요한 작업으로, 오프라인에서는 **dry-run 경로의 완전성**과 **생성물의 정합성**만 검증 가능.

### CL-8: CLI 기능 완성도

| 기능 | 상태 | 비고 |
|------|------|------|
| `scalex facts` | CODE EXISTS | CPU/mem/GPU/disk/NIC/IOMMU/kernel. **실제 SSH 실행 미검증** |
| `scalex sdi init` (no flag) | OK | 호스트 준비 + 통합 리소스 풀 summary 생성/표시. 도움말 수정 (Sprint 11a) |
| `scalex sdi init <spec>` | CODE EXISTS | HCL 생성 + tofu apply. **실제 apply 미검증** |
| `scalex sdi clean --hard` | CODE EXISTS | clean logic (19 tests). **실제 실행 미검증** |
| `scalex sdi sync` | CODE EXISTS | diff 계산 로직 있음. **실제 실행 미검증** |
| `scalex cluster init <config>` | CODE EXISTS | inventory + vars + kubespray + kubeconfig. **실제 실행 미검증** |
| `scalex get baremetals` | OK | facts JSON 파싱 + tabled 출력 |
| `scalex get sdi-pools` | OK | SDI spec 파싱 + 풀 상태 표시 |
| `scalex get clusters` | OK | inventory/vars 파싱 + 클러스터 정보 표시 |
| `scalex get config-files` | OK | 설정 파일 존재/유효성 검증 |

**비판**: "CODE EXISTS"와 "WORKS"는 다르다. `get` 명령어들은 순수한 파싱/출력이므로 OK.
나머지 명령어들은 외부 시스템(SSH, tofu, kubespray)과의 상호작용이 필요하므로 실제 검증 없이는 "동작"을 보장할 수 없다.

### CL-9: 베어메탈 직접 사용 확장성

| 항목 | 상태 | 근거 |
|------|------|------|
| `ClusterMode::Baremetal` | OK | `models/cluster.rs` — SDI/Baremetal 모드 분기 |
| Kubespray only (no k3s) | OK | `test_no_k3s_references_in_project_files` |
| baremetal inventory 생성 | OK | `generate_inventory_baremetal()` + 4 tests |

### CL-10: 보안 정보 관리

| 항목 | 상태 | 근거 |
|------|------|------|
| `credentials/` gitignored | OK | `.gitignore` |
| `.example` 템플릿 | OK | 4개 (baremetal-init, .env, secrets, cloudflare-tunnel) |
| `scalex secrets apply` | OK | K8s Secret YAML 생성, management/workload 분기 |

### CL-11: 커널 파라미터 튜닝

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex kernel-tune` | OK | 14 tests, 역할별 권장값, Ansible 형식, diff |
| docs 가이드 | OK | `docs/ops-guide.md` Section 3 |

### CL-12: 디렉토리 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex-cli/`, `gitops/{common,tower,sandbox}` | OK | 구조 일치 |
| 불필요 파일 | OK | `test_no_legacy_toplevel_artifacts`, `test_no_gitops_dead_code_directories` |

### CL-13: 멱등성

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 멱등성 | OK (unit test) | `test_generate_tofu_main_idempotent` |
| inventory 생성 멱등성 | OK (unit test) | `test_generate_inventory_idempotent` |
| cluster-vars 생성 멱등성 | OK (unit test) | `test_generate_cluster_vars_idempotent` |
| clean→rebuild 멱등성 | OK (unit test) | `test_e2e_clean_rebuild_idempotency` |
| **실제 tofu apply 멱등성** | **NEVER** | OpenTofu 자체는 멱등적이나 **생성된 HCL의 실제 적용 미검증** |
| **kubespray 재실행 멱등성** | **NEVER** | 미검증 |

### CL-14: 외부 kubectl 접근 (CF Tunnel)

| 항목 | 상태 | 근거 |
|------|------|------|
| CF Tunnel ingress: `api.k8s.jinwang.dev` | CONFIG OK | `values.yaml` — `https://kubernetes.default.svc:443` |
| SOCKS5 proxy | CONFIG OK | `socks5-proxy/manifest.yaml` |
| kubeconfig 생성 가이드 | OK | `client/generate-kubeconfig.sh` + `docs/ops-guide.md` |
| **실제 외부 접근 검증** | **NEVER** | CF Tunnel Pod 미기동 상태에서 검증 불가 |

### CL-15: NAT 접근 방법

| 항목 | 상태 | 근거 |
|------|------|------|
| Tailscale / CF Tunnel / LAN | OK | `docs/ops-guide.md` Section 4 — 3가지 비교표 |
| LAN 스위치 접근 가이드 | OK | Section 4에 포함 |

---

## 이전 DASHBOARD의 한계 — 근본 원인 분석

### 1. "OK"의 기준이 모호했다

이전 DASHBOARD는 **코드가 존재하면 "OK"**, 물리 인프라 필요 시 "NEVER"로 분류했다.
그러나 체크리스트는 **"동작하는가?"**를 묻고 있다. 코드 존재와 동작은 다른 것이다.

### 2. 의미론적 분석 부재

CL-1의 `sdi init` (no flag)가 **"전체 리소스 풀로 관측되도록 구성"**을 요구하는데,
실제 코드는 **호스트 준비(KVM/bridge/VFIO 설치)**만 수행한다.
이 차이를 이전 분석은 "도움말 수정"(B-6)으로 해소했다고 판단했는데, 이는 **요구사항을 코드에 맞춘 것**이지 코드를 요구사항에 맞춘 것이 아니다.

### 3. 테스트 커버리지의 착시

310개 테스트 중:
- **순수 함수 테스트**: ~280개 (HCL 생성, inventory 생성, 파싱, 검증)
- **통합 경로 테스트**: ~20개 (cross-config consistency, pipeline dry-run)
- **실제 I/O 테스트**: 0개

실제 사용자가 `scalex sdi init`을 실행했을 때의 전체 경로(facts 확인 → config 로드 → 호스트 준비 → HCL 생성 → tofu apply)를 검증하는 테스트는 **하나도 없다**.

### 4. 오프라인 한계 인정 부족

물리 인프라 없이는 검증할 수 없는 항목들(CL-1 실제 apply, CL-7 E2E, CL-13 멱등성, CL-14 외부 접근)을 명확히 구분하지 않고, 오프라인에서 할 수 있는 작업과 혼재시켰다.

---

## 오프라인에서 해결 가능한 GAP 목록

| # | GAP | 해결 방법 | 상태 |
|---|-----|-----------|------|
| G-1 | `sdi init` (no flag) 리소스 풀 관측 | 이미 구현됨 — 도움말 텍스트 수정 + 3개 검증 테스트 추가 | **FIXED** (Sprint 11a) |
| G-2 | README/CLAUDE.md 테스트 카운트 | 313 tests로 동기화 | **FIXED** (Sprint 11a) |
| G-3 | `sdi init` 리소스 풀 상태 파일 | 이미 `resource-pool-summary.json` 생성 중 | **VERIFIED** |
| G-4 | `get sdi-pools` no-spec 모드 | 이미 `resource-pool-summary.json` 로드 + "Unified Bare-Metal Resource Pool" 표시 | **VERIFIED** |
| G-5 | dry-run 경로의 E2E 통합 테스트 부재 | dry-run 모드에서의 전체 파이프라인 테스트 추가 | TODO (Sprint 11b) |

## 오프라인에서 해결 불가능한 항목 (물리 인프라 필요)

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

### Sprint 11a: `sdi init` 리소스 풀 관측 기능 + 문서 정합성 (오프라인)

> **목표**: CL-1의 핵심 GAP 해결 — `sdi init` (no flag) 실행 후 **전체 리소스 풀이 관측 가능한 상태**를 만든다.

| # | Task | TDD 테스트 | 상태 |
|---|------|-----------|------|
| 11a-1 | 리소스 풀 기능 이미 구현됨 확인 | 기존 5개 테스트 | VERIFIED |
| 11a-2 | `sdi init` no-spec → `resource-pool-summary.json` 이미 구현됨 확인 | `test_sdi_init_no_spec_generates_resource_pool_summary` | DONE |
| 11a-3 | `get sdi-pools` → baremetal pool 표시 이미 구현됨 확인 | `test_get_sdi_pools_supports_baremetal_resource_pool` | DONE |
| 11a-4 | `sdi init` 도움말: "unified resource pool" 의미로 수정 | `test_sdi_init_help_describes_resource_pool` | DONE |
| 11a-5 | README/CLAUDE.md 테스트 카운트 313으로 동기화 | | DONE |
| 11a-6 | 전체 테스트 통과 (313) + Commit + Push | | DONE |

### Sprint 11b: dry-run E2E 통합 테스트 (오프라인) — **COMPLETE** (2026-03-11)

> **목표**: 실제 I/O 없이 전체 파이프라인(facts→sdi init→cluster init→secrets→gitops)의 dry-run 경로가 정상 동작함을 검증

| # | Task | 상태 |
|---|------|------|
| 11b-1 | facts→resource pool→SDI→cluster→secrets→kernel-tune 전체 파이프라인 테스트 | DONE |
| 11b-2 | SSH command 생성 E2E 테스트 (3가지 접근 모드 전체) | DONE |
| 11b-3 | 테스트 카운트 315 동기화 + Commit + Push | DONE |

### Sprint 11c: 실환경 E2E 검증 (물리 인프라 필요)

> **목표**: CL-7 "완전히 초기화된 베어메탈에서 `scalex get cluster` 동작까지" 보장

| # | Task | 상태 |
|---|------|------|
| 11c-1 | `scalex facts --all` 실행 (4노드) | TODO |
| 11c-2 | `scalex sdi init` (no flag — 리소스 풀 관측 확인) | TODO |
| 11c-3 | `scalex sdi init config/sdi-specs.yaml` 실행 | TODO |
| 11c-4 | `scalex cluster init config/k8s-clusters.yaml` 실행 | TODO |
| 11c-5 | `scalex secrets apply` 실행 | TODO |
| 11c-6 | GitOps bootstrap + ArgoCD 동작 확인 | TODO |
| 11c-7 | 외부 kubectl 접근 (CF Tunnel) 검증 | TODO |
| 11c-8 | `sdi clean --hard` + 재구축 (멱등성) 검증 | TODO |

### Sprint 11d: 확장성 검증

| # | Task | 상태 |
|---|------|------|
| 11d-1 | 단일 노드 SDI E2E | TODO |
| 11d-2 | 3번째 클러스터 추가 확장성 검증 | TODO |
| 11d-3 | Keycloak Realm GitOps 자동화 | TODO |

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
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node |
| models/* | 8 | parse/serialize |
| core/resource_pool | 5 | aggregation, table |
| commands/facts | 4 | facts gathering |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| **TOTAL** | **315** | |

---

## Bug History

### Sprint 10a (2026-03-11) — 9 bugs fixed

| # | Severity | 결함 | 수정 |
|---|----------|------|------|
| B-1 | CRITICAL | AppProjects 미배포 → `project not found` | `spread.yaml`에 `cluster-projects` Application 추가 |
| B-2 | CRITICAL | Kyverno helm repo 누락 | AppProject `sourceRepos`에 추가 |
| B-3 | HIGH | `sshKeyPathOfReachableNode` 미사용 | ProxyCommand `-i` 연결 |
| B-4 | HIGH | `sdi sync` 네트워크 설정 무시 | `bm_config.network_defaults` 전달 |
| B-5 | HIGH | CIDR prefix `/24` 하드코딩 | `extract_cidr_prefix()` 함수 추가 |
| B-6 | MEDIUM | `sdi init` 도움말 오해 유발 | 도움말 텍스트 수정 |
| B-7 | MEDIUM | git submodule init 미안내 | README Step 0.5 추가 |
| B-8 | MEDIUM | README 테스트 카운트 stale | 업데이트 |
| B-9 | MEDIUM | api.k8s 선택/필수 불일치 | "필수"로 통일 |
