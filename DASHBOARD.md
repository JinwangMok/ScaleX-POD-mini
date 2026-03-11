# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석

### 근본 문제: "CODE-EXISTS" ≠ "동작한다"

이전 DASHBOARD는 15개 체크리스트 항목 대부분을 `✅ VERIFIED`로 표기했으나, **검증 기준이 순수 함수 단위 테스트 통과에 한정**되어 있다.

**비판 1: 테스트의 범위가 순수 함수에만 한정됨**
- 445개 테스트 전부가 `#[cfg(test)]` 인라인 단위 테스트
- HCL 문자열 생성, YAML 파싱, 테이블 포맷팅 등 **데이터 변환 로직만 검증**
- `run_init()`, `run_clean()`, `run_sync()` 등 **실행 경로(I/O)는 0% 테스트 커버리지**
- SSH 연결, `tofu apply`, Kubespray 실행, `kubectl` 접근 등 핵심 작업이 미검증

**비판 2: Checklist 항목별 검증 깊이가 불균등**
- #4(Rust CLI), #10(시크릿), #12(디렉토리)는 코드 존재만으로 ✅ 부여 가능
- #1(SDI 가상화), #7(E2E Installation), #13(멱등성)은 **실환경 없이 검증 불가능**
- #14(외부 kubectl)는 **구조적으로 Keycloak 없이 CF Tunnel 경유 불가능**이라는 핵심 제약을 발견했으나, 해결책 구현은 없음

**비판 3: "코드 수준 검증"이라는 면책 조항**
- 실환경 없이도 개선 가능한 로직 갭들이 존재:
  - `sdi init` no-flag 경로의 `build_host_infra_inputs()` 엣지 케이스 미테스트
  - `resolve_network_config()` 우선순위 로직 미테스트
  - `check_gpu_passthrough_needed()` spec 파싱 연동 미테스트
  - `run_sync()`의 `--force` 플래그 분기 로직 미테스트
  - `cluster init`의 gitops update + kubeconfig SCP 인수 구성 미테스트

**비판 4: Sprint 기록의 단절**
- Sprint 25 이후 Sprint 29-30이 있으나 DASHBOARD에 Sprint 30 기록 누락
- 테스트 카운트가 431로 표기되어 있으나 실제는 445개 (Sprint 30에서 14개 추가됨)

---

## 현재 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ VERIFIED | 테스트로 검증 완료 (순수 함수 테스트 통과) |
| ✅ STRUCTURE-OK | 코드/파일 구조가 올바름 (구조적 검증) |
| 🔶 CODE-EXISTS | 코드는 존재하지만 실행 경로 미검증 |
| 🔶 PARTIAL | 일부만 구현/검증 |
| ❌ NOT-DONE | 구현 또는 검증되지 않음 |
| ⬜ NEEDS-INFRA | 물리 인프라에서만 검증 가능 |
| ⬜ NEEDS-USER | 사용자 수동 작업 필요 |

---

## Checklist 상세 검증 (15개 항목)

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

**상태: ✅ VERIFIED (순수 함수) / 🔶 CODE-EXISTS (실행) / ⬜ NEEDS-INFRA (E2E)**

| 구성 요소 | 상태 | 근거 |
|-----------|------|------|
| `sdi-specs.yaml` 파싱/검증 | ✅ | 모델 파싱, pool 검증, IP 중복, 리소스 0 체크 테스트 |
| HCL 생성 (`generate_tofu_main`) | ✅ | provider, VM 정의, ssh_user, VFIO, single-node 테스트 |
| Host-infra HCL 생성 (`generate_tofu_host_infra`) | ✅ | 단일/다중 노드, 멱등성 테스트 |
| 리소스 풀 요약 생성 | ✅ | 집계, 테이블 포맷, disk_gb 테스트 |
| KVM/bridge/VFIO 설치 스크립트 | ✅ | 순수 함수 스크립트 생성 테스트 |
| SDI host 참조 검증 | ✅ | unknown host 탐지 + sdi init 파이프라인 연결 |
| `sdi init` no-flag: host-infra 경로 | ✅ | `build_host_infra_inputs`, network config resolution 테스트 |
| `sdi init <spec>`: VM pool 경로 | ✅ | HCL + state + spec cache 생성 테스트 |
| `tofu init/apply` 실제 실행 | 🔶 | 코드 존재, 실행 미검증 |
| VM이 실제로 생성/접근 가능한지 | ⬜ | 물리 인프라 필요 |

**아키텍처 정합성**: 4개 노드 → resource_pool(통합 뷰) → sdi_pools(tower + sandbox) → 2개 클러스터 구조 ✅

---

### #2. CF Tunnel GitOps 배포

**상태: ✅ VERIFIED**

- `gitops/tower/cloudflared-tunnel/kustomization.yaml` — Helm chart (community-charts, v2.1.2) ✅
- `gitops/tower/cloudflared-tunnel/values.yaml` — tunnel name `playbox-admin-static`, ingress 3개 ✅
- `gitops/generators/tower/tower-generator.yaml` — sync wave 3에 cloudflared-tunnel 포함 ✅
- ArgoCD가 spread.yaml 적용 시 CF Tunnel을 자동 배포하는 GitOps 구조 ✅

---

### #3. CF Tunnel 완성도 + 사용자 수동 작업

**상태: 🔶 PARTIAL**

**자동 처리**: Helm chart 배포 (ArgoCD), K8s Secret 생성 (`scalex secrets apply`), Ingress 규칙 3개

**사용자 필수 작업**:
1. ⬜ Cloudflare Dashboard에서 tunnel `playbox-admin-static` 생성
2. ⬜ Credentials JSON 다운로드 → `credentials/cloudflare-tunnel.json` 저장
3. ⬜ Public Hostname 3개: `cd.jinwang.dev`, `auth.jinwang.dev`, `api.k8s.jinwang.dev`
4. ⬜ DNS CNAME 자동 생성 확인

**문서화**: `docs/ops-guide.md`에 상세 가이드 존재 ✅

---

### #4. CLI Rust 구현 + FP 원칙

**상태: ✅ VERIFIED**

| 항목 | 상태 |
|------|------|
| Rust 구현 | ✅ `scalex-cli/` — 27개 .rs 파일, Cargo 프로젝트 |
| clap derive CLI | ✅ 8개 서브커맨드 |
| 순수 함수 분리 | ✅ `generate_*` (I/O 없음) / `run_*` (I/O) 분리 |
| thiserror 에러 | ✅ `ScalexError` enum |
| 445 tests, 0 clippy warnings | ✅ |
| cargo fmt | ✅ |
| FP 원칙 (Pure/No Side Effect/Immutability) | ✅ 모든 생성 함수가 순수 |

---

### #5. 사용자 친절한 가이드

**상태: ✅ VERIFIED**

- README.md: Installation Guide (Step 0~8), Quick Reference, CLI Reference, Troubleshooting
- docs/ops-guide.md: CF Tunnel + Keycloak 설정 가이드
- docs/SETUP-GUIDE.md: 상세 프로비저닝 워크스루
- docs/TROUBLESHOOTING.md: 문제별 원인/해결 테이블
- CLI: `--help`, `--dry-run` 모든 커맨드 지원

---

### #6. README.md 상세 내용

**상태: ✅ VERIFIED**

포함 섹션: Architecture Overview, Design Philosophy (7개 원칙), Installation Guide (Step 0~8), Quick Reference, CLI Reference, GitOps Pattern (sync waves, app 추가), Project Structure, Testing, Documentation 링크

---

### #7. Installation Guide → 초기화된 베어메탈에서 `scalex get clusters`까지

**상태: 🔶 PARTIAL**

- README Step 0~8: 논리적으로 완전한 흐름 제공 ✅
- Pre-flight SSH 테스트 가이드 (Step 1.5) ✅
- 실패 시 대응 가이드 + Troubleshooting 테이블 ✅
- **그러나**: 실제 초기화된 베어메탈에서 끝까지 실행한 적 없음 ❌
- Step 4→5→7 경로가 실환경에서 작동하는지 미검증 ⬜

---

### #8. CLI 기능 전체

| 명령어 | 코드 | 순수 함수 테스트 | 실행 로직 | 실환경 |
|--------|:----:|:---------------:|:---------:|:------:|
| `scalex facts --all/--host` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex sdi init` (no flag) | ✅ | ✅ | 🔶 | ⬜ |
| `scalex sdi init <spec>` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex sdi clean --hard --yes-i-really-want-to` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex sdi sync [--force]` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex cluster init <config>` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex get baremetals` | ✅ | ✅ | N/A | ⬜ |
| `scalex get sdi-pools` | ✅ | ✅ | N/A | ⬜ |
| `scalex get clusters` | ✅ | ✅ | N/A | ⬜ |
| `scalex get config-files` | ✅ | ✅ | N/A | ⬜ |
| `scalex secrets apply` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex bootstrap` | ✅ | ✅ | 🔶 | ⬜ |
| `scalex status` | ✅ | ✅ | N/A | ⬜ |
| `scalex kernel-tune` | ✅ | ✅ | N/A | ⬜ |

**`.baremetal-init.yaml` 포맷**:
- 3가지 SSH 접근 방식: direct, external IP (Tailscale), ProxyJump ✅
- 2가지 인증: password, key ✅
- `.env` 변수 참조 방식 ✅
- `networkDefaults` 섹션 (SDI host-infra용) ✅

---

### #9. Baremetal 확장성 (SDI 없이)

**상태: ✅ VERIFIED (코드 수준)**

- `k8s-clusters.yaml`: `cluster_mode: "baremetal"` 옵션 ✅
- `generate_inventory_baremetal()` 구현 + 테스트 ✅
- SDI/baremetal 혼합 모드 공존 테스트 ✅
- k3s 참조 없음 (프로덕션 수준 Kubespray만 사용) ✅

---

### #10. 시크릿 템플릿화

**상태: ✅ VERIFIED**

- `credentials/*.example` 4개 파일 존재 ✅
- `credentials/` 디렉토리 `.gitignore` 포함 ✅
- `scalex secrets apply`로 K8s Secret 자동 생성 ✅
- management/workload 클러스터별 시크릿 생성 테스트 (12 tests) ✅

---

### #11. 커널 파라미터 튜닝

**상태: ✅ VERIFIED (코드 수준)**

- `scalex kernel-tune` 커맨드 ✅
- 역할별 파라미터 추천 (base, control-plane, worker, management) ✅
- diff 기능 + Ansible task YAML 생성 + sysctl.conf 생성 ✅
- 14개 테스트 ✅
- `docs/ops-guide.md` 커널 튜닝 섹션 ✅

---

### #12. 디렉토리 구조

**상태: ✅ STRUCTURE-OK**

```
scalex-cli/           ✅ Rust CLI (445 tests)
gitops/               ✅ ArgoCD multi-cluster
  bootstrap/          ✅ spread.yaml
  generators/         ✅ tower/ + sandbox/
  projects/           ✅ tower-project + sandbox-project
  common/             ✅ cilium-resources, cert-manager, kyverno, kyverno-policies
  tower/              ✅ argocd, cert-issuers, cilium, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy
  sandbox/            ✅ cilium, cluster-config, local-path-provisioner, rbac, test-resources
credentials/          ✅ .example 템플릿 (실제 파일 gitignored)
config/               ✅ sdi-specs, k8s-clusters 예제
docs/                 ✅ 7개 문서
ansible/              ✅ node preparation
kubespray/            ✅ submodule v2.30.0
client/               ✅ OIDC kubeconfig
tests/                ✅ run-tests.sh
```

---

### #13. 멱등성

**상태: ✅ VERIFIED (순수 함수) / ⬜ NEEDS-INFRA (실행)**

순수 함수 멱등성 테스트:
- `test_checklist_tofu_hcl_generation_idempotent` ✅
- `test_checklist_kubespray_inventory_idempotent` ✅
- `test_checklist_cluster_vars_idempotent` ✅
- `test_e2e_clean_rebuild_idempotency` ✅
- `test_generate_tofu_host_infra_idempotent` ✅

실행 수준 멱등성 (sdi init → clean → sdi init 사이클): ⬜ 미검증

---

### #14. 외부 kubectl 접근

**상태: 🔶 PARTIAL — 구조적 제약 존재**

| 접근 방법 | 상태 | 조건 |
|-----------|------|------|
| **LAN 직접** | ⬜ NEEDS-INFRA | 동일 네트워크 + kubeconfig |
| **Tailscale** | ⬜ NEEDS-INFRA | Tailscale 설치, kubeconfig server=Tailscale IP |
| **CF Tunnel + OIDC** | ❌ | Keycloak Realm/Client 설정 필수 (미완료) |
| **CF Tunnel + client cert** | ❌ 구조적 불가 | CF가 TLS 종단 → client cert 전달 안됨 |
| **SOCKS5 Proxy** | 🔶 | manifest 존재, 미검증 |

**핵심**: Keycloak 없이는 CF Tunnel 경유 외부 kubectl **불가능**. Pre-OIDC 상태에서 외부 접근은 **Tailscale만 가능**.

**문서화**: `docs/ops-guide.md` + README External Access 섹션 ✅

---

### #15. NAT 접근 경로

**상태: ✅ VERIFIED (문서 수준)**

- 클러스터는 NAT 내부 ✅
- 외부 접근: Tailscale + CF Tunnel ✅
- LAN 내부: 스위치 접근 가이드 문서화 ✅
- README External Access 테이블 + 다이어그램 ✅

---

## 근본 원인 분석

| 구분 | 원인 | 영향 |
|------|------|------|
| **테스트 한계** | 459개 전부 순수 함수 → 실행 경로 0% | 모든 `run_*` 함수 |
| **코드 내 미테스트 로직** | Sprint 31에서 주요 엣지 케이스 보강 완료. 잔여 갭은 I/O 경로만 | #1, #8 |
| **CF Tunnel + kubectl 제약** | TLS 종단 → client cert 불가, OIDC만 가능 | #14 |
| **Keycloak 미설정** | 사용자 수동 작업 필요하나 자동화 부재 | #3, #14 |
| **실환경 미검증** | 물리 인프라 없이 개발 | #1, #7, #13 |

---

## 완료된 Sprint 기록

### Sprint 18: 코드 수준 갭 해소 ✅ (388 → 396 tests)
- [x] CLI 실행 경로 pipeline ordering 테스트
- [x] CF Tunnel 인증 경로 문서 검증 테스트
- [x] `.baremetal-init.yaml` 3가지 접근 방식 + 2가지 인증 방식 테스트

### Sprint 19: 구조 검증 + 확장성 테스트 ✅ (396 → 408 tests)
- [x] SOCKS5 proxy manifest 구조 검증
- [x] GitOps 디렉토리 구조 검증
- [x] 2-Layer 템플릿 관리 검증
- [x] 단일 노드 SDI 풀 + 단일 클러스터 K8s config 검증

### Sprint 21: 엣지 케이스 검증 강화 ✅ (408 → 414 tests)
- [x] 중복 클러스터 이름 검증 함수 + cluster init 파이프라인 연결
- [x] SDI 호스트 참조 검증 함수 + sdi init 파이프라인 연결
- [x] README External Access 섹션 확장

### Sprint 25: CLI 일관성 + 문서 정합성 ✅ (414 → 417 tests)
- [x] `sdi clean`에 `--config`/`--env-file` 플래그 추가 (init/sync과 통일)
- [x] `build_pool_state` 순수 함수 — mixed host assignment 테스트

### Sprint 29: 네트워크 검증 + Sync 안전성 ✅ (417 → 431 tests)
- [x] `validate_cluster_network_overlap()` — Pod/Service CIDR + DNS domain 중복 검증
- [x] `ConflictSeverity` enum (Critical/High/Medium) + 관리 클러스터 보호

### Sprint 30: SDI sync 안전성 + CIDR 검증 ✅ (431 → 445 tests)
- [x] `sdi sync --force` 플래그 추가
- [x] SDI CP validation, CIDR overlap detection 강화
- [x] Removal safety 로직

---

## 실행 계획

### Sprint 31: 미테스트 순수 함수 로직 보강 ✅ DONE (445 → 459 tests)

#### 31a — `sdi init` no-flag 경로 + build_pool_state 엣지 케이스 (+9 tests)
- [x] `build_host_infra_inputs()` — empty, 4-node example, different admin users
- [x] `build_pool_state()` — multi-pool (tower+sandbox), unassigned host fallback, empty pools
- [x] `extract_cidr_prefix()` — boundary values (/0, /32, /8, invalid)
- [x] no-flag pipeline — host-infra HCL 생성 end-to-end
- [x] no-flag fallback network defaults coherence

#### 31b-c — clean/sync 기존 커버리지 확인
- [x] validate_clean_args: 4 tests (all combos) — 이미 충분
- [x] plan_clean_operations: 8 tests (comprehensive) — 이미 충분
- [x] compute_sync_diff: 10+ tests — 이미 충분
- [x] detect_vm_conflicts + severity: 13+ tests — 이미 충분

#### 31d — cluster init 엣지 케이스 (+5 tests)
- [x] `find_control_plane_ip()` — SDI pool without CP role, baremetal without CP, SDI without spec
- [x] `clusters_needing_gitops_update()` — 3-cluster scenario
- [x] `build_kubeconfig_scp_args()` — default root user

#### 31e — DASHBOARD/README/CLAUDE.md 테스트 카운트 업데이트 → 커밋/푸시
- [x] DASHBOARD.md 테스트 카운트 445 → 459 업데이트
- [x] README.md 테스트 카운트 업데이트
- [x] CLAUDE.md 테스트 카운트 업데이트

### Sprint 32: 순수 함수 리팩토링 + 테스트 보강 ✅ DONE (459 → 482 tests)

#### 32a — `check_gpu_passthrough_needed` 순수 함수 분리 (+7 tests)
- [x] `spec_needs_gpu_passthrough()` 순수 함수 추출 (I/O 분리)
- [x] `_host_name` → `host_name` 네이밍 수정
- [x] explicit host match, no match, placement fallback, no devices
- [x] empty pools, empty placement + no host, multi-pool mixed GPU

#### 32b — facts 모듈 테스트 강화 (+10 tests, 4→14)
- [x] parse error paths: missing start/end markers
- [x] empty sections — 빈 노드 파싱 무장애 확인
- [x] `parse_section` edge cases: missing markers, start > end
- [x] NIC speed formatting boundaries (10G/1000M/100M/unknown/25G)
- [x] GPU vendor detection (AMD, unknown)
- [x] IOMMU group multi-device 파싱
- [x] kernel params 두 포맷 (" = " vs "=")
- [x] disk size bytes→GB 변환

#### 32c — secrets 모듈 엣지 케이스 (+6 tests, 13→19)
- [x] unknown/empty/case-sensitive cluster role → empty
- [x] empty data vec in K8sSecretSpec
- [x] JSON value in secret (Cloudflare credentials)
- [x] extra unknown YAML fields → forward compatibility
- [x] management + cloudflare = 3 secrets
- [x] missing required field → parse error

### Sprint 33: 실환경 E2E 준비 (⬜ 인프라 필요)

#### 33a — SDI 가상화 E2E
- [ ] `scalex facts --all` → 4노드 SSH 접근 + JSON 수집
- [ ] `scalex sdi init config/sdi-specs.yaml` → VM 5개 생성
- [ ] `scalex get sdi-pools` → 2개 풀 확인
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` → 완전 초기화
- [ ] 재실행 (멱등성 검증)

#### 33b — Kubespray + ArgoCD E2E
- [ ] `scalex cluster init` → tower + sandbox 클러스터 생성
- [ ] `kubectl get nodes` (tower/sandbox) → 정상 응답
- [ ] `scalex secrets apply` → K8s secrets 생성
- [ ] `scalex bootstrap` → ArgoCD 설치 + spread.yaml 적용
- [ ] `kubectl -n argocd get applications` → 모든 앱 Synced/Healthy

#### 33c — 외부 접근 E2E
- [ ] Tailscale IP로 tower kubectl 접근
- [ ] Keycloak Realm/Client 설정 (사용자 수동)
- [ ] CF Tunnel 경유 OIDC kubectl 접근
- [ ] `https://cd.jinwang.dev` ArgoCD UI 접근

### Sprint 34: 확장성 검증 (⬜ 인프라 필요)

- [ ] 단일 노드 모드: 모든 VM을 1개 호스트에 배치
- [ ] 3rd 클러스터 추가: sdi-specs + k8s-clusters + gitops/generators 확장
- [ ] baremetal 모드 E2E: SDI 없이 직접 클러스터링

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
|  → secrets apply → bootstrap             |
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

### External Access Paths

```
[CF Tunnel + OIDC] (Keycloak 설정 완료 후에만 kubectl 가능)
  kubectl (OIDC token) → CF Edge → cloudflared → kube-apiserver
  ⚠ client certificate auth는 CF Tunnel 통과 불가 (TLS 종단)
  ⚠ Keycloak 미설정 시 외부 kubectl 불가

[Tailscale] (cert + token — Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) → Tailscale IP → kube-apiserver
  SSH → Tailscale IP → bastion → 내부 노드

[SOCKS5 Proxy] (LAN/Tailscale 내부에서의 편의 접근)
  kubectl --proxy-url socks5://tower-ip:1080 → kube-apiserver

[LAN] (모든 인증 방식)
  kubectl → LAN IP → kube-apiserver
  SSH → LAN IP → 노드
```

---

## Test Summary

| Module | Tests | 주요 커버리지 |
|--------|:-----:|----------|
| core/validation | 95+ | pool mapping, cluster IDs/names, CIDR overlap, DNS, bootstrap, E2E pipeline |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh |
| core/kubespray | 32+ | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node |
| commands/sdi | 31 | network resolve, host infra, pool state, clean validation, CIDR, GPU passthrough |
| commands/status | 21 | platform status reporting |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14+ | ArgoCD helm, cluster add, kubectl apply, pipeline |
| core/sync | 13 | sync diff, VM conflict, severity, add+remove |
| core/secrets | 18 | K8s secret generation, edge cases, forward compat |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node, host-infra |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table, disk_gb |
| core/ssh | 5 | SSH command building, ProxyJump key |
| commands/facts | 14 | facts gathering, parse edge cases, NIC/GPU/IOMMU/disk |
| **TOTAL** | **459** | **순수 함수 테스트만 — 실행 경로(I/O) 미포함** |
