# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석

이전 DASHBOARD는 모든 WS(Work Stream) 8개를 "완료"로 표시했으나, **실제 코드와 대조 검증 결과 다수의 미해결 문제가 발견됨**.

### 1. 거짓 완료 (False Completion)

| 항목 | DASHBOARD 주장 | 실제 상태 | 근거 |
|------|---------------|----------|------|
| WS-1-3 `gitops/clusters/playbox/` 삭제 | "완료" | **미완료** | `gitops/clusters/tower/catalog.yaml` 여전히 존재 |
| WS-1-5 OLD 경로 참조 없음 | "완료" | **미완료** | `gitops/bootstrap/multi-cluster-spread.yaml` OLD 파일 잔존 |
| Checklist #2 DataX 반영 | "해결" | **부분 달성** | 7개 필드 추가했으나 `kubeconfig_localhost`, `nodelocaldns`, `ntp`, `node_prefix` 등 핵심 설정 누락 |
| Checklist #8 CLI 기능 | "완료" | **부분 달성** | `sdi init` (no flag)이 리소스 풀 관측 미구현, `sdi clean --hard`가 SSH 기반 정리 미구현 |

### 2. 구조적 문제 (Structural Issues)

| # | 문제 | 상세 |
|---|------|------|
| S1 | `sdi init` (no flag)이 "전체 리소스 풀로 관측"을 구현하지 않음 | 호스트 준비(KVM/bridge/VFIO)만 수행하고 종료. Checklist는 "통합하여 전체 리소스 풀로 관측되도록 구성"을 요구 |
| S2 | `sdi clean --hard`가 OpenTofu destroy만 수행 | SSH로 노드에 접속하여 K8s/KVM/패키지 정리를 하지 않음. Checklist는 "최소 요구 조건을 제외한 모든 프로그램 삭제" 요구 |
| S3 | `generate_cluster_vars()`에 DataX 핵심 설정 다수 누락 | `kubeconfig_localhost`, `kubectl_localhost`, `enable_nodelocaldns`, `ntp_enabled`, `kube_network_node_prefix` |
| S4 | `gitops/clusters/` OLD 디렉토리 잔존 | `catalog.yaml` 1개 파일 남아있음 |
| S5 | `gitops/bootstrap/multi-cluster-spread.yaml` 불필요 파일 잔존 | `spread.yaml`과 역할 중복 가능성 |

### 3. 근본 원인 분석

1. **검증 없는 완료 선언**: 실제 `grep`/`find` 기반 검증 없이 코드 변경만으로 완료 처리
2. **DataX 설정 부분 비교**: 7개 필드만 추출하고, kubespray 운영에 필수적인 나머지 설정들을 누락
3. **CLI 기능 정의 불일치**: Checklist #8의 상세 기능 정의와 실제 구현 간 gap 미확인
4. **테스트 커버리지 부족**: 37개 테스트가 pass하지만, 새로 발견된 gap 영역은 테스트 미존재

---

## Checklist 재검증 (2차)

| # | 질문 | 상태 | 근거 / 조치 필요 |
|---|------|------|-----------------|
| 1 | OpenTofu 전체 가상화 | **부분** | `sdi init <spec>` HCL 생성 OK, `sdi init` (no flag) 리소스 풀 관측 미구현 |
| 2 | DataX kubespray 반영 | **부분** | 7개 추가 OK, `kubeconfig_localhost`/`nodelocaldns`/`ntp`/`node_prefix` 누락 |
| 3 | Keycloak 설정 | **가이드** | Helm chart via GitOps OK, WebUI 설정 필요 (ops-guide.md) |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` Helm chart |
| 5 | CF tunnel 완성 | **가이드** | WebUI 설정 필요 (ops-guide.md Section 1) |
| 6 | CLI 이름 scalex | **완료** | `scalex-cli/` Rust CLI |
| 7 | Rust CLI | **완료** | 37 tests, clippy clean, FP style |
| 8 | CLI 기능 | **부분** | facts/get/sdi-init(spec)/cluster-init OK. sdi-init(no-flag)/sdi-clean/sdi-sync 미흡 |
| 9 | 베어메탈 확장성 | **완료** | `ClusterMode::Baremetal`, k3s 제거 |
| 10 | credentials 구조화 | **완료** | example 파일들 + .gitignore 적용 |
| 11 | 커널 튜닝 | **가이드** | ops-guide.md Section 3 |
| 12 | 디렉토리 구조 | **부분** | NEW 구조 OK, OLD `gitops/clusters/` 잔존 |
| 12b | 멱등성 | **미검증** | 개별 연산은 멱등적 설계이나 end-to-end 검증 없음 |
| 13 | CF tunnel 가이드 | **완료** | ops-guide.md |
| 14 | 외부 접근 | **완료** | CF tunnel + Tailscale + LAN 가이드 |
| Q | Kyverno 위치 | **Common** | `gitops/common/kyverno/` + 양쪽 generator 포함 (정책 일관성) |

---

## 실행 계획 (최소 핵심 기능 단위)

### Phase 1: OLD 구조 정리

> `gitops/clusters/` 잔존 파일 삭제, 불필요 bootstrap 파일 정리

- [ ] **1-1** `gitops/clusters/` 디렉토리 전체 삭제
- [ ] **1-2** `gitops/bootstrap/multi-cluster-spread.yaml` 검토 후 삭제 (spread.yaml과 중복시)
- [ ] **1-3** `grep -r "gitops/clusters"` 로 참조 없음 확인
- [ ] **1-4** YAML 유효성 검증

### Phase 2: DataX Kubespray 설정 보강 (TDD)

> `generate_cluster_vars()` 에 DataX 프로덕션 핵심 설정 추가

- [ ] **2-1** RED: `test_generate_cluster_vars_datax_production_settings` 작성
- [ ] **2-2** GREEN: `CommonConfig`에 필드 추가 및 `generate_cluster_vars()` 출력 추가
- [ ] **2-3** REFACTOR: 중복 제거 및 코드 정리
- [ ] **2-4** `k8s-clusters.yaml.example` 업데이트
- [ ] **2-5** `cargo test` 전체 통과 확인

### Phase 3: SDI Clean 강화 (TDD)

> `sdi clean --hard` 가 SSH로 노드에 접속하여 KVM/K8s/패키지를 정리

- [ ] **3-1** RED: `test_generate_node_cleanup_script` 작성
- [ ] **3-2** GREEN: `host_prepare.rs`에 `generate_node_cleanup_script()` 순수 함수 구현
- [ ] **3-3** `sdi.rs` `run_clean()`에 SSH 기반 정리 로직 통합
- [ ] **3-4** `cargo test` 전체 통과 확인

### Phase 4: SDI Init (no flag) 리소스 풀 관측 (TDD)

> `sdi init` (spec 없이 실행)시 모든 호스트의 리소스를 집계하여 통합 리소스 풀로 표시

- [ ] **4-1** RED: `test_generate_resource_pool_summary` 작성
- [ ] **4-2** GREEN: `core/resource_pool.rs` 모듈 생성
- [ ] **4-3** `sdi.rs` `run_init()`에 no-spec 경로 통합
- [ ] **4-4** `cargo test` 전체 통과 확인

### Phase 5: repoURL 일관성

- [ ] **5-1** `git remote -v` 로 실제 remote URL 확인
- [ ] **5-2** gitops YAML 내 repoURL과 비교, 불일치시 수정

### Phase 6: 문서 보강

- [ ] **6-1** Cloudflare Tunnel WebUI 설정 가이드 검토 및 보완
- [ ] **6-2** Keycloak Realm/Client 설정 가이드 검토 및 보완
- [ ] **6-3** LAN 내부 접근 (스위치 경유) 가이드 검토 및 보완

### Phase 7: 최종 검증 및 커밋

- [ ] **7-1** `cargo test` 전체 통과
- [ ] **7-2** `cargo clippy` clean
- [ ] **7-3** `cargo fmt --check` clean
- [ ] **7-4** gitops YAML 유효성 검증
- [ ] **7-5** OLD 경로 참조 0건 확인
- [ ] **7-6** 커밋 및 푸쉬

---

## 진행 상황 추적

| Phase | 설명 | 상태 | 비고 |
|-------|------|------|------|
| 1 | OLD 구조 정리 | **대기** | |
| 2 | DataX kubespray 보강 | **대기** | TDD |
| 3 | SDI clean 강화 | **대기** | TDD |
| 4 | SDI init 리소스 풀 | **대기** | TDD |
| 5 | repoURL 일관성 | **대기** | |
| 6 | 문서 보강 | **대기** | |
| 7 | 최종 검증 | **대기** | |

---

## 기존 완료 항목 (검증 완료)

- [x] credentials/ 구조 (`.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`)
- [x] config/ 스키마 (`baremetal.yaml.example`, `sdi-specs.yaml.example`, `k8s-clusters.yaml.example`)
- [x] scalex-cli Rust 프로젝트 (clap, serde, thiserror, FP style)
- [x] `scalex facts` (SSH → HW 정보 수집 → JSON 파싱)
- [x] `scalex get` (baremetals, sdi-pools, clusters, config-files)
- [x] `scalex sdi init <spec>` (OpenTofu HCL 생성 + 호스트 준비)
- [x] `scalex sdi sync` (기본 diff 기반 동기화)
- [x] `scalex cluster init` (inventory + vars 생성 + kubespray 실행 + kubeconfig 수집)
- [x] `ClusterMode::Baremetal` 지원 (SDI 없이 직접 kubespray)
- [x] gitops/common/ (cilium, cilium-resources, cert-manager, cluster-config, kyverno)
- [x] gitops/tower/ (argocd, keycloak, cloudflared-tunnel, socks5-proxy)
- [x] gitops/sandbox/ (local-path-provisioner, rbac, test-resources)
- [x] generators/ (tower-common, tower-apps, sandbox-common, sandbox-apps)
- [x] projects/ (tower-project, sandbox-project)
- [x] spread.yaml → NEW 경로
- [x] Kyverno → common/ + 양쪽 generator 포함
- [x] tofu/ → .legacy-tofu/ 이동, k3s 제거
- [x] playbox CLI deprecation notice
- [x] Sandbox URL 자동화 (core/gitops.rs)
- [x] tofu.rs gateway spec에서 전달

## 사용자 수동 작업

- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- `credentials/.baremetal-init.yaml` 및 `credentials/.env` 작성
- `config/sdi-specs.yaml` 작성
- `config/k8s-clusters.yaml` 작성
