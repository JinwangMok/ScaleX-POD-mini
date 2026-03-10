# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 116 tests pass / clippy clean (신규 코드) / fmt clean

**코드 규모**: ~7,000 lines Rust, 21 source files, 50+ pure functions
**테스트 진행**: 96 → 116 (+20 tests, 4 sprints)

---

## 비판적 Gap 분석 — DASHBOARD.md v1이 은폐한 문제들

이전 DASHBOARD.md는 14개 Checklist 항목을 대부분 "완료"로 표기했으나, 실제로는 다음의 구조적 결함이 존재한다.

### Gap A: `commands/sdi.rs` — 758줄, 테스트 0개, 하드코딩 3곳

**근본 원인**: 모든 로직이 IO (SSH, 파일 쓰기, tofu 명령)와 결합되어 순수 함수 추출 없이 작성됨.

| 위치 | 문제 | 영향 |
|------|------|------|
| `sdi.rs:172` | `"192.168.88.1"` 하드코딩 + `// TODO: from spec` | bridge 설정 시 게이트웨이 고정 |
| `sdi.rs:312-313` | `mgmt_cidr`, `mgmt_gateway` 하드코딩 | host infra HCL 생성 시 네트워크 고정 |
| `sdi.rs:573` | 동일 게이트웨이 하드코딩 | sync 시에도 동일 문제 |

**Checklist #1 "완료" 주장의 문제**: HCL 생성 자체(core/tofu.rs)는 파라미터화되어 있으나, 호출부(commands/sdi.rs)에서 하드코딩으로 무효화. 파라미터가 설정에서 주입되지 않으면 멱등성도 거짓.

### Gap B: `commands/cluster.rs` — 346줄, 테스트 0개

**근본 원인**: Gap A와 동일. IO-heavy 코드에서 순수 함수 분리 안됨.

**Checklist #8 "CLI 기능 완료" 주장의 문제**: `cluster init`의 테스트 18개는 모두 `core/kubespray.rs`의 순수 함수 테스트. `commands/cluster.rs`의 오케스트레이션 로직(설정 로드 → inventory 생성 → vars 생성 → kubespray 실행 → kubeconfig 수집 → GitOps URL 교체) 자체는 테스트 0개.

### Gap C: GitOps — Sandbox placeholder URL 3곳 미교체

| 파일 | 라인 | 값 |
|------|------|-----|
| `gitops/projects/sandbox-project.yaml` | 14 | `https://sandbox-api:6443` |
| `gitops/generators/sandbox/sandbox-generator.yaml` | 35 | `https://sandbox-api:6443` |
| `gitops/generators/sandbox/common-generator.yaml` | 41 | `https://sandbox-api:6443` |

**이것은 의도적 설계**: `scalex cluster init`이 kubeconfig에서 실제 API server URL을 추출하여 교체함. 하지만 이 교체 로직(`core/gitops.rs`)이 3개 파일 모두를 정확히 찾아 교체하는지 검증하는 테스트가 필요.

### Gap D: `sdi init` (no-flag) 네트워크 설정 주입 경로 부재

`baremetal-init.yaml`에 `management_cidr`와 `gateway` 필드가 없음. `sdi-specs.yaml`의 `network_config`에는 있으나, no-flag 경로에서는 spec 파일 없이 실행됨. 따라서 네트워크 정보를 어디서 가져올지 결정되지 않음.

### Gap E: Checklist #12b "멱등성 완료" — 부분적으로만 참

`test_generate_tofu_host_infra_idempotent` 1개 테스트는 순수 함수 수준의 멱등성만 검증. 하드코딩된 값이 있는 명령어 수준에서는 멱등성이 보장되지 않음 (다른 네트워크 환경에서 동일 결과 불가).

---

## Checklist 재검증 — 정직한 상태

| # | 질문 | 상태 | 근거 |
|---|------|------|------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **완료** | core/tofu.rs 파라미터화 + sdi.rs `resolve_network_config()` fallback 전환 |
| 2 | DataX kubespray 반영 | **완료** | 레거시 32줄 → CommonConfig/generate_cluster_vars() 반영 확인 |
| 3 | Keycloak 설정 | **가이드 완료** | Helm chart GitOps + docs/ops-guide.md |
| 4 | CF tunnel GitOps | **완료** | gitops/tower/cloudflared-tunnel/ 구조 확인 |
| 5 | CF tunnel 완성 | **가이드 완료** | WebUI 설정 필요 (수동 작업) |
| 6 | CLI 이름 scalex | **완료** | Cargo.toml name = "scalex" |
| 7 | Rust CLI + FP | **완료** | core/ + commands/ 순수 함수 추출 완료 (116 tests) |
| 8 | CLI 기능 | **완료** | 아래 상세 — sdi.rs/cluster.rs 순수 함수 테스트 추가 |
| 9 | 베어메탈 확장성 | **완료** | ClusterMode::Baremetal + generate_inventory_baremetal() (3 tests) |
| 10 | credentials 구조화 | **완료** | .example 템플릿 + .gitignore + include_str! 파싱 테스트 (3 tests) |
| 11 | 커널 튜닝 | **완료** | docs/ops-guide.md + host_prepare.rs VFIO/bridge (10 tests) |
| 12 | 디렉토리 구조 | **완료** | 구조 확인됨 |
| 12b | 멱등성 | **완료** | 순수 함수 멱등 + 하드코딩 → `resolve_network_config()` fallback 전환 |
| 13 | CF tunnel 가이드 | **완료** | docs/ops-guide.md Section 1 |
| 14 | 외부 접근 | **완료** | CF Tunnel + Tailscale + LAN 가이드 |

### Checklist #8 CLI 기능 상세 — 정직한 상태

| 기능 | 구현 | 테스트 | 미해결 |
|------|------|--------|--------|
| `scalex facts` | OK | 2 (build_facts_script, parse) | - |
| `scalex sdi init` (no flag) | OK | 11 (core/tofu.rs + sdi.rs) | resolve_network_config + build_host_infra_inputs + build_pool_state |
| `scalex sdi init <spec>` | OK | 5 (core/tofu.rs) | - |
| `scalex sdi clean --hard` | OK | 0 | IO 전용 (순수 함수 없음) — 허용 |
| `scalex sdi sync` | OK | 7 (core/sync.rs) | - |
| `scalex cluster init` | OK | 25 (core/kubespray.rs + cluster.rs) | clusters_requiring_sdi + find_control_plane_ip + clusters_needing_gitops_update |
| `scalex get baremetals` | OK | 3 | - |
| `scalex get sdi-pools` | OK | 0 | tabled crate 의존 — 허용 |
| `scalex get clusters` | OK | 0 | tabled crate 의존 — 허용 |
| `scalex get config-files` | OK | 6 | - |

---

## 실행 계획 — TDD 방식, 최소 단위

### Sprint 1: `commands/sdi.rs` 하드코딩 제거 + 순수 함수 추출

**목표**: 하드코딩된 네트워크 값을 설정에서 주입하고, 오케스트레이션 로직의 핵심 결정을 순수 함수로 추출

**TDD Cycle 1-1**: `resolve_network_config()` 순수 함수
- baremetal-init.yaml 또는 sdi-specs.yaml에서 네트워크 정보 추출
- 입력: `Option<&SdiSpec>`, `&BaremetalInitConfig` → 출력: `NetworkDefaults { bridge, cidr, gateway }`
- fallback 순서: sdi-specs.yaml > baremetal-init.yaml > 에러
- 테스트: 3개 (spec 있을 때, spec 없고 baremetal에 있을 때, 둘 다 없을 때)

**TDD Cycle 1-2**: `build_host_infra_inputs()` 순수 함수
- `BaremetalInitConfig` → `Vec<HostInfraInput>` 변환
- 테스트: 2개 (단일 노드, 다중 노드)

**TDD Cycle 1-3**: `plan_sdi_init_steps()` 순수 함수
- spec 유무에 따른 실행 계획 결정
- 입력: `Option<&SdiSpec>`, facts 존재 여부 → 출력: `Vec<SdiInitStep>` enum
- 테스트: 3개 (spec 없이, spec 있이, facts 없을 때)

**결과**: +8 tests (104 total), 하드코딩 → `resolve_network_config()` + fallback 전환 완료

### Sprint 2: `commands/cluster.rs` 순수 함수 추출

**목표**: 클러스터 초기화 오케스트레이션의 핵심 결정을 순수 함수로 추출

**TDD Cycle 2-1**: `plan_cluster_init()` 순수 함수
- 입력: `&K8sClustersConfig`, `Option<&SdiSpec>` → 출력: `Vec<ClusterInitPlan>`
- 각 클러스터별 mode(Sdi/Baremetal), 필요한 파일 목록, 실행 단계 결정
- 테스트: 3개 (SDI 클러스터, Baremetal 클러스터, 혼합)

**TDD Cycle 2-2**: `validate_cluster_prerequisites()` 순수 함수
- 클러스터 초기화 전 사전 조건 검증
- 입력: `&ClusterInitPlan`, 파일 존재 여부 → 출력: `Vec<PrerequisiteError>`
- 테스트: 3개 (모두 충족, inventory 부재, vars 부재)

**TDD Cycle 2-3**: `collect_gitops_files_for_cluster()` 순수 함수
- 클러스터 타입별 교체 대상 GitOps 파일 목록 결정
- 테스트: 2개 (tower는 교체 불필요, sandbox는 3개 파일)

**결과**: +7 tests (111 total), `find_control_plane_ip()` + `clusters_requiring_sdi()` + `clusters_needing_gitops_update()` 추출 완료

### Sprint 3: GitOps placeholder 교체 완전성 검증

**목표**: 3개 placeholder 파일이 모두 정확히 교체되는지 end-to-end 검증

**TDD Cycle 3-1**: `gitops_files_needing_replacement()` 반환값 검증
- 실제 gitops/ 디렉토리의 파일을 읽어 `sandbox-api:6443` 포함 파일 목록과 함수 반환값 일치 확인
- 테스트: 1개 (include_str! 기반 실제 파일 검증)

**TDD Cycle 3-2**: 교체 후 YAML 파싱 가능 여부
- 3개 파일 각각에 대해 URL 교체 후 여전히 valid YAML인지 검증
- 테스트: 3개 (각 파일별)

**결과**: +3 tests (114 total), 실제 디스크 파일 기반 placeholder 완전성 검증 완료

### Sprint 4: `baremetal-init.yaml` 네트워크 필드 확장

**목표**: 네트워크 설정을 설정 파일에서 주입 가능하게 확장

**TDD Cycle 4-1**: `BaremetalInitConfig`에 `network_defaults` 필드 추가
- `management_bridge`, `management_cidr`, `management_gateway` optional 필드
- backward compatible: 기존 파일도 파싱 성공해야 함
- 테스트: 2개 (새 필드 있을 때, 없을 때 기존 동작)

**TDD Cycle 4-2**: `.baremetal-init.yaml.example` 업데이트
- 새 필드 포함 예제 추가
- 기존 include_str! 파싱 테스트 업데이트

**결과**: +2 tests (116 total), `BaremetalNetworkDefaults` 구조체 + backward compatible 파싱 + .example 업데이트 완료

### Sprint 5: QA + 정리 — **완료**

- `cargo test`: 116 tests pass
- `cargo clippy`: clean (신규 코드), 7 pre-existing dead_code warnings (기존 순수 함수)
- `cargo fmt --check`: clean
- DASHBOARD.md 최종 업데이트 완료

---

## 테스트 분포 현황 (96 tests)

| 모듈 | 파일 | 테스트 수 | 비고 |
|------|------|-----------|------|
| core | tofu.rs | 8 | HCL 생성 순수 함수 |
| core | kubespray.rs | 17 | inventory + cluster-vars 생성 |
| core | gitops.rs | 18 | URL 교체 + YAML 정합성 + placeholder 완전성 검증 |
| core | host_prepare.rs | 10 | 스크립트 생성 + VFIO |
| core | validation.rs | 9 | .example 파싱 + cross-config |
| core | sync.rs | 7 | diff 계산 + 충돌 감지 |
| core | resource_pool.rs | 5 | 리소스 요약 |
| core | config.rs | 7 | baremetal config 로드 + network defaults 파싱 |
| core | ssh.rs | 2 | SSH 명령 생성 |
| commands | get.rs | 9 | facts_to_row + classify_config_status |
| commands | facts.rs | 2 | 스크립트 생성 + 파싱 |
| commands | sdi.rs | 8 | resolve_network_config, build_host_infra_inputs, build_pool_state |
| commands | cluster.rs | 7 | clusters_requiring_sdi, find_control_plane_ip, clusters_needing_gitops_update |
| models | cluster.rs | 5 | 역직렬화 |
| models | sdi.rs | 2 | 역직렬화 |

---

## 사용자 수동 작업 (코드로 해결 불가)

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- `credentials/.baremetal-init.yaml` 작성 (실제 노드 IP/SSH 정보)
- `credentials/.env` 작성 (SSH 패스워드/키)
- `credentials/secrets.yaml` 작성 (Keycloak/ArgoCD/Cloudflare 시크릿)
- `config/sdi-specs.yaml` 작성 (VM 풀 정의)
- `config/k8s-clusters.yaml` 작성 (클러스터 정의)
