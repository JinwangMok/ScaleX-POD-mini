# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status (Sprint 10a in progress)

- **Tests**: 301 pass / clippy 0 warnings / fmt clean
- **Code**: ~13,500 lines Rust, 27 source files
- **GitOps**: 41 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## Checklist Gap Analysis (Sprint 10a — 심층 재분석)

> 이전 DASHBOARD(Sprint 9a)는 "OK" 표기에도 불구하고 **실제 코드 버그 7건, 문서 갭 5건**을 놓쳤다.
> Sprint 10a는 코드를 라인 단위로 재검증하여 발견한 실질적 결함을 수정한다.

### CL-1: 4개 노드 OpenTofu 가상화 + 리소스 풀 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 (multi-host libvirt) | OK | `core/tofu.rs` — `generate_tofu_main()` |
| 4개 호스트 provider | OK | `generate_provider_block()` per unique host |
| VM 리소스 생성 | OK | `generate_vm_resource()` — disk, cloudinit, domain |
| 단일 노드 환경 | OK | `test_single_node_sdi_all_pools_on_one_host` |
| `sdi init` (no spec) 설명 오류 | **BUG** | 도움말: "virtualize bare-metal" → 실제: host prepare만 수행. 리소스 풀 가상화 아님 |
| 실제 `tofu apply` 실행 | NEVER | 물리 인프라에서 미실행 |

### CL-2: Cloudflare tunnel — ArgoCD GitOps 방식

| 항목 | 상태 | 근거 |
|------|------|------|
| GitOps YAML 존재 | OK | `gitops/tower/cloudflared-tunnel/` |
| Helm chart 방식 | OK | `values.yaml` — tunnelConfig + ingress |
| ApplicationSet 등록 | OK | `tower-generator.yaml` — syncWave: "3" |

### CL-3: Cloudflare tunnel 완료 여부

| 항목 | 상태 | 근거 |
|------|------|------|
| 사용자 수동 작업 문서화 | OK | `docs/ops-guide.md` Section 1 — 6단계 가이드 |
| credentials 템플릿 | OK | `credentials/cloudflare-tunnel.json.example` |
| `scalex secrets apply` 자동화 | OK | tunnel-credentials Secret 생성 |
| **실제 동작 검증** | **NEVER** | cloudflared Pod 기동 여부 미확인 |

### CL-4: CLI — Rust 구현 + FP 스타일

| 항목 | 상태 | 근거 |
|------|------|------|
| Rust 구현 | OK | `scalex-cli/` — 27 source files |
| Pure functions | OK | generators/validators는 I/O 분리 |
| Clippy 0 warnings | OK | `cargo clippy` 통과 |
| **SSH ProxyJump key 미연결** | **BUG** | `ssh.rs:45-63` — `sshKeyPathOfReachableNode` 파싱만 되고 실제 `-i` 옵션에 사용 안됨 |
| **CIDR prefix 하드코딩** | **BUG** | `sdi.rs:198,603` — `/24` 하드코딩, `networkDefaults.managementCidr`에서 추출해야 함 |
| **sync 경로 네트워크 무시** | **BUG** | `sdi.rs:593-598` — `resolve_network_config(None, None)` → bm_config.network_defaults 무시 |

### CL-5: 사용자 친절한 가이드

| 항목 | 상태 | 근거 |
|------|------|------|
| README Installation Guide | OK | Step 0~8 + 트러블슈팅 |
| Pre-flight 점검 | OK | Step 1.5 SSH 접근 테스트 |
| 에러 메시지 | OK | 뉴비 친화적 validation 메시지 |
| **git submodule init 누락** | **DOC-GAP** | Kubespray submodule init 단계 미안내 → `cluster init` 실패 유발 |

### CL-6: README 상세 내용

| 항목 | 상태 | 근거 |
|------|------|------|
| Architecture / Philosophy / CLI / GitOps / Structure | OK | 모두 포함 |
| **테스트 카운트 하드코딩** | **DOC-GAP** | README "300 tests" → 실제 301개 (stale) |

### CL-7: Installation Guide E2E 보장

| 항목 | 상태 | 근거 |
|------|------|------|
| Step 0~8 문서 | OK | README에 포함 |
| **전체 E2E 실행** | **NEVER** | 물리 인프라에서 미실행 |

### CL-8: CLI 기능 완성도

| 기능 | 상태 | 비고 |
|------|------|------|
| `scalex facts` | OK | CPU/mem/GPU/disk/NIC/IOMMU/kernel |
| `scalex sdi init` (no flag) | **BUG** | 도움말 오해의 소지 (host prepare만 수행) |
| `scalex sdi init <spec>` | OK | HCL 생성 + tofu apply |
| `scalex sdi clean --hard` | OK | clean logic (19 tests) |
| `scalex sdi sync` | **BUG** | 네트워크 config 무시 (hardcoded fallback) |
| `scalex cluster init <config>` | OK | inventory + vars + kubespray + kubeconfig |
| `scalex get baremetals/sdi-pools/clusters/config-files` | OK | 모두 구현 |

### CL-9: 베어메탈 직접 사용 확장성

| 항목 | 상태 | 근거 |
|------|------|------|
| `ClusterMode::Baremetal` | OK | `models/cluster.rs` |
| Kubespray only (no k3s) | OK | 프로덕션 수준 |

### CL-10: 보안 정보 관리

| 항목 | 상태 | 근거 |
|------|------|------|
| `credentials/` gitignored | OK | |
| `.example` 템플릿 4개 | OK | |
| `scalex secrets apply` | OK | |

### CL-11: 커널 파라미터 튜닝

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex kernel-tune` | OK | 14 tests, 역할별 권장값, Ansible 형식, diff |

### CL-12: 디렉토리 구조

| 항목 | 상태 | 비고 |
|------|------|------|
| `scalex-cli/`, `gitops/{common,tower,sandbox}` | OK | 구조 일치 |
| 불필요 파일 | OK | 없음 |

### CL-13: 멱등성

| 항목 | 상태 | 근거 |
|------|------|------|
| `tofu apply` 멱등성 | CLAIMED | OpenTofu 자체 멱등적이나 미검증 |
| `kubespray` 멱등성 | CLAIMED | 미검증 |
| GitOps 멱등성 | OK | ArgoCD self-heal + prune |

### CL-14: 외부 kubectl 접근 (CF Tunnel)

| 항목 | 상태 | 근거 |
|------|------|------|
| CF Tunnel ingress: `api.k8s.jinwang.dev` | OK (config) | `values.yaml` — `https://kubernetes.default.svc:443` |
| SOCKS5 proxy | OK (config) | `socks5-proxy/manifest.yaml` — ClusterIP only |
| **ops-guide: api.k8s 선택/필수 불일치** | **DOC-GAP** | `ops-guide.md:39` 선택 표기 vs ARCHITECTURE.md 필수 표기 |
| **실제 접근 검증** | **NEVER** | |

### CL-15: NAT 접근 방법

| 항목 | 상태 | 근거 |
|------|------|------|
| Tailscale / CF Tunnel / LAN | OK | `docs/ops-guide.md` Section 4 — 3가지 비교표 |

---

## 발견된 버그 및 결함 (Sprint 10a 수정 대상)

### CRITICAL — GitOps 배포 실패

| # | 결함 | 파일 | 영향 |
|---|------|------|------|
| B-1 | **AppProjects 미배포** — `tower-project`, `sandbox-project`가 bootstrap에서 배포되지 않음. 모든 ApplicationSet 생성 Application이 `project not found`로 실패 | `gitops/bootstrap/spread.yaml` | ArgoCD 전체 배포 실패 |
| B-2 | **Kyverno helm repo 누락** — `https://kyverno.github.io/kyverno/` 미등록. Kyverno Application 동기화 실패 | `gitops/projects/{tower,sandbox}-project.yaml` | Kyverno 배포 불가 |

### HIGH — SSH/네트워크 기능 결함

| # | 결함 | 파일:라인 | 영향 |
|---|------|-----------|------|
| B-3 | **`sshKeyPathOfReachableNode` 미사용** — 파싱/env 치환까지 완료되나 `build_ssh_command()` ProxyJump에서 `-i` 옵션 누락 | `ssh.rs:45-63` | Key 기반 ProxyJump SSH 실패 |
| B-4 | **`sdi sync` 네트워크 설정 무시** — `resolve_network_config(None, None)` 호출 → `bm_config.network_defaults` 미전달 | `sdi.rs:593-598` | sync 시 잘못된 네트워크 설정 사용 |
| B-5 | **CIDR prefix `/24` 하드코딩** — `managementCidr`에서 prefix 추출하지 않고 `24` 고정 | `sdi.rs:198,603` | /16, /20 등 비표준 CIDR에서 bridge 설정 오류 |

### MEDIUM — 문서/UX 결함

| # | 결함 | 파일 | 영향 |
|---|------|------|------|
| B-6 | **`sdi init` 도움말 오해** — "virtualize bare-metal" 표기이나 실제는 host prepare만 | `sdi.rs:20` | 사용자 혼란 |
| B-7 | **git submodule init 미안내** — Kubespray submodule 초기화 단계 누락 | `README.md` | 클론 후 cluster init 실패 |
| B-8 | **README 테스트 카운트 오래됨** — "300 tests" → 실제 301 | `README.md` | 정합성 |
| B-9 | **api.k8s.jinwang.dev 선택/필수 불일치** — ops-guide "선택" vs architecture "필수" | `docs/ops-guide.md` | 설정 혼란 |

---

## Root Cause Analysis (Sprint 9a 대비 추가 분석)

### Sprint 9a의 한계

Sprint 9a는 "오프라인 단위 테스트" 갭을 식별하고 14개 테스트를 추가했으나:

1. **코드 리뷰 미수행**: 순수 함수 테스트를 추가했을 뿐, SSH command builder와 GitOps YAML의 **실제 동작 경로**를 추적하지 않음
2. **GitOps YAML 정합성 미검증**: `spread.yaml`이 AppProjects를 배포하지 않는 **구조적 결함**을 놓침
3. **필드 추적 누락**: `sshKeyPathOfReachableNode`가 파싱→저장→끝인 dead code path를 발견 못함
4. **init/sync 대칭성 미검증**: `run_init`은 `network_defaults`를 전달하나 `run_sync`는 누락

### 근본 원인

> "테스트가 많다"와 "올바르게 동작한다"는 다르다.
> 301개 테스트 중 **SSH command building 테스트 2개**, **GitOps 정합성 테스트 0개(AppProject 배포 경로)**.
> 가장 중요한 통합 경로에 대한 테스트가 빈약하다.

---

## Sprint Plan

### Sprint 10a: Critical Bug Fixes + TDD (오프라인 가능) — **현재 진행 중**

| # | Task | 상태 | TDD 검증 |
|---|------|------|----------|
| 10a-1 | **GITOPS FIX**: AppProjects를 `spread.yaml`에 포함 | TODO | RED: AppProject 배포 경로 테스트 → GREEN: spread.yaml 수정 |
| 10a-2 | **GITOPS FIX**: Kyverno helm repo를 AppProjects `sourceRepos`에 추가 | TODO | RED: sourceRepos 검증 테스트 → GREEN: YAML 수정 |
| 10a-3 | **CODE FIX**: `sshKeyPathOfReachableNode`를 ProxyJump `-i`에 연결 | TODO | RED: key-based ProxyJump 테스트 → GREEN: ssh.rs 수정 |
| 10a-4 | **CODE FIX**: `sdi sync` 네트워크 설정에 `bm_config.network_defaults` 전달 | TODO | RED: sync network resolution 테스트 → GREEN: sdi.rs 수정 |
| 10a-5 | **CODE FIX**: CIDR prefix를 `managementCidr`에서 추출 | TODO | RED: /16 CIDR prefix 테스트 → GREEN: parse logic 추가 |
| 10a-6 | **UX FIX**: `sdi init` 도움말 텍스트 수정 | TODO | 도움말 텍스트 테스트 |
| 10a-7 | **DOC FIX**: git submodule init + README 테스트 카운트 + api.k8s 불일치 | TODO | README 검증 테스트 업데이트 |
| 10a-8 | 전체 테스트 통과 확인 + Commit + Push | TODO | `cargo test` + `cargo clippy` |

### Sprint 10b: 실환경 E2E 검증 (물리 인프라 필요)

| # | Task | 상태 |
|---|------|------|
| 10b-1 | `scalex facts --all` 실행 (4노드) | TODO |
| 10b-2 | `scalex sdi init config/sdi-specs.yaml` 실행 | TODO |
| 10b-3 | `scalex cluster init config/k8s-clusters.yaml` 실행 | TODO |
| 10b-4 | `scalex secrets apply` 실행 | TODO |
| 10b-5 | GitOps bootstrap (`kubectl apply -f gitops/bootstrap/spread.yaml`) | TODO |
| 10b-6 | 외부 kubectl 접근 (CF Tunnel) 검증 | TODO |
| 10b-7 | `sdi clean --hard` + 재구축 (멱등성) 검증 | TODO |

### Sprint 10c: 확장성 검증

| # | Task | 상태 |
|---|------|------|
| 10c-1 | 단일 노드 SDI E2E | TODO |
| 10c-2 | 3번째 클러스터 추가 확장성 | TODO |
| 10c-3 | Keycloak Realm GitOps 자동화 | TODO |

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
├── sdi/            (OpenTofu HCL + state)
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
| core/validation | 58 | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generators |
| core/kubespray | 32 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node |
| commands/status | 21 | platform status reporting |
| commands/sdi | 19 | network resolve, host infra, pool state, clean validation |
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
| core/ssh | 2 | SSH command building |
| **TOTAL** | **301** | |
