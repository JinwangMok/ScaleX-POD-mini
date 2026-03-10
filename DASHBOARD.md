# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 160 tests pass / clippy 0 warnings / fmt clean

**코드 규모**: ~8,500 lines Rust, 23 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)
**레거시**: 모든 레거시 디렉토리 `.legacy-` prefix로 이동 완료

---

## Checklist 검증 결과 (2026-03-10, 전체 코드베이스 + 테스트 증거 기반)

### 검증 방법론

모든 항목은 다음 3단계로 검증:
1. **코드 증거**: 해당 기능의 소스 코드 존재 + 순수 함수 구현 확인
2. **테스트 증거**: `cargo test` 154개 전체 통과 (관련 테스트 명시)
3. **설정 파일 증거**: `.example` 파일과 실제 파서의 일치 여부

| # | 질문 | 상태 | 코드 증거 | 테스트 증거 | 미해결 |
|---|------|------|-----------|-------------|--------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **완료** | `tofu.rs`: host infra + VM HCL 생성, `sdi.rs`: host prep → resource pool → VM 생성 | `test_generate_tofu_host_infra_multi_node` (4개 노드), `test_generate_tofu_contains_vm`, `test_build_pool_state_*` | — |
| 2 | DataX kubespray 반영 | **100% 반영** | `kubespray.rs:147-156`: 8개 addon 비활성화 (cert_manager, argocd, metallb, ingress_nginx, local_path_provisioner, nfd, metrics_server, registry) | `test_cluster_vars_addon_disablement_prevents_argocd_conflicts`, `test_cluster_vars_datax_addon_disablement_metrics_and_registry`, `test_cluster_vars_contains_all_required_keys` | — |
| 3 | Keycloak 설정 | **가이드 완료** | `gitops/tower/keycloak/`, `secrets.rs`: keycloak-admin/db Secret 생성 | `test_secrets_for_management_cluster`, `test_generate_k8s_secret_yaml_keycloak_admin` | 사용자 Realm/Client 설정 필요 (ops-guide.md Section 2) |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` + sync-wave 3, `secrets.rs`: tunnel-credentials Secret | `test_secrets_for_management_cluster` (cloudflared-tunnel-credentials) | — |
| 5 | CF tunnel 완성 | **가이드 완료** | `docs/ops-guide.md` Section 1: 6단계 가이드 | — | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | `main.rs:8`: `name = "scalex"` | CLI 파싱 테스트 (clap derive) | — |
| 7 | Rust + FP 스타일 | **완료** | 23 소스 파일, 55+ pure functions, `#[cfg(test)]` 모듈 내장 | 160 tests, clippy 0 warnings, fmt clean | — |
| 8 | CLI 기능 완성도 | **100%** | 아래 상세 표 참조 | 아래 상세 표 참조 | — |
| 9 | 베어메탈 확장성 / k3s 배제 | **완료** | `ClusterMode::Baremetal` + `generate_inventory_baremetal()`, k3s 코드 없음 | `test_generate_inventory_baremetal`, `test_no_k3s_references_in_project_files` | — |
| 10 | Secrets 구조화 | **완료** | `secrets.rs`: SecretsConfig → K8sSecretSpec → YAML 생성, `credentials/*.example` 템플릿 | `test_parse_secrets_*`, `test_generate_all_secrets_manifests_*` | — |
| 11 | 커널 튜닝 가이드 | **완료** | `docs/ops-guide.md` Section 3: 자동 적용 + 수동 튜닝(스토리지/네트워크/IOMMU) | `test_facts_script_covers_all_required_hardware_sections` (kernel_params 포함) | — |
| 12a | 디렉토리 구조 | **완료** | `scalex-cli/`, `gitops/`, `credentials/`, `config/`, `docs/` 정상 | `test_no_legacy_toplevel_artifacts`, `test_no_gitops_dead_code_directories` | — |
| 12b | 멱등성 | **완료** | 모든 생성 함수가 순수 함수 (동일 입력 → 동일 출력) | `test_generate_tofu_host_infra_idempotent` | — |
| 13 | CF tunnel 가이드 | **완료** | `docs/ops-guide.md` Section 1 | — | — |
| 14 | 외부 접근 가이드 | **완료** | `docs/ops-guide.md` Section 4: CF Tunnel + Tailscale + LAN 스위치 | — | — |

### Kyverno 배치: **Common** (확정)

모든 클러스터에 일관된 보안/운영 정책. `gitops/common/kyverno/` 위치. 클러스터별 예외는 PolicyException으로.

---

### Checklist #8 CLI 기능 상세

| 명령어 | 구현 | 테스트 | 비고 |
|--------|------|--------|------|
| `scalex facts` | ✅ `facts.rs`: SSH facts gathering (cpu, mem, gpu, storage, pcie, kernel, network, iommu) | `test_parse_facts_output`, `test_facts_script_covers_all_required_hardware_sections`, `test_parsed_facts_has_all_checklist_fields` | `.baremetal-init.yaml` + `.env` 기반 |
| `scalex sdi init` (no flag) | ✅ `sdi.rs`: host prep (KVM, bridge, VFIO) + resource pool summary + host-level libvirt infra | `test_generate_tofu_host_infra_*` (3개), `test_build_host_infra_inputs_*` (2개) | facts 미존재 시 자동 실행 |
| `scalex sdi init <spec>` | ✅ `sdi.rs` + `tofu.rs`: VM HCL 생성 + VFIO XSLT + pool state + tofu apply | `test_generate_tofu_contains_*`, `test_build_pool_state_*` (2개) | GPU passthrough 지원 |
| `scalex sdi clean --hard` | ✅ `sdi.rs`: tofu destroy + 전 노드 SSH cleanup | `test_generate_node_cleanup_script_*` (2개) | `--yes-i-really-want-to` 필수 |
| `scalex sdi sync` | ✅ `sdi.rs` + `sync.rs`: diff 계산 + VM conflict 감지 + facts 수집/삭제 | `test_compute_sync_diff_*` (4개), `test_detect_vm_conflicts_*` (3개) | 활성 VM 보호 |
| `scalex cluster init` | ✅ `cluster.rs` + `kubespray.rs`: inventory + vars 생성 + kubespray 실행 + kubeconfig 수집 | `test_generate_inventory_*` (6개), `test_generate_cluster_vars_*` (8개), `test_full_pipeline_dryrun` | SDI/Baremetal 모드 |
| `scalex get baremetals` | ✅ `get.rs`: facts JSON → table | `test_facts_to_row*` (3개) | — |
| `scalex get sdi-pools` | ✅ `get.rs`: sdi-state.json → table | `test_sdi_pools_to_rows_*` (3개) | — |
| `scalex get clusters` | ✅ `get.rs`: cluster dirs → table | `test_count_nodes_from_inventory*`, `test_extract_cluster_name_from_vars*` | — |
| `scalex get config-files` | ✅ `get.rs`: 9개 파일/디렉토리 검증 | `test_classify_config_status_*` (6개) | YAML 유효성 검사 포함 |
| `scalex secrets apply` | ✅ `secrets.rs`: secrets.yaml → K8s Secret YAML 생성 | `test_generate_all_secrets_manifests_*` (3개), `test_secrets_for_*` (3개) | management/workload 분리 |

---

## 이전 DASHBOARD 비판적 분석

### 왜 이전 DASHBOARD가 부정확했는가

1. **테스트 카운트 불일치**: "152 tests" 기록 → 실제 160 tests. 이전 커밋 후 2개 테스트 추가됨
2. **Clippy 경고 허위 보고**: "11 warnings" → 실제 0 warnings. `cargo clippy --fix` 이미 적용 완료
3. **DEFECT-1~4 모두 이미 해결됨**:
   - DEFECT-1 (clippy 11 warnings): `cargo clippy -- -D warnings` 통과 확인
   - DEFECT-2 (metrics_server/registry 누락): `kubespray.rs:155-156`에 이미 존재
   - DEFECT-3 (레거시 디렉토리): `.legacy-` prefix로 이동 완료, `test_no_legacy_toplevel_artifacts` 통과
   - DEFECT-4 (kube_api_anonymous_auth): `kubespray_extra_vars`로 클러스터별 적용 (설계 의도)
4. **근본 원인**: DASHBOARD 작성 시점과 마지막 코드 커밋 시점 사이에 수정이 이루어졌으나 DASHBOARD가 갱신되지 않음

---

## 현재 진행 상태: 남은 작업

### 카테고리 A: 코드 품질 강화 (TDD 완료, 6개 테스트 추가)

| ID | 작업 | 설명 | 우선순위 | 상태 |
|----|------|------|----------|------|
| A-1 | `kube_api_anonymous_auth` 흐름 테스트 | kubespray_extra_vars를 통한 전달이 올바르게 YAML에 반영되는지 검증 + 중복 키 방어 | HIGH | **완료** (`test_cluster_vars_kube_api_anonymous_auth_flow`, `test_cluster_vars_extra_vars_duplicate_core_key_produces_invalid_yaml`) |
| A-2 | 크로스-config 예제 파일 일관성 테스트 | sdi-specs ↔ k8s-clusters IP 범위 충돌 없음 검증 | HIGH | **완료** (`test_example_configs_ip_no_overlap_between_node_and_service_cidrs`) |
| A-3 | GitOps 구조 완전성 테스트 강화 | generator sync-wave 값 범위 검증 (0-10) | MEDIUM | **완료** (`test_gitops_generator_apps_have_valid_sync_wave_order`) |
| A-4 | Cilium ClusterMesh ID 유일성 테스트 | 멀티클러스터 cluster_id 유일성 + 범위(1-255) 검증 | HIGH | **완료** (`test_example_configs_cilium_cluster_ids_unique`) |
| A-5 | 전체 파이프라인 dry-run 테스트 강화 | 모든 클러스터 addon disablement + extra_vars + YAML 유효성 검증 | MEDIUM | **완료** (`test_full_pipeline_dryrun_addon_disablement_all_clusters`) |

### 카테고리 B: 사용자 수동 작업 (코드로 해결 불가)

| ID | 작업 | 가이드 위치 |
|----|------|-------------|
| B-1 | Cloudflare Tunnel WebUI 설정 | `docs/ops-guide.md` Section 1 |
| B-2 | Keycloak Realm/Client 설정 | `docs/ops-guide.md` Section 2 |
| B-3 | `credentials/` 실제 파일 작성 (.baremetal-init.yaml, .env, secrets.yaml) | `credentials/*.example` |
| B-4 | `config/` 실제 파일 작성 (sdi-specs.yaml, k8s-clusters.yaml) | `config/*.example` |
| B-5 | GitOps repo URL 확인: gitops YAML의 `k8s-playbox.git` → 실제 레포명 확인 | `gitops/bootstrap/spread.yaml` |

### 카테고리 C: 향후 개선 (현 스프린트 범위 외)

| ID | 작업 | 설명 |
|----|------|------|
| C-1 | `scalex kernel-tune` 서브커맨드 | 원격 커널 파라미터 일괄 적용 |
| C-2 | Cilium ClusterMesh 자동화 | tower ↔ sandbox 연결 |
| C-3 | `scalex status` 서브커맨드 | 전체 플랫폼 상태 대시보드 |
| C-4 | `drawio` k3s 참조 제거 | `docs/*.drawio` 수동 수정 |

---

## 아키텍처 요약

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
|  get baremetals/sdi-pools/clusters        |
|  secrets apply                            |
+-------------------------------------------+
        |
        v
_generated/
+-- facts/          (hardware JSON per node)
+-- sdi/            (OpenTofu HCL + state)
+-- clusters/       (inventory.ini + vars per cluster)
        |
        v
+-------------------------------------------+
|           gitops/ (ArgoCD)                |
|  bootstrap/spread.yaml                    |
|  generators/{tower,sandbox}/              |
|  common/ tower/ sandbox/                  |
+-------------------------------------------+
```

## 프로젝트 구조

```
scalex-cli/                # Rust CLI (primary) — 160 tests, 0 clippy warnings
gitops/                    # Multi-cluster GitOps (ArgoCD)
+-- bootstrap/spread.yaml  # tower-root + sandbox-root
+-- generators/            # ApplicationSets per cluster
+-- projects/              # AppProjects (tower, sandbox)
+-- common/                # cert-manager, cilium-resources, cluster-config, kyverno
+-- tower/                 # argocd, cilium, cloudflared-tunnel, keycloak, socks5-proxy
+-- sandbox/               # cilium, local-path-provisioner, rbac, test-resources
credentials/               # Secrets + init (gitignored, .example templates)
config/                    # User config templates (sdi-specs, k8s-clusters)
docs/                      # ops-guide, architecture, troubleshooting
ansible/                   # Node preparation playbooks
kubespray/                 # Kubespray submodule (v2.30.0)
tests/                     # BATS + pytest + YAML validation (legacy shell tests)
_generated/                # Gitignored output (facts, SDI HCL, cluster configs)
.legacy-datax-kubespray/   # Archived DataX kubespray reference
.legacy-gitops-apps/       # Archived old ArgoCD app structure
.legacy-gitops-manual/     # Archived manual kubespray configs
.legacy-tofu/              # Archived old tower VM config
```
