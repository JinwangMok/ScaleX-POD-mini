# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석

### 근본 문제: "CODE-EXISTS" ≠ "동작한다" ≠ "설계대로 동작한다"

이전 DASHBOARD는 Sprint 32까지 진행하며 482개 테스트를 확보했으나, **3가지 근본적 한계**가 있다:

**비판 1: 테스트 범위가 순수 함수 유닛에 한정**
- 482개 테스트 전부 `#[cfg(test)]` 인라인 단위 테스트
- `generate_*` (순수 함수) 계열만 테스트 — `run_*` (I/O) 계열 0% 커버리지
- SSH, `tofu apply`, `ansible-playbook`, `helm`, `kubectl` 등 실제 실행 경로 미검증
- **순수 함수 간 연동(integration)** 테스트도 부재: 개별 함수는 통과하나 함수 간 데이터 흐름이 올바른지 미검증

**비판 2: "코드 수준 검증"이라는 면책 조항으로 실질적 문제 회피**
- 이전 DASHBOARD는 `⬜ NEEDS-INFRA`로 미검증 항목을 분류하여 책임을 "물리 인프라 부재"로 전가
- 그러나 **물리 인프라 없이도 검증 가능한 갭**이 다수 존재:
  - `sdi init` no-flag → `sdi init <spec>` → `cluster init` 파이프라인의 데이터 연속성
  - config example 파일이 실제 파서를 통과하는지의 round-trip 검증
  - `scalex get config-files` 출력이 모든 필수 파일을 커버하는지
  - GitOps YAML이 ArgoCD 스키마에 적합한지 (dry-run 수준)
  - `sdi sync`의 복잡한 상태 전이 시나리오 (add+remove+conflict 동시)

**비판 3: Checklist 항목별 검증 깊이 불균등**
- #4(Rust CLI), #10(시크릿), #12(디렉토리)는 존재 확인만으로 ✅ 가능
- #1(SDI 가상화), #7(E2E Installation), #13(멱등성)은 **코드 실행 없이 검증 불가**
- #8(CLI 기능 전체)은 가장 복잡하나, 개별 함수 테스트만으로 "지원" 표기

**비판 4: 사용자 Checklist의 "2-Layer 관리 체계" 미구현**
- Checklist 핵심 철학 #6: `sdi-specs.yaml` + `k8s-clusters.yaml` 두 파일로 "가상화 파트"와 "형상관리 파트" 분리
- 현재 상태: 두 파일이 존재하지만, **이 두 파일의 연동이 올바른지 검증하는 테스트가 부재**
- `sdi-specs.yaml`의 `pool_name` ↔ `k8s-clusters.yaml`의 `cluster_sdi_resource_pool` 매핑 검증은 `validation.rs`에 있으나, **example 파일 간 실제 매핑 정합성** 미테스트

**비판 5: Sprint 기록 단절 + 테스트 카운트 불일치**
- DASHBOARD: "TOTAL 459" 표기 → 실제 482 (Sprint 32 반영 누락)
- Sprint 33-34는 "⬜ 인프라 필요"로 기록만 해둔 상태 — 실제 진행 의지 부재

---

## 현재 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ VERIFIED | 테스트 통과 (코드 수준 검증 완료) |
| ✅ STRUCTURE-OK | 코드/파일 구조가 올바름 |
| 🔶 INTEGRATION-GAP | 순수 함수는 통과하나 모듈 간 연동 미검증 |
| 🔶 PARTIAL | 일부만 구현/검증 |
| ❌ NOT-DONE | 구현 또는 검증되지 않음 |
| ⬜ NEEDS-INFRA | 물리 인프라에서만 검증 가능 |
| ⬜ NEEDS-USER | 사용자 수동 작업 필요 |

---

## Checklist 상세 검증 (15개 항목)

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

**상태: ✅ 순수 함수 / 🔶 INTEGRATION-GAP / ⬜ NEEDS-INFRA (E2E)**

| 구성 요소 | 상태 | 근거 |
|-----------|------|------|
| `sdi-specs.yaml` 파싱/검증 | ✅ | 모델 파싱, pool 검증, IP 중복, 리소스 0 체크 |
| HCL 생성 (`generate_tofu_main`) | ✅ | provider, VM 정의, ssh_user, VFIO, single-node |
| Host-infra HCL 생성 | ✅ | 단일/다중 노드, 멱등성 |
| `sdi init` no-flag → spec 파이프라인 데이터 연속성 | 🔶 | 개별 함수 통과하나 no-flag→spec 전환 시 데이터 흐름 미검증 |
| `tofu init/apply` 실제 실행 | ⬜ | 물리 인프라 필요 |

**아키텍처 정합성**: 4개 노드 → resource_pool → sdi_pools(tower+sandbox) → 2개 클러스터 ✅

---

### #2. CF Tunnel GitOps 배포 — ✅ VERIFIED

- `gitops/tower/cloudflared-tunnel/kustomization.yaml` — Helm chart ✅
- `gitops/tower/cloudflared-tunnel/values.yaml` — tunnel `playbox-admin-static` ✅
- `gitops/generators/tower/tower-generator.yaml` — sync wave 3 ✅

---

### #3. CF Tunnel 완성도 — 🔶 PARTIAL

**자동 처리**: Helm 배포(ArgoCD), K8s Secret 생성(`scalex secrets apply`), Ingress 3개

**사용자 필수 작업**:
1. ⬜ Cloudflare Dashboard에서 tunnel `playbox-admin-static` 생성
2. ⬜ Credentials JSON → `credentials/cloudflare-tunnel.json`
3. ⬜ Public Hostname: `cd.jinwang.dev`, `auth.jinwang.dev`, `api.k8s.jinwang.dev`
4. ⬜ DNS CNAME 자동 생성 확인

**문서화**: `docs/ops-guide.md` 상세 가이드 ✅

---

### #4. CLI Rust 구현 + FP 원칙 — ✅ VERIFIED

| 항목 | 상태 |
|------|------|
| Rust 구현 (`scalex-cli/`) | ✅ 27개 .rs, 8개 서브커맨드 |
| clap derive CLI | ✅ |
| 순수 함수 분리 (`generate_*` / `run_*`) | ✅ |
| thiserror 에러 처리 | ✅ |
| 505 tests, 0 clippy warnings, fmt OK | ✅ |

---

### #5. 사용자 친절한 가이드 — ✅ VERIFIED

README, docs/ops-guide.md, docs/SETUP-GUIDE.md, docs/TROUBLESHOOTING.md, `--help`, `--dry-run` 전 커맨드 지원

---

### #6. README.md 상세 내용 — ✅ VERIFIED

Architecture, Design Philosophy(7원칙), Installation Guide(Step 0~8), CLI Reference, GitOps Pattern, Project Structure, Testing, Documentation 링크

---

### #7. Installation Guide E2E — 🔶 PARTIAL

- Step 0~8 논리적 흐름 ✅
- Pre-flight SSH 테스트 가이드 ✅
- **실제 초기화된 베어메탈에서 끝까지 실행한 적 없음** ❌
- **Step 4→5→7 연쇄 경로 실환경 미검증** ⬜

---

### #8. CLI 기능 전체 — ✅ 순수 함수 / 🔶 INTEGRATION-GAP

| 명령어 | 순수 함수 | 모듈 간 연동 | 실환경 |
|--------|:---------:|:-----------:|:------:|
| `scalex facts` | ✅ | 🔶 | ⬜ |
| `scalex sdi init` (no flag) | ✅ | 🔶 | ⬜ |
| `scalex sdi init <spec>` | ✅ | 🔶 | ⬜ |
| `scalex sdi clean --hard` | ✅ | ✅ | ⬜ |
| `scalex sdi sync [--force]` | ✅ | ✅ | ⬜ |
| `scalex cluster init <config>` | ✅ | 🔶 | ⬜ |
| `scalex get baremetals/sdi-pools/clusters/config-files` | ✅ | 🔶 | ⬜ |
| `scalex secrets apply` | ✅ | ✅ | ⬜ |
| `scalex bootstrap` | ✅ | ✅ | ⬜ |
| `scalex status` | ✅ | ✅ | ⬜ |
| `scalex kernel-tune` | ✅ | ✅ | ⬜ |

**`.baremetal-init.yaml` 포맷**: 3가지 SSH 접근 (direct, external IP, ProxyJump) + 2가지 인증 (password, key) ✅

---

### #9. Baremetal 확장성 — ✅ VERIFIED

`cluster_mode: "baremetal"` 옵션, `generate_inventory_baremetal()`, SDI/baremetal 혼합 ✅. k3s 참조 없음 ✅.

---

### #10. 시크릿 템플릿화 — ✅ VERIFIED

`credentials/*.example` 4개, `.gitignore`, `scalex secrets apply` ✅

---

### #11. 커널 파라미터 튜닝 — ✅ VERIFIED

`scalex kernel-tune`, 역할별 추천, diff, Ansible task YAML 생성 ✅

---

### #12. 디렉토리 구조 — ✅ STRUCTURE-OK

```
scalex-cli/           ✅ Rust CLI (505 tests)
gitops/               ✅ ArgoCD multi-cluster
  bootstrap/          ✅ spread.yaml
  generators/         ✅ tower/ + sandbox/
  projects/           ✅ tower-project + sandbox-project
  common/             ✅ cilium-resources, cert-manager, kyverno, kyverno-policies
  tower/              ✅ argocd, cert-issuers, cilium, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy
  sandbox/            ✅ cilium, cluster-config, local-path-provisioner, rbac, test-resources
credentials/          ✅ .example 템플릿
config/               ✅ sdi-specs, k8s-clusters 예제
docs/                 ✅ 7개 문서
ansible/              ✅ node preparation
kubespray/            ✅ submodule v2.30.0
client/               ✅ OIDC kubeconfig
tests/                ✅ run-tests.sh
```

---

### #13. 멱등성 — ✅ 순수 함수 / ⬜ NEEDS-INFRA

순수 함수 멱등성 5개 테스트 ✅. 실행 수준 (sdi init → clean → sdi init 사이클) ⬜.

---

### #14. 외부 kubectl 접근 — 🔶 PARTIAL

| 방법 | 상태 | 조건 |
|------|------|------|
| LAN 직접 | ⬜ | 동일 네트워크 + kubeconfig |
| Tailscale | ⬜ | Tailscale 설치, kubeconfig server=Tailscale IP |
| CF Tunnel + OIDC | ❌ | Keycloak Realm/Client 설정 필수 (미완료) |
| SOCKS5 Proxy | 🔶 | manifest 존재, 미검증 |

**핵심**: Keycloak 없이 CF Tunnel 경유 외부 kubectl **불가능**. Pre-OIDC 외부 접근은 **Tailscale만 가능**.

---

### #15. NAT 접근 경로 — ✅ VERIFIED

클러스터 NAT 내부, 외부: Tailscale + CF Tunnel, LAN: 스위치 접근 ✅

---

## 근본 원인 분석

| 원인 | 영향 | 해결 가능성 |
|------|------|:----------:|
| 482개 전부 순수 함수 → 모듈 간 연동 0% | 개별 OK인데 파이프라인은 미검증 | ✅ 코드로 해결 |
| config example 파일 round-trip 미검증 | 사용자가 example 복사 후 파서 에러 가능 | ✅ 코드로 해결 |
| `sdi init` no-flag → spec 전환 데이터 흐름 미검증 | 2-layer 관리 체계의 핵심 연동 미보장 | ✅ 코드로 해결 |
| `scalex get config-files` 출력 완전성 미검증 | 사용자가 누락된 파일을 발견 못할 수 있음 | ✅ 코드로 해결 |
| CF Tunnel + kubectl 제약 (OIDC 필수) | 외부 접근 미완성 | ⬜ 인프라 필요 |
| 실환경 E2E 미검증 | 전체 파이프라인 보장 불가 | ⬜ 인프라 필요 |

---

## 실행 계획: 코드 수준에서 해결 가능한 갭 해소

### Sprint 33: 모듈 간 Integration 테스트 + Config Round-trip ✅ (완료)

**목표**: 개별 순수 함수를 넘어 **모듈 간 데이터 흐름**이 올바른지 검증

#### 33a — Config Example Round-trip 테스트 (+4 tests) ✅
- [x] `sdi-specs.yaml.example` → `SdiSpec` 파싱 → pool 이름/노드 수 검증
- [x] `k8s-clusters.yaml.example` → `K8sClustersConfig` 파싱 → cluster 이름/모드 검증
- [x] `.baremetal-init.yaml.example` → `BaremetalInitConfig` 파싱 (`.env.example`과 함께) → 노드 수/접근방식 검증
- [x] 두 config 간 cross-reference: `k8s-clusters.yaml`의 `cluster_sdi_resource_pool` ↔ `sdi-specs.yaml`의 `pool_name` 정합

#### 33b — SDI Pipeline Integration 테스트 (+4 tests) ✅
- [x] `sdi init` no-flag → `build_host_infra_inputs()` → `generate_tofu_host_infra()` end-to-end 데이터 흐름
- [x] `sdi init <spec>` → `generate_tofu_main()` → pool state → `build_pool_state()` 파이프라인 연속성
- [x] `sdi init` → `cluster init` 연쇄: sdi-spec-cache.yaml 생성 → cluster init에서 로드 가능한지
- [x] resource_pool 요약 → `scalex get sdi-pools` 출력 포맷 정합성

#### 33c — Cluster Pipeline Integration 테스트 (+4 tests) ✅
- [x] `cluster init` → `generate_inventory()` + `generate_cluster_vars()` → Kubespray 호환 포맷 검증
- [x] tower + sandbox 2-cluster: inventory가 겹치지 않는 IP 사용하는지
- [x] `find_control_plane_ip()` → `update_gitops_cilium_values()` → values.yaml 내용 정합
- [x] `scalex get clusters` 출력이 config의 모든 클러스터를 포함하는지

#### 33d — `scalex get config-files` 완전성 테스트 (+3 tests) ✅
- [x] 4개 필수 파일 존재 시 모두 OK/Present 출력
- [x] 1개 누락 시 Missing 출력 + 나머지 정상
- [x] YAML 파싱 실패 시 구체적 에러 메시지

#### 33e — 커밋/푸시
- [x] 482 + 15 = 497 테스트 확인
- [x] DASHBOARD.md 테스트 카운트 업데이트
- [ ] 커밋 + 푸시

---

### Sprint 34: SOCKS5 Proxy 검증 + External Access 문서 보강 ✅ (완료)

#### 34a — SOCKS5 Proxy GitOps 구조 검증 (+3 tests) ✅
- [x] `gitops/tower/socks5-proxy/` manifest YAML 파싱 유효성 (Deployment + Service)
- [x] sync wave 3 설정 확인 (tower-generator.yaml)
- [x] Service port 1080 + ClusterIP 보안 검증

#### 34b — External Access 경로 문서 정합성 (+2 tests) ✅
- [x] README External Access 테이블 — 4가지 방법 모두 포함 확인 (LAN, Tailscale, CF Tunnel, SOCKS5)
- [x] `docs/ops-guide.md`에 Tailscale + SOCKS5 + LAN + Pre-OIDC 가이드 존재 확인

#### 34c — README Installation Guide 완전성 감사 (+3 tests) ✅
- [x] Step 0~8 모든 단계에서 참조하는 파일이 실제 존재하는지 (include_str! 컴파일 타임 검증)
- [x] CLI 8개 서브커맨드 + get 4개 서브커맨드가 README CLI Reference에 모두 존재
- [x] README Project Structure에 10개 디렉토리 + gitops 6개 서브디렉토리 매칭

#### 34d — 커밋/푸시
- [x] 테스트 카운트 업데이트 (497 → 505)
- [ ] 커밋 + 푸시

---

### Sprint 35: sdi sync 복합 시나리오 + 멱등성 강화

#### 35a — sdi sync 복합 상태 전이 (+4 tests)
- [ ] 동시 add + remove: 2개 노드 추가 + 1개 노드 제거
- [ ] sync 후 resource pool 재계산 정합
- [ ] conflict severity escalation: medium → high → critical
- [ ] `--force` 플래그로 critical conflict 무시 시 경고 메시지

#### 35b — 멱등성 파이프라인 테스트 (+3 tests)
- [ ] `generate_tofu_main()` 2회 호출 → 동일 출력
- [ ] `generate_inventory()` 2회 호출 → 동일 출력
- [ ] `generate_cluster_vars()` 2회 호출 → 동일 출력 (입력이 같으면)

#### 35c — 커밋/푸시

---

### Sprint 36+: 실환경 E2E (⬜ 인프라 필요)

#### 36a — SDI 가상화 E2E
- [ ] `scalex facts --all` → 4노드 SSH + JSON
- [ ] `scalex sdi init config/sdi-specs.yaml` → VM 5개 생성
- [ ] `scalex get sdi-pools` → 2개 풀
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` → 초기화
- [ ] 재실행 (멱등성)

#### 36b — Kubespray + ArgoCD E2E
- [ ] `scalex cluster init` → tower + sandbox
- [ ] `kubectl get nodes` 양쪽 정상
- [ ] `scalex secrets apply` → K8s secrets
- [ ] `scalex bootstrap` → ArgoCD + spread.yaml
- [ ] `kubectl -n argocd get applications` → Synced/Healthy

#### 36c — 외부 접근 E2E
- [ ] Tailscale IP → tower kubectl
- [ ] Keycloak Realm/Client 설정 (사용자 수동)
- [ ] CF Tunnel OIDC kubectl
- [ ] `https://cd.jinwang.dev` ArgoCD UI

---

## 완료된 Sprint 기록

### Sprint 18~25: 초기 테스트 인프라 (388 → 417 tests)
- CLI 실행 경로, CF Tunnel, SSH 접근 방식, 구조 검증, 엣지 케이스

### Sprint 29~30: 네트워크 검증 + Sync 안전성 (417 → 445 tests)
- Pod/Service CIDR 중복, ConflictSeverity, sdi sync --force, CIDR overlap

### Sprint 31: 미테스트 순수 함수 보강 (445 → 459 tests)
- `build_host_infra_inputs`, `build_pool_state`, `find_control_plane_ip` 엣지 케이스

### Sprint 32: 순수 함수 리팩토링 (459 → 482 tests)
- GPU passthrough 순수 함수 분리, facts 파서 엣지 케이스, secrets 엣지 케이스

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
  ⚠ client cert는 CF Tunnel 통과 불가 (TLS 종단)

[Tailscale] (Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) → Tailscale IP → kube-apiserver

[SOCKS5 Proxy] (LAN/Tailscale 내부 편의)
  kubectl --proxy-url socks5://tower-ip:1080 → kube-apiserver

[LAN] (모든 인증 방식)
  kubectl → LAN IP → kube-apiserver
```

---

## Test Summary

| Module | Tests | 주요 커버리지 |
|--------|:-----:|----------|
| core/validation | 95+ | pool mapping, cluster IDs/names, CIDR overlap, DNS, bootstrap |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium |
| core/kubespray | 32+ | inventory (SDI + baremetal), cluster vars, OIDC |
| commands/sdi | 31 | network resolve, host infra, pool state, clean, GPU |
| commands/status | 21 | platform status |
| core/secrets | 18 | K8s secret generation, edge cases |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14+ | ArgoCD helm, cluster add, kubectl apply |
| commands/facts | 14 | facts gathering, parse edge cases |
| core/sync | 13 | sync diff, VM conflict, severity |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table |
| core/ssh | 5 | SSH command building |
| **TOTAL** | **505** | **순수 함수 + 구조 + Integration + GitOps/문서 검증 (Sprint 33-34)** |
