# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) -> SDI (OpenTofu) -> Node Pools -> Cluster (Kubespray) -> GitOps (ArgoCD)

---

## 현재 상태: 149 tests pass / clippy clean / fmt clean

**코드 규모**: ~8,100 lines Rust, 22 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)

---

## 이전 DASHBOARD.md 비판적 분석

이전 DASHBOARD.md는 여러 항목을 "CRITICAL" 또는 "누락"으로 표기했으나 실제 코드와 불일치:

### 오진 1: CRITICAL-1 "Cilium k8sServiceHost 하드코딩" -- 이미 해결됨
- `gitops/common/cilium/values.yaml`에서 `k8sServiceHost` 이미 제거 ("shared reference only")
- `gitops/tower/cilium/values.yaml` -> `192.168.88.100` (tower CP)
- `gitops/sandbox/cilium/values.yaml` -> `PLACEHOLDER_SANDBOX_CP_IP` (cluster init 시 교체)
- common-generator에서 cilium 제외됨 (cilium-resources만 포함)
- 그러나 `common/cilium/`에 kustomization.yaml이 남아 **dead code** 혼란 유발

### 오진 2: "node_feature_discovery_enabled: false 누락" -- 이미 구현됨
- `kubespray.rs:154` -> `node_feature_discovery_enabled: false` 포함
- 테스트 존재: `kubespray.rs:755`, `kubespray.rs:781`

### 오진 3: "get sdi-pools 0개 테스트" -- 3개 테스트 존재
- `test_sdi_pools_to_rows_basic`, `_empty`, `_multi_pool_multi_node`

### 정당한 지적: k3s 잔존 참조 -- 여전히 미해결

### 놓친 문제들:
- `docs/SETUP-GUIDE.md`, `docs/TROUBLESHOOTING.md`가 `./playbox`와 `values.yaml` 참조
- `common/cilium/` dead code (generator가 참조하지 않는 kustomization.yaml)

---

## Checklist 재검증 (2026-03-10)

| # | 질문 | 상태 | 근거 | 미해결 |
|---|------|------|------|--------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **코드 완료** | tofu.rs: HCL 생성, sdi.rs: pool state | sdi init (no flag) 리소스 풀 관측 검증 필요 |
| 2 | DataX kubespray 반영 | **완료** | kubespray.rs: 모든 addon 비활성화 반영 | -- |
| 3 | Keycloak 설정 | **가이드 완료** | gitops/tower/keycloak/ + docs/ops-guide.md | 사용자 Realm/Client 설정 필요 |
| 4 | CF tunnel GitOps 배포 | **완료** | gitops/tower/cloudflared-tunnel/ | -- |
| 5 | CF tunnel 완성 | **가이드 완료** | docs/ops-guide.md Section 1 | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | main.rs: `name = "scalex"` | -- |
| 7 | Rust + FP 스타일 | **완료** | 144 tests, 55+ pure functions | -- |
| 8 | CLI 기능 완성도 | **95%** | facts/sdi/cluster/get/secrets 구현 | facts 커버리지 상세 검증 |
| 9 | 베어메탈 확장성 / k3s 배제 | **미완성** | mode=baremetal 지원 | **k3s 참조 잔존** |
| 10 | Secrets 구조화 | **완료** | secrets.rs + secrets apply CLI | -- |
| 11 | 커널 튜닝 가이드 | **완료** | docs/ops-guide.md Section 3 | -- |
| 12a | 디렉토리 구조 | **완료** | Kyverno -> common/ | -- |
| 12b | 멱등성 | **완료** | 순수 함수 기반 | -- |
| 13 | CF tunnel 가이드 | **완료** | docs/ops-guide.md Section 1 | -- |
| 14 | 외부 접근 가이드 | **완료** | docs/ops-guide.md Section 4 | -- |

---

## 미해결 항목 상세

### DEFECT-1: k3s 잔존 참조

| 파일 | 참조 | 처리 |
|------|------|------|
| `values.yaml` | `tower.k3s.version` | 삭제 (scalex는 config/ 사용) |
| `lib/cluster.sh` | k3s 설치/제거 함수 | 삭제 (전체 lib/ 레거시) |
| `tests/fixtures/values-full.yaml` | `k3s:` 섹션 | 삭제 |
| `tests/fixtures/values-minimal.yaml` | `k3s:` 섹션 | 삭제 |

### DEFECT-2: docs 레거시 참조

| 파일 | 문제 | 처리 |
|------|------|------|
| `docs/SETUP-GUIDE.md` | `./playbox`, `values.yaml` 참조 | scalex 기반 재작성 |
| `docs/TROUBLESHOOTING.md` | `./playbox destroy-all` 참조 | scalex 기반 재작성 |

### DEFECT-3: common/cilium/ dead code

generator가 참조하지 않는 `gitops/common/cilium/` (kustomization.yaml + values.yaml) 존재.
실제 배포는 tower/cilium/, sandbox/cilium/에서 수행. 혼란 방지를 위해 삭제.

---

## 실행 계획 -- TDD, 최소 핵심 단위

### Unit 1: k3s 잔존 참조 정리 + 검증 테스트
> Checklist #9 해결

**1-1** RED: k3s-free 검증 테스트 (validation.rs)
**1-2** GREEN: values.yaml, fixtures, lib/ k3s 참조 제거
**1-3** REFACTOR: 테스트 통과 확인

### Unit 2: common/cilium/ dead code 정리 + GitOps 정합성 테스트
> dead code 제거, 구조 정합성 검증

**2-1** RED: generator가 참조하는 모든 경로가 존재하는지, dead code 없는지 테스트
**2-2** GREEN: common/cilium/ 삭제
**2-3** REFACTOR: gitops 테스트 업데이트

### Unit 3: docs 레거시 참조 업데이트
**3-1**: SETUP-GUIDE.md -> scalex 기반
**3-2**: TROUBLESHOOTING.md -> scalex 기반

### Unit 4: facts 커버리지 검증
**4-1** RED: facts 스크립트에 필수 항목(커널, cpu, mem, gpu, storage, pcie) 포함 테스트
**4-2** GREEN: 누락 항목 추가

---

## 사용자 수동 작업 (코드로 해결 불가)

1. **Cloudflare Tunnel WebUI 설정** -> `docs/ops-guide.md` Section 1
2. **Keycloak Realm/Client 설정** -> `docs/ops-guide.md` Section 2
3. **credentials/ 파일 작성** (.baremetal-init.yaml, .env, secrets.yaml)
4. **config/ 파일 작성** (sdi-specs.yaml, k8s-clusters.yaml)

---

## Kyverno 배치 결정: **Common** (확정)

모든 클러스터에 일관된 보안/운영 정책 적용. 클러스터별 예외는 PolicyException으로 관리.
