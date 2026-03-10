# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 96 tests pass / clippy clean / fmt clean

**테스트 진행**: 62 → 96 (+34 tests, 6개 Gap 모두 해결)

| Sprint | 설명 | 상태 | 추가 테스트 |
|--------|------|------|-------------|
| 1 | `sdi sync` 순수 함수 추출 + 테스트 | **완료** | +7 (compute_sync_diff, detect_vm_conflicts) |
| 2 | `scalex get` 포매팅 테스트 | **완료** | +9 (facts_to_row, classify_config_status) |
| 3+4 | `.example` 디스크 읽기 + cross-config validation | **완료** | +9 (include_str! 파싱, pool mapping, cluster ID) |
| 5 | GitOps generator YAML 정합성 | **완료** | +9 (파싱, placeholder, URL 일관성, app list 동기화) |
| 6 | QA (test/clippy/fmt) | **완료** | - |

---

## Checklist 최종 검증 (6차 — 96 tests 기반)

| # | 질문 | 상태 | 근거 | 테스트 |
|---|------|------|------|--------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **완료** | `sdi init` host infra HCL + VM pool HCL + tofu apply | 8 tests |
| 2 | DataX kubespray 반영 | **완료** | 레거시 32줄 전체 → CommonConfig/generate_cluster_vars() 반영 | 6 tests |
| 3 | Keycloak 설정 | **가이드 완료** | Helm chart GitOps + docs/ops-guide.md Section 2 | - |
| 4 | CF tunnel GitOps | **완료** | gitops/tower/cloudflared-tunnel/ kustomization.yaml | - |
| 5 | CF tunnel 완성 | **가이드 완료** | WebUI 설정 필요. docs/ops-guide.md Section 1 | - |
| 6 | CLI 이름 scalex | **완료** | Cargo.toml name = "scalex" | - |
| 7 | Rust CLI + FP | **완료** | 96 tests, pure functions, clap derive, thiserror | 96 tests |
| 8 | CLI 기능 | **완료** | 아래 상세 — 모든 GAP 해결 | 아래 |
| 9 | 베어메탈 확장성 | **완료** | ClusterMode::Baremetal + generate_inventory_baremetal() | 3 tests |
| 10 | credentials 구조화 | **완료** | .example 템플릿 + .gitignore + include_str! 디스크 파싱 테스트 | 3 tests |
| 11 | 커널 튜닝 | **완료** | docs/ops-guide.md Section 3 + host_prepare.rs VFIO/bridge | - |
| 12 | 디렉토리 구조 | **완료** | scalex-cli/, gitops/{common,tower,sandbox}, credentials/, config/ | - |
| 12b | 멱등성 | **완료** | 순수 함수 기반 + test_generate_tofu_host_infra_idempotent | 1 test |
| 13 | CF tunnel 가이드 | **완료** | docs/ops-guide.md Section 1 (6 steps) | - |
| 14 | 외부 접근 | **완료** | CF Tunnel + Tailscale + LAN/스위치 가이드 | - |
| Q | Kyverno 위치 | **Common** | gitops/common/kyverno/ + 양쪽 generator에서 동일 app list 확인 | 1 test |

### Checklist #8 CLI 기능 상세 — GAP 해결 완료

| 기능 | 구현 | 테스트 | GAP |
|------|------|--------|-----|
| `scalex facts` | ✅ | 2 tests | - |
| `scalex sdi init` (no flag) | ✅ | 3 tests (host_infra HCL) | - |
| `scalex sdi init <spec>` | ✅ | 5 tests (VM HCL) | - |
| `scalex sdi clean --hard` | ✅ | 0 pure-fn tests | IO 의존 (순수 함수 부분 없음) |
| `scalex sdi sync` | ✅ | **7 tests** | ~~해결~~ compute_sync_diff + detect_vm_conflicts |
| `scalex cluster init` | ✅ | 18 tests | - |
| `scalex get baremetals` | ✅ | **3 tests** | ~~해결~~ facts_to_row 포매팅 |
| `scalex get sdi-pools` | ✅ | 0 tests | tabled crate 의존 (순수 함수 부분 없음) |
| `scalex get clusters` | ✅ | 0 tests | tabled crate 의존 (순수 함수 부분 없음) |
| `scalex get config-files` | ✅ | **6 tests** | ~~해결~~ classify_config_status 검증 |

---

## 해결된 Gap 목록

### ~~Gap 1~~: `sdi sync` 순수 함수 테스트 (Sprint 1) ✅
`core/sync.rs` 추가: `compute_sync_diff()` + `detect_vm_conflicts()` 순수 함수 추출, 7 tests.

### ~~Gap 2~~: `scalex get` 포매팅 테스트 (Sprint 2) ✅
`commands/get.rs` 테스트 추가: `facts_to_row()` 3 tests + `classify_config_status()` 6 tests.

### ~~Gap 3~~: `.example` 파일 디스크 읽기 (Sprint 3) ✅
`core/validation.rs`: `include_str!`로 실제 `.example` 파일 파싱 — baremetal-init, sdi-specs, k8s-clusters. 3 tests.

### ~~Gap 4~~: cross-config validation (Sprint 4) ✅
`core/validation.rs`: `validate_cluster_sdi_pool_mapping()` + `validate_unique_cluster_ids()` + cross-file consistency. 6 tests.

### ~~Gap 5~~: GitOps repo URL 일관성 (Sprint 5) ✅
`core/gitops.rs`: `test_all_generators_use_consistent_repo_url` — 4개 generator 파일의 repoURL 동일성 검증. 1 test.

### ~~Gap 6~~: Sandbox URL 자동 교체 검증 (Sprint 5) ✅
`core/gitops.rs`: 실제 generator YAML에서 placeholder 존재 확인 + 교체 동작 + YAML 구조 보존 검증. 8 tests.

---

## 신규 모듈 요약

| 파일 | 용도 | 테스트 |
|------|------|--------|
| `core/sync.rs` | SDI sync diff 계산 + VM 충돌 감지 순수 함수 | 7 |
| `core/validation.rs` | .example 파일 파싱 + cross-config 정합성 검증 | 9 |
| `core/gitops.rs` (기존 확장) | GitOps generator YAML 정합성 + URL 교체 검증 | +9 (기존 6 + 신규 9 = 15) |
| `commands/get.rs` (기존 확장) | facts_to_row + classify_config_status 순수 함수 | +9 |

---

## 사용자 수동 작업 (코드로 해결 불가)

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- `credentials/.baremetal-init.yaml` 작성 (실제 노드 IP/SSH 정보)
- `credentials/.env` 작성 (SSH 패스워드/키)
- `credentials/secrets.yaml` 작성 (Keycloak/ArgoCD/Cloudflare 시크릿)
- `config/sdi-specs.yaml` 작성 (VM 풀 정의)
- `config/k8s-clusters.yaml` 작성 (클러스터 정의)
