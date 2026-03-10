# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석 (3차)

### 기존 DASHBOARD 문제점

이전 DASHBOARD(2차)는 진행 상황을 정확하게 반영했으나, **해결해야 할 실제 gap을 과소평가**했다.

1. **OIDC 설정 누락을 간과**: DataX 레거시의 핵심인 `kube_oidc_*` 설정이 `generate_cluster_vars()`에서 전혀 출력되지 않음. k8s-clusters.yaml.example에 OIDC 관련 설정이 있지만 파싱/생성 코드가 없음.
2. **`sdi init` (no-flag) 기능 정의 불명확**: "리소스 풀 관측"이라는 요구사항에 대해 구체적 구현 계획이 없었음. 호스트 준비만 수행하고 종료하는 현재 동작은 Checklist #1의 "통합하여 전체 리소스 풀로 관측되도록 구성" 요구사항을 충족하지 않음.
3. **`sdi clean --hard` 오판**: 이전 DASHBOARD는 미구현이라 기록했으나, 실제로는 `generate_node_cleanup_script()` + SSH 기반 정리가 이미 구현됨. **검증 없이 상태를 기록한 전형적 사례.**
4. **DataX의 `PodTolerationRestriction` 미반영**: DataX admission plugins에 `PodTolerationRestriction`이 있으나 현재 example config에는 `NodeRestriction`만 존재.
5. **OLD 구조 정리 오판**: `gitops/clusters/`와 `multi-cluster-spread.yaml`은 이미 삭제됨. Phase 1을 불필요하게 계획.

### 근본 원인

1. **코드를 직접 읽지 않고 이전 분석을 신뢰**: `sdi.rs:run_clean()`이 이미 SSH cleanup을 호출하고 있었으나, 이전 분석을 그대로 복사.
2. **OIDC가 멀티-클러스터의 핵심임을 놓침**: Keycloak + OIDC 연동은 kubespray 클러스터 변수에 직접 반영되어야 하는데, GitOps 배포만으로 해결된다고 오판.
3. **검증 기반 개발 미적용**: 코드 변경 후 `cargo test`만 돌리고, 기능 수준의 검증(생성된 YAML 내용 확인 등)을 하지 않음.

---

## Checklist 재검증 (3차 — 코드 직접 검증 기반)

| # | 질문 | 상태 | 근거 |
|---|------|------|------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **부분** | `sdi init <spec>` OK. `sdi init` (no-flag)은 호스트 준비만 하고 리소스 풀 집계/표시 없음 |
| 2 | DataX kubespray 반영 | **부분** | 핵심 설정 반영됨. **OIDC 설정 미반영** (`kube_oidc_*`), `PodTolerationRestriction` 미포함 |
| 3 | Keycloak 설정 | **가이드 완료** | Helm chart GitOps 배포 OK. WebUI 설정 가이드 `docs/ops-guide.md` Section 2 |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` kustomization.yaml |
| 5 | CF tunnel 완성 | **가이드 완료** | WebUI 설정 필요. `docs/ops-guide.md` Section 1 |
| 6 | CLI 이름 scalex | **완료** | `scalex-cli/Cargo.toml` name = "scalex" |
| 7 | Rust CLI + FP | **완료** | 40 tests pass, pure functions, clap derive, thiserror |
| 8 | CLI 기능 | **부분** | 아래 상세 |
| 9 | 베어메탈 확장성 | **완료** | `ClusterMode::Baremetal`, k3s 완전 제거 |
| 10 | credentials 구조화 | **완료** | `.example` 템플릿 + `.gitignore` + `secrets.yaml.example` |
| 11 | 커널 튜닝 | **완료** | `docs/ops-guide.md` Section 3 + `host_prepare.rs` VFIO/bridge |
| 12 | 디렉토리 구조 | **완료** | `scalex-cli/`, `gitops/{common,tower,sandbox}`, `credentials/`, `config/` |
| 12b | 멱등성 | **설계 완료, E2E 미검증** | 개별 함수는 멱등적 설계. 통합 검증 없음 |
| 13 | CF tunnel 가이드 | **완료** | `docs/ops-guide.md` Section 1 |
| 14 | 외부 접근 | **완료** | CF Tunnel + Tailscale + LAN 가이드 |
| Q | Kyverno 위치 | **Common** | `gitops/common/kyverno/` + 양쪽 generator |

### Checklist #8 CLI 기능 상세 검증

| 기능 | 상태 | 근거 |
|------|------|------|
| `scalex facts` | **완료** | SSH HW 수집 → JSON. `--all`, `--host`, `--dry-run` |
| `scalex sdi init` (no flag) | **부분** | 호스트 준비(KVM/bridge/VFIO)만 수행. **리소스 풀 집계/표시 미구현** |
| `scalex sdi init <spec>` | **완료** | HCL 생성 + tofu apply + pool state 저장 |
| `scalex sdi clean --hard` | **완료** | tofu destroy + SSH node cleanup (K8s/KVM/bridge 제거) |
| `scalex sdi sync` | **완료** | diff 기반 동기화 + VM 충돌 감지 + facts 수집 |
| `scalex cluster init` | **완료** | inventory + cluster-vars 생성 + kubespray 실행 + kubeconfig 수집 |
| `scalex get` | **완료** | baremetals, sdi-pools, clusters, config-files |

---

## 남은 Gap 목록 (우선순위순)

### Gap 1: OIDC 설정이 cluster-vars에 미반영 (Checklist #2)

**문제**: DataX의 `kube_oidc_auth`, `kube_oidc_client_id`, `kube_oidc_url` 등이 `generate_cluster_vars()`에서 출력되지 않음. 현재 `ClusterDef`에 OIDC 관련 필드가 없음.

**영향**: Kubespray로 클러스터를 배포해도 OIDC 인증이 활성화되지 않아 Keycloak 연동 불가.

**해결**:
- `ClusterDef`에 `OidcConfig` 추가
- `generate_cluster_vars()`에 OIDC 변수 출력
- `k8s-clusters.yaml.example` 업데이트
- `PodTolerationRestriction` admission plugin 추가

### Gap 2: `sdi init` (no-flag) 리소스 풀 요약 미구현 (Checklist #1, #8)

**문제**: Checklist는 "`sdi init` (no flag)이 모든 베어메탈을 가상화하고 전체 리소스 풀로 관측되도록 구성"을 요구. 현재는 호스트 준비만 수행.

**영향**: 사용자가 SDI 레이어의 전체 가용 리소스를 한눈에 파악 불가.

**해결**:
- 호스트 준비 후 facts 기반 리소스 집계 (CPU/MEM/GPU/Storage 합산)
- `_generated/sdi/resource-pool-summary.json` 생성
- 터미널에 테이블 형태로 표시

### Gap 3: 설정 파일 예제 정합성 (Checklist #8)

**문제**: `.baremetal-init.yaml.example`의 YAML 구조가 Checklist 스펙과 미세하게 다름 (`reachable_node_ip` vs `reachable_via` 필드명 등).

**영향**: 사용자가 Checklist 기반으로 설정 파일을 작성할 때 혼동.

**해결**: example 파일을 Checklist 스펙에 맞게 업데이트

---

## 실행 계획 (최소 핵심 기능 단위, TDD)

### Phase 1: OIDC 설정 생성 기능 (TDD) ← 최우선

> DataX OIDC 설정을 cluster-vars에 반영

- [x] **1-1** RED: `test_generate_cluster_vars_oidc_settings` 실패 테스트 작성
- [x] **1-2** GREEN: `OidcConfig` 모델 추가 + `generate_cluster_vars()` OIDC 출력
- [x] **1-3** REFACTOR: 중복 제거, 필드 정리
- [x] **1-4** `k8s-clusters.yaml.example`에 OIDC 설정 추가 (kubespray_extra_vars → oidc 필드로 이동)
- [x] **1-5** `PodTolerationRestriction` 이미 존재 확인
- [x] **1-6** `cargo test` 45→50 전체 통과 확인

### Phase 2: SDI Init 리소스 풀 요약 (TDD)

> `sdi init` (no flag) 시 리소스 풀 집계 표시

- [x] **2-1** RED: `test_generate_resource_pool_summary` 테스트 작성 (5개)
- [x] **2-2** GREEN: `core/resource_pool.rs` 모듈 + 순수 함수 구현
- [x] **2-3** `sdi.rs` run_init()에 통합 (no-flag 모드에서 리소스 풀 요약 표시 + JSON 저장)
- [x] **2-4** `cargo test` 50 전체 통과 확인

### Phase 3: 설정 파일 정합성 보강

> example 파일과 Checklist 스펙 일치

- [x] **3-1** `.baremetal-init.yaml.example` Checklist 스펙과 일치 확인 (sshKeyPathOfReachableNode 포함)
- [x] **3-2** `k8s-clusters.yaml.example` OIDC 필드 + PodTolerationRestriction 반영 확인
- [x] **3-3** config 파싱 테스트 추가 (`test_parse_oidc_config`, `test_parse_cluster_without_oidc`)

### Phase 4: 최종 검증 및 커밋

- [x] **4-1** `cargo test` 50개 전체 통과
- [x] **4-2** `cargo clippy` clean
- [x] **4-3** `cargo fmt --check` clean
- [ ] **4-4** 커밋 및 푸쉬

---

## 진행 상황 추적

| Phase | 설명 | 상태 | 비고 |
|-------|------|------|------|
| 1 | OIDC 설정 생성 | **완료** | TDD: RED→GREEN→REFACTOR, 5 new tests |
| 2 | SDI 리소스 풀 요약 | **완료** | TDD: 5 new tests, sdi.rs 통합 |
| 3 | 설정 파일 정합성 | **완료** | example 검증, 파싱 테스트 추가 |
| 4 | 최종 검증/커밋 | **진행중** | 50 tests pass, clippy+fmt clean |

---

## 이미 완료된 항목 (코드 직접 검증)

- [x] Rust CLI `scalex` (clap derive, serde, thiserror, FP style) — 40 tests pass
- [x] `scalex facts` (SSH → HW JSON, --all/--host/--dry-run)
- [x] `scalex get` (baremetals, sdi-pools, clusters, config-files)
- [x] `scalex sdi init <spec>` (HCL 생성 + host prep + tofu apply + pool state)
- [x] `scalex sdi clean --hard --yes-i-really-want-to` (tofu destroy + SSH node cleanup)
- [x] `scalex sdi sync` (diff + VM 충돌 감지 + facts 수집)
- [x] `scalex cluster init` (inventory + vars + kubespray + kubeconfig)
- [x] `ClusterMode::Baremetal` (SDI 없이 직접 kubespray)
- [x] gitops/common/ (cilium, cert-manager, kyverno, cluster-config, cilium-resources)
- [x] gitops/tower/ (argocd, keycloak, cloudflared-tunnel, socks5-proxy)
- [x] gitops/sandbox/ (local-path-provisioner, rbac, test-resources)
- [x] ApplicationSets + sync waves (generators/tower, generators/sandbox)
- [x] AppProjects (tower-project, sandbox-project)
- [x] spread.yaml bootstrap (tower-root + sandbox-root)
- [x] Kyverno → common/ (양쪽 generator 포함)
- [x] k3s 완전 제거, .legacy-tofu/ 이동
- [x] OLD 디렉토리 정리 완료 (gitops/clusters/ 삭제, multi-cluster-spread.yaml 삭제)
- [x] Sandbox URL 자동화 (core/gitops.rs)
- [x] credentials/ 구조 (.example 템플릿 + .gitignore)
- [x] docs/ops-guide.md (CF Tunnel + Keycloak + 커널 튜닝 + LAN 접근 가이드)
- [x] DataX kubespray 핵심 설정 반영 (kube_proxy_remove, kubeconfig_localhost, ntp, nodelocaldns 등)
- [x] Node cleanup script (K8s/KVM/bridge 제거, SSH 보존)

## 사용자 수동 작업 (코드로 해결 불가)

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- `credentials/.baremetal-init.yaml` 작성 (실제 노드 IP/SSH 정보)
- `credentials/.env` 작성 (SSH 패스워드/키)
- `credentials/secrets.yaml` 작성 (Keycloak/ArgoCD/Cloudflare 시크릿)
- `config/sdi-specs.yaml` 작성 (VM 풀 정의)
- `config/k8s-clusters.yaml` 작성 (클러스터 정의)
