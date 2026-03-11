# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical -> SDI (OpenTofu) -> Node Pools -> Cluster (Kubespray) -> GitOps (ArgoCD)

---

## Critical Gap Analysis (Sprint 40 재검증)

### 이전 DASHBOARD의 문제점

이전 작업자는 15개 Checklist 중 12개를 "✅ VERIFIED"로 표시했으나, 이는 **심각한 오류**이다.

**근본 원인 1: "코드 수준 검증"과 "운용 수준 검증"의 혼동**

554개 테스트는 **순수 함수의 출력 정확성**만 검증한다. 예를 들어:
- `generate_tofu_main()` → HCL 문자열 생성 검증 ✅ → 하지만 `tofu apply`로 실제 VM이 생성되는지? ❌
- `generate_inventory()` → Kubespray inventory.ini 생성 검증 ✅ → 하지만 `ansible-playbook`으로 클러스터가 구축되는지? ❌
- `generate_argocd_helm_install_args()` → Helm 명령어 문자열 검증 ✅ → 하지만 ArgoCD가 실제로 Sync되는지? ❌

이는 "컴파일러가 통과했으므로 프로그램이 올바르다"고 주장하는 것과 같다.

**근본 원인 2: 합성 데이터로 검증한 것을 실제 검증으로 포장**

`_generated/facts/*.json` 파일은 **손으로 작성한 합성 데이터**이다:
- 타임스탬프가 모두 `2026-03-11T12:00:00Z` (미래 날짜)
- 데이터가 비현실적으로 완벽함 (실제 `lspci` 출력은 이렇게 깔끔하지 않음)
- 실제 SSH 접속으로 수집된 적 없음

**근본 원인 3: 멱등성(CL-13) 검증 오류**

"순수 함수를 2회 호출하면 동일 출력" → 이것은 **함수 결정론(determinism)**이지 **인프라 멱등성(idempotency)**이 아니다.
진정한 멱등성 검증은: `sdi init` 2회 실행 후 OpenTofu state가 변하지 않고 VM 상태가 동일한지 확인하는 것이다.

---

## Checklist 재평가 (정직한 현황)

### 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ CODE-OK | 코드 구현 완료, 순수 함수 테스트 통과 |
| 🔶 UNTESTED | 코드는 있지만 실제 환경에서 검증되지 않음 |
| ❌ NOT-VERIFIED | 검증 불가능 또는 미완료 |
| ⬜ NEEDS-INFRA | 물리 인프라 필요 |

| # | 항목 | 코드 | 운용 | 비고 |
|---|------|:----:|:----:|------|
| 1 | SDI 가상화 (4노드→풀→2클러스터) | ✅ | ⬜ | HCL 생성기 OK, `tofu apply` 미실행 |
| 2 | CF Tunnel GitOps 배포 | ✅ | ⬜ | kustomization.yaml 존재, ArgoCD 미배포 |
| 3 | CF Tunnel 완성도 | 🔶 | ❌ | 사용자 수동 작업 미완료, 통합 미검증 |
| 4 | Rust CLI + FP 원칙 | ✅ | ✅ | 554 tests, 순수함수/IO 분리 확인 |
| 5 | 사용자 친절 가이드 | ✅ | 🔶 | --help/--dry-run 있으나 실사용 피드백 없음 |
| 6 | README 상세 내용 | ✅ | 🔶 | 포괄적이나 Installation Guide E2E 미검증 |
| 7 | Installation Guide E2E | 🔶 | ❌ | Dry-run만 OK, 실제 실행 미검증 **(핵심 미달성)** |
| 8 | CLI 기능 전체 | ✅ | ⬜ | 11개 명령어 구현, 실행 미검증 |
| 9 | Baremetal 확장성 | ✅ | 🔶 | `cluster_mode: "baremetal"` 경로 존재, 미검증 |
| 10 | 시크릿 템플릿화 | ✅ | 🔶 | `.example` 4개, `.gitignore` OK |
| 11 | 커널 파라미터 튜닝 | ✅ | 🔶 | `scalex kernel-tune` 구현, 실적용 미검증 |
| 12 | 디렉토리 구조 | ✅ | ✅ | 설계 명세와 일치 확인 |
| 13 | 멱등성 | 🔶 | ❌ | 함수 결정론만 검증, 인프라 멱등성 미검증 |
| 14 | 외부 kubectl 접근 | 🔶 | ❌ | Keycloak 미설정, CF Tunnel 미검증 |
| 15 | NAT 접근 경로 | ✅ | 🔶 | 문서화 완료, 실제 접근 미검증 |

**정직한 요약: 코드 구현은 대부분 완료. 하지만 실제 인프라에서 단 한 번도 실행된 적 없음.**

---

## 현재 코드 품질 현황

### 강점
- **Rust CLI 아키텍처**: 순수 함수(`generate_*`) / IO 함수(`run_*`) 분리가 일관적
- **테스트 커버리지**: 579개 단위 테스트 (+25 from Sprint 41), `cargo clippy` 0 warnings, `cargo fmt` OK
- **Config 스키마**: `sdi-specs.yaml`, `k8s-clusters.yaml`, `.baremetal-init.yaml` 3개 파일이 `serde`로 타입 안전하게 파싱
- **GitOps 구조**: ApplicationSet 기반 멀티-클러스터 패턴 구현
- **Cross-config 검증**: pool mapping, CIDR overlap, cluster ID uniqueness, resource over-allocation, quorum loss 등
- **Workflow 의존성 검증**: 명령어 실행 순서별 필수 artifact 검증 + 해결 안내 메시지

### Sprint 41에서 해결된 사항
1. ~~합성 facts 파일 정리~~: 실제 합성 파일 없음 확인 (인라인 테스트 픽스처만 존재)
2. ~~`scalex sdi sync` 부작용 감지 강화~~: etcd quorum loss 감지 추가 (6 tests)
3. ~~baremetal-init.yaml 스키마 검증 강화~~: 자기참조, 순환참조, IP 형식, 충돌 설정 감지 (5 tests)
4. ~~GitOps manifest 무결성 테스트~~: kustomization 참조 + YAML 파싱 검증 (2 tests)
5. ~~에러 UX 개선~~: workflow 의존성 검증 + 포맷팅된 에러 메시지 (8 tests)
6. ~~리소스 과다할당 감지~~: SDI spec vs facts CPU/MEM 초과 검증 (4 tests)

### 남은 개선 사항
1. **README Installation Guide**: 실행 순서의 논리적 일관성 재검증 필요
2. **실환경 E2E 검증**: 물리 인프라에서 전체 파이프라인 실행

---

## 실행 계획

### Phase A: 코드 품질 강화 (인프라 불필요 — 즉시 실행 가능)

#### A-1. 합성 facts 파일 정리 ✅ (Sprint 41)
- [x] 확인 결과 `_generated/facts/` 디렉토리 없음 — 합성 파일 미존재 (인라인 테스트 픽스처만 존재)

#### A-2. baremetal-init.yaml 스키마 검증 강화 ✅ (Sprint 41, +5 tests)
- [x] `reachable_via` 자기참조 감지 (`validate_baremetal_config`)
- [x] `reachable_via` 순환참조 체인 감지
- [x] IP 형식 검증 (`is_valid_ip()` 순수 함수)
- [x] `reachable_node_ip` 형식 검증
- [x] `direct_reachable=true` + `reachable_via` 동시 설정 충돌 감지

#### A-3. 2-layer 정합성 강화 ✅ (Sprint 41, +4 tests)
- [x] `validate_sdi_resource_allocation()`: SDI pool CPU/MEM 총합 vs facts 물리 자원 초과 감지
- [x] CPU 초과할당 감지 테스트
- [x] MEM 초과할당 감지 테스트
- [x] facts 미존재 호스트 경고 테스트

#### A-4. GitOps manifest 무결성 테스트 ✅ (Sprint 41, +2 tests)
- [x] 모든 kustomization.yaml의 resources/helmCharts 참조 파일 존재 검증
- [x] gitops/ 디렉토리 전체 YAML 파싱 검증 (multi-document 지원)

#### A-5. `scalex sdi sync` 부작용 감지 강화 ✅ (Sprint 41, +6 tests)
- [x] etcd quorum loss 감지 (`detect_quorum_loss_risk()`)
- [x] 3 CP 중 2 호스트 제거 시 quorum 상실 감지
- [x] 3 CP 중 1 호스트 제거 시 quorum 안전 확인
- [x] 단일 CP 제거 시 치명적 quorum 상실 감지
- [x] 동일 호스트에 다수 CP 배치 시 위험 감지
- [x] 워커 전용 풀은 quorum 위험 없음 확인
- [x] 멀티-풀 독립 quorum 위험 평가

#### A-6. 에러 UX + Workflow 의존성 ✅ (Sprint 41, +8 tests)
- [x] `check_workflow_dependencies()`: 명령어별 필수 artifact 의존성 검증
- [x] `format_workflow_errors()`: 사용자 친화적 에러 메시지 포맷팅
- [x] sdi-init, sdi-init-spec, cluster-init, bootstrap 4개 명령어 워크플로우 정의
- [x] 누락 artifact별 구체적 해결 명령어 안내

#### A-7. README Installation Guide 정합성
- [ ] 각 Step의 전제 조건이 이전 Step의 출력과 일치하는지 프로그래밍적 검증
- [ ] dry-run 시나리오의 전체 파이프라인 통합 테스트

### Phase B: 실환경 E2E 검증 (⬜ 인프라 필요)

#### B-1. SDI 가상화 E2E (CL-1, CL-13 검증)
- [ ] `scalex facts --all` → 4노드 실제 SSH 접속 → JSON 수집
- [ ] `scalex sdi init` (no flag) → 호스트 준비 (KVM, bridge)
- [ ] `scalex sdi init config/sdi-specs.yaml` → OpenTofu HCL 생성 → `tofu apply` → VM 5개 생성
- [ ] `scalex get sdi-pools` → 2개 풀(tower, sandbox) 확인
- [ ] 멱등성 검증: `sdi init` 재실행 → 상태 변화 없음
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` → 초기화 → 재생성 → 동일 결과

#### B-2. Kubespray + Multi-cluster E2E (CL-7, CL-8 검증)
- [ ] `scalex cluster init config/k8s-clusters.yaml` → Kubespray 실행 → tower + sandbox
- [ ] `kubectl --kubeconfig=_generated/clusters/tower/kubeconfig.yaml get nodes` 정상
- [ ] `kubectl --kubeconfig=_generated/clusters/sandbox/kubeconfig.yaml get nodes` 정상
- [ ] 멱등성 검증: `cluster init` 재실행 → 상태 변화 없음

#### B-3. ArgoCD Bootstrap E2E (CL-2, CL-7 검증)
- [ ] `scalex secrets apply` → K8s Secrets 생성
- [ ] `scalex bootstrap` → ArgoCD Helm 설치 + 클러스터 등록 + spread.yaml 적용
- [ ] ArgoCD UI 접근 가능 확인
- [ ] 모든 Applications Synced/Healthy

#### B-4. 외부 접근 E2E (CL-3, CL-14, CL-15 검증)
- [ ] Tailscale IP → tower kubectl 접근
- [ ] Cloudflare Tunnel → ArgoCD UI (`cd.jinwang.dev`) 접근
- [ ] Keycloak Realm/Client 설정 (사용자 수동 작업)
- [ ] CF Tunnel OIDC → kubectl 접근
- [ ] LAN 내부 → 스위치 → kubectl 접근

### Phase C: 문서 최종화
- [ ] README Installation Guide를 실행 결과로 검증/수정
- [ ] REQUEST-TO-USER.md 업데이트
- [ ] DASHBOARD.md 최종 상태 반영

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

## Test Summary (579 tests — 모두 순수 함수 단위 테스트)

| Module | Tests | 주요 커버리지 |
|--------|:-----:|----------|
| core/validation | 209 | pool mapping, cluster IDs/names, CIDR overlap, DNS, bootstrap, 2-layer consistency, **resource over-allocation (+4), workflow deps (+8)** |
| commands/sdi | 47 | network resolve, host infra, pool state, clean, GPU |
| core/gitops | 41 | ApplicationSet, kustomization, sync waves, Cilium, **manifest integrity (+2)** |
| core/kubespray | 38 | inventory (SDI + baremetal), cluster vars, OIDC, single-node |
| core/sync | 33 | sync diff, VM conflict, severity, **quorum loss detection (+6)** |
| core/config | 29 | baremetal config, semantic validation, error UX, **circular ref/IP validation (+5)** |
| commands/cluster | 28 | cluster init, SDI/baremetal, gitops, E2E pipeline |
| commands/get | 24 | facts row, config status, SDI pools, clusters, JSON output |
| core/secrets | 23 | K8s secret generation, block scalar, JSON values |
| commands/status | 21 | platform status |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14 | ArgoCD helm, cluster add, kubectl apply |
| commands/facts | 14 | facts gathering, parse edge cases |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table |
| core/ssh | 5 | SSH command building |

---

## 2-Layer 템플릿 관리 체계

| Layer | 구성 파일 | 관리 범위 | 도구 |
|-------|----------|----------|------|
| **Infrastructure** | `config/sdi-specs.yaml` + `credentials/.baremetal-init.yaml` | SDI 가상화 + K8s 프로비저닝 | `scalex sdi init` -> `cluster init` |
| **GitOps** | `config/k8s-clusters.yaml` + `gitops/` YAMLs | 멀티-클러스터 형상 관리 | `scalex bootstrap` -> ArgoCD |

새 클러스터 추가 시:
1. `sdi-specs.yaml`에 pool 추가 (Infrastructure layer)
2. `k8s-clusters.yaml`에 cluster 정의 (Infrastructure layer)
3. `gitops/generators/`에 generator 추가 (GitOps layer)
4. 완료 -- ArgoCD가 자동 배포
