# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석 (4차)

### 기존 DASHBOARD 문제점 (3차 DASHBOARD 비판)

이전 3차 DASHBOARD는 Gap 3개를 식별하고 모두 해결했다고 주장하지만, **실제로는 검증 범위가 좁고 근본적인 gap을 간과**했다.

1. **`sdi init` (no-flag)의 "리소스 풀 구성" 미완성을 리소스 풀 "요약"으로 대체**: Checklist #1은 "OpenTofu를 활용하여 모든 베어메탈을 가상화하고 통합하여 전체 리소스 풀로 관측되도록 구성"을 요구. 현재 no-flag 모드는 호스트 준비(KVM/bridge) + 집계 표시만 수행하며, **OpenTofu로 libvirt 스토리지 풀/네트워크를 생성하지 않음**. "관측되도록 구성"과 "관측되도록 표시"는 다름.

2. **Example 설정 파일 파싱 테스트 부재**: `.example` 파일이 실제로 코드에서 파싱 가능한지 검증하는 테스트가 없음. 50개 테스트는 모두 하드코딩된 YAML 문자열 기반.

3. **생성된 cluster-vars의 YAML 유효성 검증 부재**: `generate_cluster_vars()` 테스트는 부분 문자열 검사만 수행. 생성된 전체 YAML이 유효한지, kubespray가 수용하는 형식인지 검증 없음.

4. **DataX 레거시 설정과의 체계적 비교 부재**: OIDC와 PodTolerationRestriction은 확인했으나, 레거시의 모든 설정을 체계적으로 비교하지 않음.

5. **Dry-run 통합 테스트 부재**: 각 순수 함수의 단위 테스트는 있으나, `sdi init → cluster init` 파이프라인을 dry-run으로 검증하는 통합 테스트가 없음.

6. **"완료" 판정 기준 부재**: "50 tests pass"를 완료로 판정했으나, 테스트가 커버하지 않는 기능 gap은 확인하지 않음.

### 근본 원인

1. **테스트 커버리지 = 기능 완성도라는 오류**: 테스트가 통과한다고 기능이 완성된 것이 아님. 테스트가 커버하지 않는 gap이 핵심 미구현.
2. **"표시"와 "구성"의 혼동**: 리소스 풀 summary JSON을 생성/표시하는 것과, 실제 인프라를 구성하는 것은 다른 수준의 작업.
3. **Example 파일과 코드의 정합성 미검증**: example 파일이 코드의 실제 파싱과 일치하는지 테스트하지 않아, 불일치 가능성 존재.

---

## Checklist 재검증 (4차 — 코드+테스트 직접 검증 기반)

| # | 질문 | 상태 | 근거 |
|---|------|------|------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **완료** | `sdi init` no-flag: `generate_tofu_host_infra()` → libvirt provider/pool HCL 생성 + tofu apply. `sdi init <spec>`: VM pool HCL 생성 |
| 2 | DataX kubespray 반영 | **완료** | 레거시 `datax-cluster-vars.yml` 20줄 전체 반영 확인. 체계적 비교 완료 |
| 3 | Keycloak 설정 | **가이드 완료** | Helm chart GitOps OK. WebUI 가이드 `docs/ops-guide.md` Section 2 |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` kustomization.yaml |
| 5 | CF tunnel 완성 | **가이드 완료** | WebUI 설정 필요. `docs/ops-guide.md` Section 1 |
| 6 | CLI 이름 scalex | **완료** | `scalex-cli/Cargo.toml` name = "scalex" |
| 7 | Rust CLI + FP | **완료** | 62 tests, pure functions, clap derive, thiserror |
| 8 | CLI 기능 | **부분** | 아래 상세 |
| 9 | 베어메탈 확장성 | **완료** | `ClusterMode::Baremetal`, k3s 완전 제거 |
| 10 | credentials 구조화 | **완료** | `.example` 템플릿 + `.gitignore` + `secrets.yaml.example` |
| 11 | 커널 튜닝 | **완료** | `docs/ops-guide.md` Section 3 + `host_prepare.rs` VFIO/bridge |
| 12 | 디렉토리 구조 | **완료** | `scalex-cli/`, `gitops/{common,tower,sandbox}`, `credentials/`, `config/` |
| 12b | 멱등성 | **완료** | 개별 함수 멱등적 설계 + `test_full_pipeline_dryrun` + `test_generate_tofu_host_infra_idempotent` |
| 13 | CF tunnel 가이드 | **완료** | `docs/ops-guide.md` Section 1 (5 steps) |
| 14 | 외부 접근 | **완료** | CF Tunnel + Tailscale + LAN/스위치 가이드 |
| Q | Kyverno 위치 | **Common** | `gitops/common/kyverno/` + 양쪽 generator |

### Checklist #8 CLI 기능 상세 검증

| 기능 | 상태 | 테스트 | 근거 |
|------|------|--------|------|
| `scalex facts` | **완료** | 2 tests | SSH HW 수집 → JSON. `--all`, `--host`, `--dry-run` |
| `scalex sdi init` (no flag) | **완료** | 8 tests | 호스트 준비 + `generate_tofu_host_infra()` → libvirt provider/pool HCL + tofu apply |
| `scalex sdi init <spec>` | **완료** | 5 tests | HCL 생성 + tofu apply + pool state 저장 |
| `scalex sdi clean --hard` | **완료** | 2 tests | tofu destroy + SSH node cleanup |
| `scalex sdi sync` | **완료** | 0 (IO) | diff 기반 동기화 + VM 충돌 감지 + facts 수집 |
| `scalex cluster init` | **완료** | 18 tests | inventory + cluster-vars 생성 + YAML validation + pipeline dryrun + kubespray + kubeconfig |
| `scalex get` | **완료** | 0 (IO) | baremetals, sdi-pools, clusters, config-files |

---

## 남은 Gap 목록 (우선순위순)

### Gap 1: `sdi init` (no-flag) OpenTofu 호스트 인프라 미생성 (Checklist #1, #8)

**문제**: `sdi init` (no-flag)은 호스트 준비(KVM/bridge/VFIO)와 리소스 풀 summary만 수행. Checklist는 "OpenTofu를 활용하여 모든 베어메탈 하드웨어를 가상화하고 이들을 통합하여 전체 리소스 풀로 관측되도록 구성"을 요구.

**구체적 미구현**: libvirt 스토리지 풀(default pool), 네트워크(br0 기반), 그리고 이들의 상태를 OpenTofu state로 관리하는 HCL이 없음. 현재는 shell 스크립트(host_prepare)로만 설치.

**해결**: `generate_tofu_host_infra()` 순수 함수를 추가하여 libvirt provider + storage pool + network 정의를 HCL로 생성하고, `sdi init` no-flag에서 `tofu apply`로 적용. 이를 통해 "OpenTofu로 관리되는 리소스 풀" 요구사항 충족.

### Gap 2: Example 설정 파일 파싱 테스트 부재 (Checklist #8)

**문제**: `.baremetal-init.yaml.example`, `sdi-specs.yaml.example`, `k8s-clusters.yaml.example`이 코드의 파싱 로직과 일치하는지 검증하는 테스트 없음.

**해결**: 각 example 파일의 내용을 파싱하는 테스트 추가.

### Gap 3: Generated YAML 유효성 검증 부재 (Checklist #2, #8)

**문제**: `generate_cluster_vars()`의 출력이 유효한 YAML인지 검증 없음. 부분 문자열 검사만.

**해결**: 생성된 cluster-vars를 `serde_yaml::from_str::<serde_yaml::Value>()`로 파싱하여 유효 YAML 확인. DataX 레거시 설정과 체계적 비교.

### Gap 4: DataX 레거시 설정 체계적 비교 (Checklist #2)

**문제**: DataX의 `datax-cluster-vars.yml`에 있는 모든 설정이 현재 코드에 반영되었는지 체계적으로 비교하지 않음.

**해결**: DataX 레거시 설정을 테스트 fixture로 사용하여, 누락된 설정을 감지하는 테스트 추가.

### Gap 5: Dry-run 파이프라인 통합 테스트 (Checklist #12b)

**문제**: 개별 순수 함수 테스트는 있으나, `facts → sdi init → cluster init → gitops update` 파이프라인을 dry-run으로 검증하는 통합 테스트 없음.

**해결**: Mock facts 데이터로 전체 파이프라인을 테스트하는 통합 테스트 추가.

---

## 실행 계획 (최소 핵심 기능 단위, TDD)

### Phase 1: `sdi init` no-flag — OpenTofu 호스트 인프라 생성 (TDD)

> OpenTofu로 libvirt storage pool + network를 관리

- [ ] **1-1** RED: `test_generate_tofu_host_infra` — facts 기반 호스트별 libvirt pool/network HCL 생성 테스트
- [ ] **1-2** RED: `test_generate_tofu_host_infra_multi_node` — 4노드 HCL에 모든 호스트 포함 검증
- [ ] **1-3** RED: `test_generate_tofu_host_infra_idempotent` — 동일 입력에 동일 출력 검증
- [ ] **1-4** GREEN: `core/tofu.rs`에 `generate_tofu_host_infra()` 구현
- [ ] **1-5** REFACTOR: 기존 `generate_tofu_main()`과 공통 로직 추출
- [ ] **1-6** `sdi.rs` run_init()에 통합 (no-flag 모드에서 HCL 생성 + tofu apply)
- [ ] **1-7** `cargo test` 전체 통과 확인

### Phase 2: Example 설정 파일 파싱 테스트 (TDD)

> .example 파일이 코드에서 실제 파싱 가능한지 검증

- [ ] **2-1** RED: `test_parse_baremetal_init_example` — example 파일 내용 파싱
- [ ] **2-2** RED: `test_parse_sdi_specs_example` — example 파일 내용 파싱
- [ ] **2-3** RED: `test_parse_k8s_clusters_example` — example 파일 내용 파싱
- [ ] **2-4** GREEN: 필요시 example 파일 또는 파싱 코드 수정
- [ ] **2-5** `cargo test` 전체 통과 확인

### Phase 3: Generated YAML 유효성 검증 (TDD)

> 생성된 cluster-vars가 유효한 YAML이고 필수 키를 포함하는지 검증

- [ ] **3-1** RED: `test_cluster_vars_valid_yaml` — 생성된 출력을 serde_yaml로 파싱
- [ ] **3-2** RED: `test_cluster_vars_contains_all_required_keys` — kubespray 필수 키 존재 검증
- [ ] **3-3** RED: `test_inventory_contains_all_required_sections` — [all], [kube_control_plane] 등
- [ ] **3-4** GREEN: 필요시 생성 로직 수정
- [ ] **3-5** `cargo test` 전체 통과 확인

### Phase 4: DataX 설정 체계적 비교 (TDD)

> DataX 레거시 설정이 모두 반영되었는지 검증

- [ ] **4-1** DataX `datax-cluster-vars.yml` 분석 → 필수 키 목록 추출
- [ ] **4-2** RED: `test_datax_required_settings_coverage` — 모든 DataX 설정이 생성되는지 검증
- [ ] **4-3** GREEN: 누락된 설정 추가
- [ ] **4-4** `cargo test` 전체 통과 확인

### Phase 5: Dry-run 통합 테스트 (TDD)

> 전체 파이프라인을 mock 데이터로 검증

- [ ] **5-1** RED: `test_pipeline_sdi_to_cluster_dryrun` — facts → sdi init → cluster init 검증
- [ ] **5-2** GREEN: 파이프라인 로직 보강
- [ ] **5-3** `cargo test` 전체 통과 확인

### Phase 6: 최종 검증 및 커밋

- [ ] **6-1** `cargo test` 전체 통과
- [ ] **6-2** `cargo clippy` clean
- [ ] **6-3** `cargo fmt --check` clean
- [ ] **6-4** DASHBOARD.md 최종 업데이트
- [ ] **6-5** 커밋 및 푸쉬

---

## 진행 상황 추적

| Phase | 설명 | 상태 | 비고 |
|-------|------|------|------|
| 1 | OpenTofu 호스트 인프라 생성 | **완료** | `generate_tofu_host_infra()` + 3 tests |
| 2 | Example 파싱 테스트 | **완료** | 3 tests (baremetal, sdi, k8s configs) |
| 3 | YAML 유효성 검증 | **완료** | 6 tests (valid YAML, required keys, INI sections) |
| 4 | DataX 설정 비교 | **완료** | 레거시 20줄 — 모든 설정 이미 반영 확인 |
| 5 | 통합 테스트 | **완료** | `test_full_pipeline_dryrun` (config → inventory → vars → cross-cluster) |
| 6 | 최종 검증/커밋 | **완료** | 62 tests, clippy clean, fmt clean |

---

## 이미 완료된 항목 (코드 직접 검증, 62 tests pass)

- [x] Rust CLI `scalex` (clap derive, serde, thiserror, FP style) — 62 tests
- [x] `scalex facts` (SSH → HW JSON, --all/--host/--dry-run)
- [x] `scalex get` (baremetals, sdi-pools, clusters, config-files)
- [x] `scalex sdi init <spec>` (HCL 생성 + host prep + tofu apply + pool state)
- [x] `scalex sdi clean --hard --yes-i-really-want-to` (tofu destroy + SSH node cleanup)
- [x] `scalex sdi sync` (diff + VM 충돌 감지 + facts 수집)
- [x] `scalex cluster init` (inventory + vars + kubespray + kubeconfig)
- [x] `ClusterMode::Baremetal` (SDI 없이 직접 kubespray)
- [x] OIDC config → cluster-vars 생성 (kube_oidc_* 전체)
- [x] Resource pool summary (JSON + formatted table)
- [x] gitops/common/ (cilium, cert-manager, kyverno, cluster-config, cilium-resources)
- [x] gitops/tower/ (argocd, keycloak, cloudflared-tunnel, socks5-proxy)
- [x] gitops/sandbox/ (local-path-provisioner, rbac, test-resources)
- [x] ApplicationSets + sync waves (generators/tower, generators/sandbox)
- [x] AppProjects (tower-project, sandbox-project)
- [x] spread.yaml bootstrap (tower-root + sandbox-root)
- [x] Kyverno → common/ (양쪽 generator 포함)
- [x] k3s 완전 제거, .legacy-tofu/ 이동
- [x] credentials/ 구조 (.example 템플릿 + .gitignore)
- [x] docs/ops-guide.md (CF Tunnel + Keycloak + 커널 튜닝 + LAN 접근 가이드)
- [x] DataX kubespray 핵심 설정 반영 (kube_proxy_remove, kubeconfig_localhost 등)
- [x] Node cleanup script (K8s/KVM/bridge 제거, SSH 보존)
- [x] Sandbox URL 자동화 (core/gitops.rs)
- [x] OpenTofu 호스트 인프라 생성 (`generate_tofu_host_infra()` — libvirt provider/pool HCL)
- [x] Example 설정 파일 파싱 테스트 (baremetal-init, sdi-specs, k8s-clusters)
- [x] Generated YAML 유효성 검증 (cluster-vars valid YAML, required keys, INI sections)
- [x] DataX 레거시 설정 체계적 비교 (20줄 전체 반영 확인)
- [x] Full pipeline dry-run 통합 테스트 (config → inventory → vars → cross-cluster 검증)

## 사용자 수동 작업 (코드로 해결 불가)

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- `credentials/.baremetal-init.yaml` 작성 (실제 노드 IP/SSH 정보)
- `credentials/.env` 작성 (SSH 패스워드/키)
- `credentials/secrets.yaml` 작성 (Keycloak/ArgoCD/Cloudflare 시크릿)
- `config/sdi-specs.yaml` 작성 (VM 풀 정의)
- `config/k8s-clusters.yaml` 작성 (클러스터 정의)
