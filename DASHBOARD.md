# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical -> SDI (OpenTofu) -> Node Pools -> Cluster (Kubespray) -> GitOps (ArgoCD)

---

## Phase 0: 이전 DASHBOARD 비판적 분석

### 핵심 문제: "512개 테스트" = "동작하는 시스템"이 아니다

이전 DASHBOARD는 Sprint 35까지 512개 테스트를 확보했으나, **사용자의 Checklist 15개 항목을 실질적으로 달성했는지 검증하지 않았다.**

#### 비판 1: 테스트가 순수 함수 출력만 검증 — 사용자 워크플로우 미검증

512개 테스트 전부 `#[cfg(test)]` 인라인 단위 테스트이다.
- `generate_*` (순수 함수) -> 출력 문자열 비교만 수행
- `run_*` (I/O, 실행) -> 0% 커버리지
- **사용자가 실제로 수행할 워크플로우** 미검증:
  - `scalex facts --all` -> SSH 연결 -> JSON 저장
  - `scalex sdi init` -> facts 확인 -> HCL 생성 -> tofu apply
  - `scalex sdi init <spec>` -> pool 생성 -> 상태 저장
  - `scalex cluster init <config>` -> inventory + vars -> kubespray -> kubeconfig
  - `scalex bootstrap` -> ArgoCD Helm -> cluster register -> spread.yaml

#### 비판 2: "NEEDS-INFRA" 면책 조항으로 코드 수준 갭 은폐

이전 DASHBOARD는 미검증 항목을 "물리 인프라 부재"로 전가했으나, **인프라 없이도 검증 가능한 갭**이 다수 존재:

| 검증 가능한 갭 | 설명 | 해소 Sprint |
|----------------|------|:-----------:|
| 단일 노드 SDI | CL-1 철학 "단일 노드에도 설치 가능" | **Sprint 36 ✅** |
| Dry-run E2E 파이프라인 | `--dry-run`으로 전체 경로 데이터 흐름 검증 가능 | **Sprint 37 ✅** |
| Config 누락 시 에러 UX | 사용자가 `.example` 복사 안 했을 때의 에러 메시지 품질 | **Sprint 38 ✅** |
| 2-Layer cross-validation | SDI↔K8s 계층 간 정합성 | **Sprint 39 ✅** |
| Example 파일 end-to-end round-trip | .example 파일들이 전체 파이프라인을 실제로 통과하는지 | **Sprint 37 ✅** |

#### 비판 3: 사용자 Checklist 항목별 깊이 불균등

| 깊이 수준 | 항목 | 검증 방법 |
|-----------|------|----------|
| 존재 확인만으로 OK | #4(Rust), #10(시크릿), #12(디렉토리) | 파일/코드 존재 |
| 코드 수준 검증 가능 | #1(SDI), #8(CLI 전체), #13(멱등성) | 단위+통합 테스트 |
| 인프라 필수 | #7(E2E), #14(외부 kubectl) | 실환경 실행 |

#### 비판 4: 단일 노드 지원 미검증 (CL-1 핵심 철학 위반) → **Sprint 36에서 해소**

#### 비판 5: `sdi sync`의 복잡한 시나리오 부족 → **Sprint 35+39에서 부분 해소**

---

## 현재 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ VERIFIED | 테스트 통과 + 코드 수준 검증 완료 |
| ✅ STRUCTURE-OK | 코드/파일 구조 올바름 |
| 🔶 PARTIAL | 일부만 구현/검증 |
| ⬜ NEEDS-INFRA | 물리 인프라에서만 검증 가능 |
| ⬜ NEEDS-USER | 사용자 수동 작업 필요 |

---

## Checklist 상세 검증 (15개 항목)

### CL-1. SDI 가상화 (4노드 -> 리소스 풀 -> 2 클러스터)

**상태: ✅ VERIFIED (Sprint 36에서 단일 노드 테스트 추가)**

| 구성 요소 | 상태 | 근거 |
|-----------|------|------|
| `sdi-specs.yaml` 파싱/검증 | ✅ | pool 검증, IP 중복, 리소스 0 체크 |
| HCL 생성 (`generate_tofu_main`) | ✅ | provider, VM, ssh_user, VFIO |
| Host-infra HCL 생성 | ✅ | 단일/다중 노드, 멱등성 |
| 단일 노드 단일 풀 SDI | ✅ | **Sprint 36: 5 tests** |
| SDI→inventory IP 일관성 | ✅ | **Sprint 36: IP consistency test** |
| `sdi init` no-flag -> spec 전환 데이터 흐름 | ✅ | **Sprint 37: pipeline test** |
| `tofu init/apply` 실행 | ⬜ | 물리 인프라 필요 |

### CL-2. CF Tunnel GitOps 배포 -- ✅ VERIFIED

- `gitops/tower/cloudflared-tunnel/` Helm chart + values.yaml ✅
- `tower-generator.yaml` sync wave 3 ✅

### CL-3. CF Tunnel 완성도 -- 🔶 PARTIAL

**자동 처리**: Helm 배포(ArgoCD), K8s Secret 생성(`scalex secrets apply`), Ingress
**사용자 필수 작업**:
1. ⬜ Cloudflare Dashboard에서 tunnel `playbox-admin-static` 생성
2. ⬜ Credentials JSON -> `credentials/cloudflare-tunnel.json`
3. ⬜ Public Hostname: `cd.jinwang.dev`, `auth.jinwang.dev`, `api.k8s.jinwang.dev`
4. ⬜ DNS CNAME 확인

### CL-4. CLI Rust 구현 + FP 원칙 -- ✅ VERIFIED

| 항목 | 상태 |
|------|------|
| Rust 구현 (`scalex-cli/`) | ✅ 27 .rs 파일, 8 서브커맨드 |
| clap derive CLI | ✅ |
| 순수 함수/I/O 분리 (`generate_*` / `run_*`) | ✅ |
| thiserror 에러 처리 | ✅ |
| 537 tests, 0 clippy warnings, fmt OK | ✅ |

### CL-5. 사용자 친절한 가이드 -- ✅ VERIFIED (Sprint 38 강화)

README, docs/ops-guide.md, `--help`, `--dry-run` 전 커맨드 지원.
**Sprint 38**: `format_config_not_found()`, `format_yaml_parse_error()`, `format_validation_errors()` 추가.

### CL-6. README.md 상세 내용 -- ✅ VERIFIED

Architecture, Philosophy(7원칙), Installation(Step 0~8), CLI Reference, GitOps Pattern

### CL-7. Installation Guide E2E -- 🔶 PARTIAL (Sprint 37 강화)

- Step 0~8 논리적 흐름 ✅
- 각 Step 참조 파일 존재 확인 (컴파일타임) ✅
- **Dry-run 모드로 전체 파이프라인 데이터 흐름 검증** ✅ (Sprint 37: 5 tests)
- **실제 초기화된 베어메탈에서 끝까지 실행한 적 없음** ⬜ NEEDS-INFRA

### CL-8. CLI 기능 전체

| 명령어 | 순수 함수 | 통합 테스트 | 실환경 |
|--------|:---------:|:-----------:|:------:|
| `scalex facts` | ✅ | ✅ | ⬜ |
| `scalex sdi init` (no flag) | ✅ | ✅ | ⬜ |
| `scalex sdi init <spec>` | ✅ | ✅ | ⬜ |
| `scalex sdi clean --hard` | ✅ | ✅ | ⬜ |
| `scalex sdi sync` | ✅ | ✅ | ⬜ |
| `scalex cluster init` | ✅ | ✅ | ⬜ |
| `scalex get baremetals/sdi-pools/clusters/config-files` | ✅ | ✅ | ⬜ |
| `scalex secrets apply` | ✅ | ✅ | ⬜ |
| `scalex bootstrap` | ✅ | ✅ | ⬜ |
| `scalex status` | ✅ | ✅ | ⬜ |
| `scalex kernel-tune` | ✅ | ✅ | ⬜ |

### CL-9. Baremetal 확장성 -- ✅ VERIFIED

`cluster_mode: "baremetal"` 옵션, `generate_inventory_baremetal()`, k3s 참조 없음

### CL-10. 시크릿 템플릿화 -- ✅ VERIFIED

`credentials/*.example` 4개, `.gitignore`, `scalex secrets apply`

### CL-11. 커널 파라미터 튜닝 -- ✅ VERIFIED

`scalex kernel-tune`, 역할별 추천, diff, Ansible task 생성

### CL-12. 디렉토리 구조 -- ✅ STRUCTURE-OK

```
scalex-cli/           ✅ Rust CLI (537 tests)
gitops/               ✅ ArgoCD multi-cluster
  bootstrap/          ✅ spread.yaml
  generators/         ✅ tower/ + sandbox/
  projects/           ✅ AppProjects
  common/             ✅ cilium-resources, cert-manager, kyverno, kyverno-policies
  tower/              ✅ argocd, cert-issuers, cilium, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy
  sandbox/            ✅ cilium, cluster-config, local-path-provisioner, rbac, test-resources
credentials/          ✅ .example 템플릿
config/               ✅ sdi-specs, k8s-clusters 예제
docs/                 ✅ 운영 가이드
ansible/              ✅ node preparation
kubespray/            ✅ submodule
client/               ✅ OIDC kubeconfig
tests/                ✅ run-tests.sh
```

### CL-13. 멱등성 -- ✅ 순수 함수(Sprint 35) / ⬜ NEEDS-INFRA(실행)

순수 함수 멱등성: `generate_tofu_main`, `generate_inventory`, `generate_cluster_vars` 2회 호출 동일 출력 ✅

### CL-14. 외부 kubectl 접근 -- 🔶 PARTIAL

| 방법 | 상태 | 조건 |
|------|------|------|
| LAN 직접 | ⬜ | 동일 네트워크 + kubeconfig |
| Tailscale | ⬜ | Tailscale + kubeconfig (server=TS IP) |
| CF Tunnel + OIDC | ❌ | **Keycloak 미설정** |
| SOCKS5 Proxy | 🔶 | manifest 존재, 미검증 |

**핵심**: Keycloak 없이 CF Tunnel 경유 외부 kubectl **불가**. Pre-OIDC 외부 접근은 **Tailscale만 가능**.

### CL-15. NAT 접근 경로 -- ✅ VERIFIED

클러스터 NAT 내부, 외부: Tailscale + CF Tunnel, LAN: 스위치 접근. README/docs 문서화 완료.

---

## 근본 원인 분석

| # | 원인 | 영향 | 해소 |
|---|------|------|:----:|
| R1 | 단일 노드 SDI 미테스트 | CL-1 핵심 철학 위반 | **Sprint 36 ✅** |
| R2 | Dry-run E2E 파이프라인 미검증 | CL-7 Installation Guide 신뢰도 0 | **Sprint 37 ✅** |
| R3 | Config 누락 시 에러 UX 미검증 | CL-5 뉴비 친화성 약화 | **Sprint 38 ✅** |
| R4 | 2-Layer 관리 체계 end-to-end 미검증 | CL-6 철학 #6 미달성 | **Sprint 39 ✅** |
| R5 | `sdi sync` -> `get sdi-pools` 연쇄 미검증 | CL-8 sync 명령 신뢰도 낮음 | **Sprint 39 ✅** |
| R6 | CF Tunnel + kubectl 제약 (OIDC 필수) | CL-14 외부 접근 미완성 | ⬜ 인프라 |
| R7 | 실환경 E2E 미검증 | CL-7 보장 불가 | ⬜ 인프라 |

---

## 실행 계획

### Sprint 36: 단일 노드 SDI 지원 테스트 (R1 해소) ✅ DONE

**결과**: 512 → 517 tests (+5)
- SDI 단일 노드 all-roles inventory 생성 ✅
- SDI spec ↔ inventory IP 일관성 검증 ✅
- SDI pool에 control-plane 없을 때 거부 ✅
- 존재하지 않는 pool 이름 에러 처리 ✅
- 최소 파이프라인: 1 bare-metal → 1 SDI pool → inventory + cluster vars ✅

### Sprint 37: Dry-run E2E 파이프라인 테스트 (R2 해소) ✅ DONE

**결과**: 517 → 522 tests (+5)
- Example configs 전체 dry-run: load → validate → inventory + vars ✅
- Tower/Sandbox inventory 구별성 (IP 중복 없음) ✅
- Cluster vars 구별성 (CIDR, 이름, Cilium ID) ✅
- 단일 노드 E2E 파이프라인 ✅
- SDI spec → OpenTofu HCL 파이프라인 ✅

### Sprint 38: Config 에러 UX 개선 (R3 해소) ✅ DONE

**결과**: 522 → 531 tests (+9)
- `format_config_not_found()`: `.example` 파일 복사 명령 제안 ✅
- `validate_config_file_exists()`: 존재 확인 + 친절한 에러 ✅
- `format_yaml_parse_error()`: YAML 파싱 에러 + 파일 컨텍스트 ✅
- `format_validation_errors()`: 복수 에러 요약 + 개수 표시 ✅

### Sprint 39: 2-Layer Cross-Validation 강화 (R4+R5 해소) ✅ DONE

**결과**: 531 → 537 tests (+6)
- `validate_two_layer_consistency()`: SDI↔K8s 계층 간 cross-validation ✅
- Control-plane 역할 필수 검증 ✅
- Node IP ↔ Pod/Service CIDR 충돌 감지 ✅
- Orphan pool (미참조 SDI pool) 경고 ✅
- `ip_in_cidr()` helper + edge case 테스트 ✅

---

### Sprint 41+: 실환경 E2E (⬜ NEEDS-INFRA)

> 이 Sprint부터는 물리 인프라가 필요합니다.

#### 41a -- SDI 가상화 E2E
- [ ] `scalex facts --all` -> 4노드 SSH + JSON
- [ ] `scalex sdi init config/sdi-specs.yaml` -> VM 5개 생성
- [ ] `scalex get sdi-pools` -> 2개 풀
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` -> 초기화 후 재실행 (멱등성)

#### 41b -- Kubespray + ArgoCD E2E
- [ ] `scalex cluster init` -> tower + sandbox
- [ ] `kubectl get nodes` 양쪽 정상
- [ ] `scalex secrets apply` + `scalex bootstrap`
- [ ] ArgoCD Applications Synced/Healthy

#### 41c -- 외부 접근 E2E (CL-14)
- [ ] Tailscale IP -> tower kubectl
- [ ] Keycloak Realm/Client 설정 (사용자 수동)
- [ ] CF Tunnel OIDC kubectl
- [ ] `https://cd.jinwang.dev` ArgoCD UI

---

## 완료된 Sprint 기록

### Sprint 18~25: 초기 테스트 인프라 (388 -> 417 tests)
- CLI 실행 경로, CF Tunnel, SSH 접근 방식, 구조 검증, 엣지 케이스

### Sprint 29~30: 네트워크 검증 + Sync 안전성 (417 -> 445 tests)
- Pod/Service CIDR 중복, ConflictSeverity, sdi sync --force, CIDR overlap

### Sprint 31: 미테스트 순수 함수 보강 (445 -> 459 tests)
- `build_host_infra_inputs`, `build_pool_state`, `find_control_plane_ip` 엣지 케이스

### Sprint 32: 순수 함수 리팩토링 (459 -> 482 tests)
- GPU passthrough 순수 함수 분리, facts 파서 엣지 케이스, secrets 엣지 케이스

### Sprint 33: 모듈 간 Integration + DASHBOARD 비판 (482 -> 497 tests)
- Config example round-trip, SDI/Cluster pipeline integration, config-files 완전성

### Sprint 34: SOCKS5/GitOps/문서 검증 (497 -> 505 tests)
- SOCKS5 proxy manifest, External access 문서, README Installation Guide 완전성

### Sprint 35: sdi sync 복합 시나리오 + 멱등성 (505 -> 512 tests)
- 복합 상태 전이 (add+remove+conflict), 멱등성 파이프라인

### Sprint 36: 단일 노드 SDI 지원 (512 -> 517 tests)
- SDI single-node all-roles inventory, IP consistency, missing pool error

### Sprint 37: Dry-run E2E 파이프라인 (517 -> 522 tests)
- Full pipeline dry-run, distinct inventories/vars, SDI→HCL pipeline

### Sprint 38: Config 에러 UX (522 -> 531 tests)
- format_config_not_found, validate_config_file_exists, YAML parse error, validation errors

### Sprint 39: 2-Layer Cross-Validation (531 -> 537 tests)
- validate_two_layer_consistency, ip_in_cidr, orphan pool, control-plane required

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
|  -> secrets apply -> bootstrap            |
|  get baremetals/sdi-pools/clusters        |
|  status / kernel-tune                     |
+-------------------------------------------+
        |
        v
_generated/
+-- facts/          (hardware JSON per node)
+-- sdi/            (OpenTofu HCL + state)
|   +-- host-infra/ (no-flag: host-level libvirt infra)
|   +-- main.tf     (spec: VM pool HCL)
+-- clusters/       (inventory.ini + vars + kubeconfig per cluster)
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
|    -> projects/ (AppProjects)             |
|    -> generators/{tower,sandbox}/         |
|    -> common/ tower/ sandbox/             |
+-------------------------------------------+
```

### External Access Paths

```
[CF Tunnel + OIDC] (Keycloak 설정 완료 후에만 kubectl 가능)
  kubectl (OIDC token) -> CF Edge -> cloudflared -> kube-apiserver
  ! client cert는 CF Tunnel 통과 불가 (TLS 종단)

[Tailscale] (Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) -> Tailscale IP -> kube-apiserver

[SOCKS5 Proxy] (LAN/Tailscale 내부 편의)
  kubectl --proxy-url socks5://tower-ip:1080 -> kube-apiserver

[LAN] (모든 인증 방식)
  kubectl -> LAN IP -> kube-apiserver
  * 스위치 접근으로 LAN 진입 -> 직접 kubectl 가능
```

---

## Test Summary (537 tests total)

| Module | Tests | 주요 커버리지 |
|--------|:-----:|----------|
| core/validation | 101+ | pool mapping, cluster IDs/names, CIDR overlap, DNS, bootstrap, 2-layer consistency |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium |
| core/kubespray | 38 | inventory (SDI + baremetal), cluster vars, OIDC, single-node SDI |
| commands/sdi | 31 | network resolve, host infra, pool state, clean, GPU |
| commands/cluster | 28 | cluster init, SDI/baremetal, gitops, E2E pipeline |
| commands/status | 21 | platform status |
| core/secrets | 18 | K8s secret generation, edge cases |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 24 | baremetal config, semantic validation, error UX |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14+ | ArgoCD helm, cluster add, kubectl apply |
| commands/facts | 14 | facts gathering, parse edge cases |
| core/sync | 13 | sync diff, VM conflict, severity |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table |
| core/ssh | 5 | SSH command building |
