# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 근본적 비판: 왜 이전 작업은 Checklist를 달성하지 못했는가

### 비판 1: "코드 수준 검증"을 "달성"으로 혼동

이전 작업자는 601개 테스트를 근거로 15개 Checklist 중 12개를 "VERIFIED"로 표시했다.
그러나 이 테스트들은 **순수 함수의 입출력 정확성**만 검증한다:

| 테스트가 검증하는 것 | 테스트가 검증하지 않는 것 |
|---|---|
| `generate_tofu_main()` → 유효한 HCL 문자열 | `tofu apply` → 실제 VM 생성 |
| `generate_inventory()` → inventory.ini 문자열 | `ansible-playbook` → K8s 클러스터 구축 |
| `generate_cluster_vars()` → YAML 문자열 | Kubespray가 이 vars를 수용하는지 |
| `generate_argocd_helm_install_args()` → 인수 목록 | ArgoCD가 실제로 Sync 되는지 |

**결론**: 순수 함수 테스트 ≠ 인프라 달성. "코드는 작성했다"와 "동작한다"는 전혀 다르다.

### 비판 2: 파이프라인 체인 미검증 → ✅ Sprint 45에서 보강

~~각 단계의 출력이 다음 단계의 입력으로 올바르게 흘러가는지 체인 테스트가 없다.~~

**수정 (Sprint 45)**: 3개 cross-module chain test 추가 (601→604 tests):
```
facts output JSON → sdi init 소비 가능 → ✅ test_chain_facts_json_roundtrip_for_sdi_consumption
sdi-specs.yaml pool_name → k8s-clusters.yaml 매핑 → ✅ 기존 validate_cluster_sdi_pool_mapping
sdi init HCL의 VM IP → inventory의 ansible_host IP → ✅ test_chain_sdi_vm_ips_flow_into_inventory_ansible_host
cluster init inventory → Kubespray 형식 → ✅ 기존 test_full_dryrun_pipeline_both_configs (섹션 검증)
bootstrap args → helm/kubectl 명령 형식 → ✅ test_chain_bootstrap_args_reference_generated_kubeconfig_path + 기존 8개 bootstrap 테스트
```

### 비판 3: 멱등성 검증 오류

"순수 함수 2회 호출 → 동일 출력" = **결정론(determinism)**, NOT **멱등성(idempotency)**.
진정한 멱등성: `sdi init` 2회 실행 → OpenTofu state 불변 + VM 상태 동일. 이는 코드만으로 증명 불가.

### 비판 4: 단일 노드 시나리오 → ✅ 검증됨

~~Checklist 핵심 철학의 "단일 노드에도 설치할 수 있는 구조"에 대한 테스트가 없다.~~

**수정**: 17개 이상의 단일 노드 테스트가 이미 존재한다:
- `test_generate_tofu_host_infra_single_node`, `test_single_node_sdi_all_pools_on_one_host` (tofu.rs)
- `test_generate_inventory_baremetal_single_node_dual_role`, `test_generate_inventory_sdi_single_node_all_roles` (kubespray.rs)
- `test_single_node_sdi_tower_and_sandbox_on_one_host`, `test_single_node_sdi_full_pipeline` (validation.rs)
- `test_single_node_sdi_two_clusters_two_layer_consistency` (validation.rs)
- `test_dryrun_single_node_pipeline_end_to_end` (cluster.rs)
- 기타 resource_pool, sync, sdi 모듈의 단일 노드 테스트들

### 비판 5: `config/` 파일이 실데이터로 커밋됨 → ✅ 수정 완료

~~`config/sdi-specs.yaml`과 `config/k8s-clusters.yaml`이 `.gitignore`에 포함되지 않아 실데이터가 커밋될 위험이 있었다.~~

**수정 (Sprint 44)**: `.gitignore`에 `config/sdi-specs.yaml`과 `config/k8s-clusters.yaml` 추가 완료.
향후 `git rm --cached`로 추적 해제 + `.example` 파일만 커밋 상태로 전환 필요.

### 비판 6: CIDR 겹침 → ✅ 검증됨

~~멀티-클러스터 환경에서 CIDR 겹침 검증 테스트가 없다.~~

**수정**: 30개 이상의 CIDR 겹침 테스트가 이미 존재한다:
- `cidrs_overlap()` 함수 (validation.rs:223) + 15개 유닛 테스트 (`test_cidrs_overlap_*`, `test_cidrs_no_overlap_*`)
- `validate_cluster_network_overlap()` + 8개 통합 테스트 (`test_network_overlap_detects_pod_cidr_collision` 등)
- `test_example_configs_pass_network_overlap_validation` — 실제 config 파일 기반 검증
- `test_network_overlap_wired_into_cluster_init_pipeline` — 파이프라인 통합 확인
- `test_two_layer_node_ip_in_pod_cidr_detected` — node IP가 pod CIDR 범위와 충돌하는 경우까지 검증

---

## Checklist 재평가 (정직한 현황)

### 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ CODE | 순수 함수 테스트 통과 (코드 수준) |
| ✅ PIPELINE | 파이프라인 체인 dry-run 테스트 통과 |
| 🔶 PARTIAL | 일부 구현/검증됨, 추가 작업 필요 |
| ❌ BLOCKED | 물리 인프라 또는 사용자 작업 필요 |

| # | 항목 | 상태 | 정직한 근거 |
|---|------|:----:|------------|
| 1 | SDI 가상화 (4노드→풀→2클러스터) | ✅ CODE | HCL 생성 ✅, cross-config 검증 ✅, 단일 노드 시나리오 17+ 테스트 ✅, `tofu apply` 미실행 (인프라 필요) |
| 2 | CF Tunnel GitOps 배포 | ✅ CODE | ApplicationSet syncWave:3, kustomization.yaml 존재 검증 통과 |
| 3 | CF Tunnel 완성도 | ❌ | 사용자 수동 작업 필요 (Cloudflare WebUI 터널 생성 + credentials 다운로드) |
| 4 | Rust CLI + FP 원칙 | ✅ CODE | 601 tests, `generate_*/run_*` 분리, 0 clippy warnings, thiserror+clap |
| 5 | 사용자 친절 가이드 | ✅ CODE | `--help`, `--dry-run`, 에러 UX, config-files 검증 |
| 6 | README 상세 내용 | ✅ CODE | 522줄 포괄 README, Architecture/Installation/CLI Reference/GitOps Pattern |
| 7 | Installation Guide E2E | ❌ | README Steps 0-8 작성됨, **실행하여 검증한 적 없음** |
| 8 | CLI 기능 전체 | ✅ CODE | 8개 명령어 구현, full pipeline chain test ✅ (`test_full_dryrun_pipeline_both_configs`), CIDR 겹침 30+ 테스트 ✅ |
| 9 | Baremetal 확장성 | ✅ CODE | `ClusterMode::Baremetal` + routing 테스트 통과 |
| 10 | 시크릿 템플릿화 | ✅ CODE | `.example` 6개, `.gitignore` 적용, Secret↔Helm values 정합성 검증 |
| 11 | 커널 파라미터 튜닝 | ✅ CODE | `scalex kernel-tune` 14개 sysctl, role별 분리 |
| 12 | 디렉토리 구조 | ✅ CODE | 구조 올바름, `config/` 실파일 `.gitignore` 추가 완료 (Sprint 44), `git rm --cached` 필요 |
| 13 | 멱등성 | 🔶 | 순수 함수 결정론 ✅, **인프라 멱등성 미검증**, `tofu apply` 재실행 테스트 없음 |
| 14 | 외부 kubectl 접근 | ❌ | Keycloak OIDC 미설정 → CF 경유 kubectl 불가. Tailscale만 가능 |
| 15 | NAT 접근 경로 | ✅ CODE | README에 4가지 접근 경로 문서화 (LAN/Tailscale/CF+OIDC/SOCKS5) |

**정직한 요약: 코드 수준 OK 12개 / 인프라+사용자 작업 필요 3개 (CL-3,7,14)**

---

## 실행 계획: 코드 수준에서 달성 가능한 개선

> 원칙: 최소 핵심 기능 단위 → TDD (RED→GREEN→REFACTOR) → 테스트 통과 → 커밋 → 다음 단계

### Sprint 44: DASHBOARD 정확성 교정 + Config Cleanup ✅

**목표**: DASHBOARD.md의 과도한 비판 수정 + config 파일 git 관리 정책 확립

- [x] **44-1**: 비판 #4 (단일 노드) 수정 — 17+ 테스트 이미 존재 확인
- [x] **44-2**: 비판 #5 (config gitignore) 수정 — `.gitignore`에 추가 완료
- [x] **44-3**: 비판 #6 (CIDR 겹침) 수정 — 30+ 테스트 이미 존재 확인
- [x] **44-4**: Checklist 재평가 테이블 CL-1, CL-8, CL-12 상태 업데이트
- [x] **44-5**: config 파일 git 추적 상태 확인 — 이미 `.gitignore`에 있고 추적되지 않음 ✅
- [x] **44-6**: `.example` 파일 확인 — `config/sdi-specs.yaml.example`, `config/k8s-clusters.yaml.example` 이미 존재 ✅

### Sprint 45: 파이프라인 체인 보강 + 누락 검증 ✅

**목표**: 기존 테스트에서 커버하지 않는 파이프라인 간극(gap) 식별 및 보강

- [x] **45-1**: facts JSON roundtrip 체인 테스트 — `parse_facts_output` → JSON serialize → deserialize → 필드 보존 ✅
- [x] **45-2**: SDI VM IP → cluster inventory `ansible_host` IP 일치 검증 ✅
- [x] **45-3**: bootstrap kubeconfig 경로 → helm args 연결 검증 ✅
- [ ] **45-4**: 전체 디렉토리 구조 검증 테스트 (불필요 파일 없음, .example 존재 확인)

### Sprint 46: SDI Sync Cascading Side-Effect 보강

**목표**: `sdi sync`에서 호스트 제거 시 cascading 영향 edge case 보강

- [ ] **46-1**: VM이 있는 호스트 제거 → conflict detection + 클러스터 영향도 보고
- [ ] **46-2**: 호스트 추가 시나리오 → 기존 풀에 영향 없음 검증
- [ ] **46-3**: sync diff + conflict → dry-run 출력 형식 검증

### Sprint 48+: 실환경 E2E (인프라 필요)

> 이하 Sprint는 실제 베어메탈 노드(playbox-0~3)에 대한 접근이 필요하다.

- [ ] **48-1**: `scalex facts --all` → 4노드 실 SSH → JSON 수집
- [ ] **48-2**: `scalex sdi init` (no flag) → 호스트 준비 (KVM, bridge)
- [ ] **48-3**: `scalex sdi init config/sdi-specs.yaml` → `tofu apply` → VM 5개 생성
- [ ] **48-4**: `scalex get sdi-pools` → 2개 풀(tower, sandbox) 확인
- [ ] **48-5**: 멱등성: `sdi init` 재실행 → 상태 불변
- [ ] **48-6**: `scalex sdi clean --hard --yes-i-really-want-to` → 초기화 → 재생성
- [ ] **48-7**: `scalex cluster init config/k8s-clusters.yaml` → tower + sandbox K8s
- [ ] **48-8**: `kubectl get nodes` 양쪽 클러스터 정상
- [ ] **48-9**: `scalex secrets apply` → K8s Secrets 생성
- [ ] **48-10**: `scalex bootstrap` → ArgoCD + 클러스터 등록 + spread.yaml
- [ ] **48-11**: 모든 Applications Synced/Healthy
- [ ] **48-12**: Tailscale IP → tower kubectl
- [ ] **48-13**: CF Tunnel → ArgoCD UI (`cd.jinwang.dev`)
- [ ] **48-14**: LAN 스위치 접근 → kubectl

### 사용자 수동 작업 (코드로 자동화 불가)

| 작업 | 이유 | 가이드 위치 |
|------|------|------------|
| Cloudflare WebUI에서 터널 생성 (`playbox-admin-static`) | Cloudflare 계정 인증 필요 | docs/ops-guide.md |
| tunnel credentials JSON 다운로드 → `credentials/cloudflare-tunnel.json` | 1회성 수동 작업 | docs/ops-guide.md |
| Keycloak Realm/Client 설정 | OIDC IdP 정책은 수동 결정 필요 | docs/ops-guide.md |
| 각 노드 SSH 접근 가능 상태 확보 | 물리적 네트워크 설정 | README Step 1.5 |

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
|    → projects/ (AppProjects)             |
|    → generators/{tower,sandbox}/         |
|    → common/ tower/ sandbox/             |
+-------------------------------------------+
```

### External Access Paths

```
[CF Tunnel + OIDC] (Keycloak 설정 완료 후에만 kubectl 가능)
  kubectl (OIDC token) → CF Edge → cloudflared → kube-apiserver
  ! client cert는 CF Tunnel 통과 불가 (TLS 종단)

[Tailscale] (Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) → Tailscale IP → kube-apiserver

[SOCKS5 Proxy] (LAN/Tailscale 내부 편의)
  kubectl --proxy-url socks5://tower-ip:1080 → kube-apiserver

[LAN] (모든 인증 방식)
  kubectl → LAN IP → kube-apiserver
  * 스위치 접근으로 LAN 진입 → 직접 kubectl 가능
```

---

## 2-Layer 템플릿 관리 체계

| Layer | 구성 파일 | 관리 범위 | 도구 |
|-------|----------|----------|------|
| **Infrastructure** | `config/sdi-specs.yaml` + `credentials/.baremetal-init.yaml` | SDI 가상화 + K8s 프로비저닝 | `scalex sdi init` → `cluster init` |
| **GitOps** | `config/k8s-clusters.yaml` + `gitops/` YAMLs | 멀티-클러스터 형상 관리 | `scalex bootstrap` → ArgoCD |

새 클러스터 추가:
1. `sdi-specs.yaml`에 pool 추가 (Infrastructure layer)
2. `k8s-clusters.yaml`에 cluster 정의 (Infrastructure layer)
3. `gitops/generators/`에 generator 추가 (GitOps layer)
4. 완료 — ArgoCD가 자동 배포
