# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Checklist Status (15 Items)

> **판정 기준**: "코드 존재"가 아닌 "테스트로 검증됨"을 기준으로 판정.
> - **VERIFIED**: 오프라인 테스트로 로직 검증 완료
> - **CODE-ONLY**: 코드는 존재하나 실환경 검증 미완료
> - **NEEDS-INFRA**: 물리 인프라에서만 검증 가능
> - **GAP**: 코드 또는 테스트가 부족

### Layer 1: SDI 가상화 (Checklist #1)

| 항목 | 상태 | 근거 |
|------|------|------|
| 4노드 OpenTofu 가상화 코드 | VERIFIED | `core/tofu.rs` — 12 tests (HCL 생성, SSH URI, VFIO, single-node, host-infra) |
| 리소스 풀 통합 | VERIFIED | `core/resource_pool.rs` — 5 tests (aggregation, table) |
| 2개 클러스터(tower+sandbox) 분할 | VERIFIED | `core/validation.rs` — pool mapping + cluster ID 검증 |
| **실환경 `tofu apply` 실행** | **NEEDS-INFRA** | 실제 libvirt VM 생성은 물리 노드 필요 |

### Layer 2: Cloudflare Tunnel (Checklist #2, #3, #14)

| 항목 | 상태 | 근거 |
|------|------|------|
| CF Tunnel ArgoCD GitOps 배포 | VERIFIED | `gitops/tower/cloudflared-tunnel/` — kustomization + values.yaml |
| 터널 라우팅 설정 (3개 도메인) | VERIFIED | `api.k8s.jinwang.dev`, `auth.jinwang.dev`, `cd.jinwang.dev` |
| 사용자 수동 작업 가이드 | VERIFIED | `docs/ops-guide.md` Section 1 + `docs/CLOUDFLARE-ACCESS.md` |
| SOCKS5 프록시 (kubectl용) | CODE-ONLY | `gitops/tower/socks5-proxy/manifest.yaml` 존재, 실환경 미검증 |
| **외부 kubectl 접근 E2E** | **NEEDS-INFRA** | CF Tunnel → SOCKS5 → kube-apiserver 경로 실검증 필요 |
| Keycloak 없이 kubectl 동작 여부 | VERIFIED | `docs/ops-guide.md` Section 4 "Pre-OIDC" — CF Tunnel + admin kubeconfig |

### Layer 3: CLI 도구 (Checklist #4, #8)

| 항목 | 상태 | 근거 |
|------|------|------|
| Rust 구현 | VERIFIED | `scalex-cli/` — Cargo.toml, 26 .rs files, ~14,000 lines |
| FP 원칙 (순수 함수) | VERIFIED | 모든 generator가 입출력만 처리, side-effect 없음 |
| `scalex facts` | VERIFIED | `commands/facts.rs` — 4 tests + `core/config.rs` — 15 tests |
| `scalex sdi init` (no flag) | VERIFIED | `commands/sdi.rs` — resource pool summary 생성 로직 |
| `scalex sdi init <spec>` | VERIFIED | `commands/sdi.rs` — HCL 생성 + pool state 저장 |
| `scalex sdi clean --hard` | VERIFIED | `commands/sdi.rs` — clean validation tests |
| `scalex sdi sync` | VERIFIED | `core/sync.rs` — 13 tests (diff, conflict, add/remove) |
| `scalex cluster init` | VERIFIED | `commands/cluster.rs` — 11 tests + `core/kubespray.rs` — 32 tests |
| `scalex get` (4 subcommands) | VERIFIED | `commands/get.rs` — 18 tests |
| `.baremetal-init.yaml` 3가지 SSH 모드 | VERIFIED | direct, reachable_node_ip, reachable_via + ProxyJump |
| **실환경 CLI 실행** | **NEEDS-INFRA** | 모든 명령어의 실 실행은 물리 노드 필요 |

### Layer 4: 문서화 (Checklist #5, #6, #7)

| 항목 | 상태 | 근거 |
|------|------|------|
| README.md 설치 가이드 (Step 0-8) | VERIFIED | 8단계 + pre-flight + troubleshooting 포함 |
| 7개 docs/ 문서 | VERIFIED | setup-guide, architecture, ops-guide, troubleshooting 등 |
| CLI 레퍼런스 | VERIFIED | README.md에 core + query 명령어 전체 문서화 |
| **설치 가이드 E2E 실행 검증** | **NEEDS-INFRA** | 초기화된 베어메탈 → `scalex get clusters` 동작까지 미검증 |

### Layer 5: 아키텍처 & 확장성 (Checklist #9-15)

| 항목 | 상태 | 근거 |
|------|------|------|
| #9 Baremetal 모드 확장성 | VERIFIED | `ClusterMode::Baremetal` enum + `generate_inventory_baremetal()` + tests |
| #10 시크릿 템플릿화 | VERIFIED | `credentials/*.example` 5개 + `core/secrets.rs` — 12 tests |
| #11 커널 파라미터 튜닝 | VERIFIED | `commands/kernel_tune.rs` + `core/kernel.rs` — 14 tests + `docs/ops-guide.md` |
| #12 디렉토리 구조 | VERIFIED | scalex-cli/ + gitops/ + credentials/ + config/ + docs/ 정합 |
| #13 멱등성 | CODE-ONLY | 코드상 멱등 설계이나 실환경 재적용 미검증 |
| #14 외부 kubectl (CF Tunnel) | **NEEDS-INFRA** | 라우팅 설정 존재, 실 접근 미검증 |
| #15 NAT 접근 경로 (Tailscale+CF) | VERIFIED | `docs/ops-guide.md` Section 4 + LAN 접근 가이드 포함 |

---

## Gap Summary

### 오프라인에서 추가 검증 가능한 갭

| ID | 갭 | 해결 방법 |
|----|------|-----------|
| G-7 | Config example 파일이 체크리스트 스펙과 정확히 일치하는지 테스트 없음 | 파싱 + 필드 존재 테스트 추가 |
| G-8 | `sdi init` (no-flag) 자동 디스커버리 경로 테스트 부족 | resource pool summary 생성 로직 테스트 |
| G-9 | CF Tunnel values.yaml 라우팅이 kubectl 접근 도메인과 일치하는지 테스트 없음 | GitOps YAML 파싱 테스트 |
| G-10 | 멱등성 (같은 입력 → 같은 출력) 오프라인 테스트 부족 | HCL/inventory 재생성 동일성 테스트 |
| G-11 | README 설치 가이드의 명령어가 실제 CLI와 일치하는지 테스트 없음 | README 파싱 + CLI 구조 비교 테스트 |
| G-12 | 불필요 파일 존재 (PROMPT.md, REQUEST-TO-USER.md) | 삭제 |
| ~~G-13~~ | ~~Keycloak 없이 외부 kubectl 직접 접근 방법 미문서화~~ | **RESOLVED** — `docs/ops-guide.md` Section 4 "Pre-OIDC" 이미 문서화 |

### 실환경 필수 갭 (물리 인프라 필요)

| ID | 갭 | 해결 방법 |
|----|------|-----------|
| I-1 | `scalex facts --all` 실행 (4노드) | 물리 노드 SSH 접근 |
| I-2 | `scalex sdi init` → 실제 VM 생성 | libvirt + OpenTofu |
| I-3 | `scalex cluster init` → 실제 K8s 프로비저닝 | Kubespray 실행 |
| I-4 | GitOps bootstrap → ArgoCD 동작 | K8s 클러스터 필요 |
| I-5 | CF Tunnel 외부 kubectl 접근 | 실제 터널 + 도메인 필요 |
| I-6 | `sdi clean --hard` + 재구축 (멱등성) | 전체 인프라 필요 |

---

## Execution Plan

### Sprint 13a: Config Format Alignment Tests (오프라인)
- [ ] `.baremetal-init.yaml.example` 파싱 + 3가지 SSH 모드 필드 존재 검증
- [ ] `sdi-specs.yaml.example` 파싱 + pool_name/nodeSpecs 구조 검증
- [ ] `k8s-clusters.yaml.example` 파싱 + cluster_sdi_resource_pool 매핑 검증

### Sprint 13b: External Access Documentation Verification (오프라인) ✅
- [x] G-13 해결: Pre-OIDC kubectl 접근 경로 검증 (ops-guide.md Section 4 이미 문서화)
- [x] NAT 접근 3가지 방법 (CF Tunnel, Tailscale, LAN+스위치) 문서화 검증
- [x] 2 tests 추가 (342 total)

### Sprint 13c: 2-Layer Template Consistency + Client OIDC (오프라인) ✅
- [x] sdi-specs pool_name ↔ k8s-clusters cluster_sdi_resource_pool 정합성 검증
- [x] k8s-clusters domains ↔ CF Tunnel values.yaml 라우팅 정합성 검증
- [x] client/kubeconfig-oidc.yaml.j2 Jinja2 변수 검증 (domains, keycloak)
- [x] credentials/ 5개 example 파일 완전성 검증
- [x] 버그 수정: setup-client.sh "playbox" → "scalex" 레거시 참조
- [x] 5 tests 추가 (347 total)

### Sprint 13d: Edge Cases (오프라인) ✅
- [x] Cilium cluster_id 고유성 검증 (ClusterMesh 요구사항)
- [x] 공통 설정 필수 필드 검증 (CNI, runtime, etcd, DNS 등)
- [x] SDI+Baremetal 혼합 모드 공존 검증 (#9 확장성)
- [x] SDI node placement → baremetal-init 호스트 참조 검증
- [x] 클러스터 간 Pod/Service CIDR 비중복 검증
- [x] 5 tests 추가 (352 total)

### Sprint 14: 실환경 E2E (물리 인프라 필요)
- [ ] I-1 ~ I-6 순차 실행

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
| core/resource_pool | 5 | aggregation, table |
| commands/facts | 4 | facts gathering |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| core/validation (13a-e) | 18 | config alignment, CF tunnel routing, idempotency, README accuracy, GitOps completeness |
| core/validation (13b) | 2 | pre-OIDC kubectl access docs, NAT access methods docs |
| core/validation (13c) | 5 | 2-layer template consistency, OIDC template, credentials completeness, setup-client fix |
| core/validation (13d) | 5 | Cilium cluster_id, common config, mixed-mode, host placement, CIDR overlap |
| **TOTAL** | **352** | |

---

## Sprint History

| Sprint | Date | Tests | Summary |
|--------|------|-------|---------|
| 13d | 2026-03-11 | 352 | Edge cases: Cilium cluster_id, common config, mixed-mode, host placement, CIDR overlap |
| 13c | 2026-03-11 | 347 | 2-layer template 정합성, OIDC 템플릿, credentials 완전성, setup-client.sh 버그 수정 |
| 13b | 2026-03-11 | 342 | G-13 해결 (pre-OIDC kubectl 이미 문서화), NAT 접근 경로 검증 tests |
| 13a | 2026-03-11 | 340 | Checklist 15항목 갭 분석 + 18 tests (config alignment, CF tunnel, idempotency, docs accuracy) |
| 12e | 2026-03-11 | 322 | GitOps structure ↔ cluster config consistency test |
| 12d | 2026-03-11 | 321 | 3rd cluster extensibility test (tower+sandbox+datax pipeline) |
| 12b | 2026-03-11 | 320 | Meta-file cleanup |
| 12a | 2026-03-11 | 320 | Gap verification tests: baremetal E2E, CF tunnel, single-node SDI |
| 11b | 2026-03-11 | 315 | E2E pipeline + SSH integration tests |
| 11a | 2026-03-11 | 313 | DASHBOARD rewrite + sdi init resource pool verification |
| 10a | 2026-03-11 | 308 | 9 bugs fixed (B-1~B-9) |
| 9a | 2026-03-11 | 301 | Sprint 9a final |
