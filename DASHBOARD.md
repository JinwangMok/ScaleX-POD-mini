# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 152 tests pass / clippy clean / fmt clean

**코드 규모**: ~8,360 lines Rust, 23 source files, 55+ pure functions
**GitOps**: 31 YAML files (bootstrap + generators + common/tower/sandbox apps)

---

## 이전 DASHBOARD.md 비판적 분석 (2026-03-10)

이전 DASHBOARD는 Checklist 14개 항목 중 11개를 "완료"로 표기했으나, **실제 검증 없이 코드 존재 여부만으로 판단**한 부분이 다수 존재.

### 비판 1: "95% 완료"라는 CLI 기능 평가 — 과대평가
- `scalex facts`: 구현됨. 그러나 Checklist #8이 정의한 `baremetal-init.yaml` 스키마의 `reachable_via`, `reachable_node_ip` 필드가 코드에 반영되었는지 검증 없음
- `scalex sdi init` (no flag): 호스트 인프라만 구성. "전체 리소스 풀로 관측"되는지 검증 없음
- `scalex sdi sync`: 사이드 이펙트 감지/예방 로직 구현됨(VM 충돌 감지). 그러나 실제 동작 검증 없음
- `scalex cluster init`: Kubespray inventory + vars 생성됨. 실제 Kubespray 실행 후 kubeconfig 획득까지 e2e 검증 없음

### 비판 2: k3s 잔존 참조를 "미완성"으로만 분류 — 범위 축소
- 이전 DASHBOARD는 `values.yaml`, `lib/cluster.sh`, `tests/fixtures/`만 나열
- **누락**: `docs/architecture-overview.drawio` (k3s 아키텍처 다이어그램), `docs/provisioning-flow.drawio` (k3s 설치 흐름), `docs/ops-guide.md:48` (`./playbox bootstrap` 참조)

### 비판 3: docs 레거시 참조 범위 축소
- `SETUP-GUIDE.md`, `TROUBLESHOOTING.md`만 지적했으나, `ops-guide.md`도 `./playbox` 참조 포함

### 비판 4: GitOps repo URL 미검증
- 모든 gitops YAML이 `https://github.com/JinwangMok/k8s-playbox.git` 참조
- 실제 레포는 `playbox-provisioning` — 불일치 여부 사용자 확인 필요

### 비판 5: "완료" 항목에 대한 검증 부재
- `test_no_k3s_references_in_project_files` 테스트가 존재하나 `values.yaml` + fixtures만 검사 — drawio/docs 미포함
- GitOps 정합성 테스트는 generator 참조만 검증 — dead code 검출 테스트 없음

---

## Checklist 재검증 (2026-03-10, 심층 분석)

| # | 질문 | 상태 | 근거 | 미해결/결함 |
|---|------|------|------|-------------|
| 1 | OpenTofu 전체 가상화 + 리소스 풀 | **코드 구현됨, 미검증** | tofu.rs: HCL 생성, sdi.rs: host prep + pool state | sdi init (no flag) 리소스 풀 "관측" 미구현 — resource_pool.rs 요약 표시만 |
| 2 | DataX kubespray 반영 | **핵심 반영됨** | kubespray.rs: kube_proxy_remove, addon disable, cilium CNI | legacy에 127개 설정 존재 — 전수 대조 미완 |
| 3 | Keycloak 설정 | **가이드 완료** | gitops/tower/keycloak/ + docs/ops-guide.md | 사용자 Realm/Client 설정 필요 |
| 4 | CF tunnel GitOps 배포 | **완료** | gitops/tower/cloudflared-tunnel/ + sync-wave 3 | — |
| 5 | CF tunnel 완성 | **가이드 완료** | docs/ops-guide.md Section 1 | 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | main.rs: `name = "scalex"` | — |
| 7 | Rust + FP 스타일 | **완료** | 149 tests, 55+ pure functions, clippy clean | — |
| 8 | CLI 기능 완성도 | **80%** | facts/sdi/cluster/get/secrets 구현 | 아래 상세 참조 |
| 9 | 베어메탈 확장성 / k3s 배제 | **결함 있음** | ClusterMode::Baremetal 지원 | **k3s 참조 잔존** (drawio/docs/playbox) |
| 10 | Secrets 구조화 | **완료** | secrets.rs + credentials/.example | — |
| 11 | 커널 튜닝 가이드 | **완료** | docs/ops-guide.md Section 3 | — |
| 12a | 디렉토리 구조 | **거의 완료** | Kyverno → common/ | **common/cilium/ dead code** |
| 12b | 멱등성 | **완료** | 순수 함수, re-runnable | — |
| 13 | CF tunnel 가이드 | **결함 있음** | docs/ops-guide.md Section 1 | `./playbox` 참조 잔존 (line 48) |
| 14 | 외부 접근 가이드 | **완료** | docs/ops-guide.md Section 4 | — |

### Checklist #8 CLI 기능 상세

| 명령어 | 구현 | 테스트 | 미해결 |
|--------|------|--------|--------|
| `scalex facts` | ✅ SSH 기반 수집, 파싱 | ✅ 4개 테스트 | — |
| `scalex sdi init` (no flag) | ✅ 호스트 준비 + 인프라 HCL | ✅ tofu 테스트 | 리소스 풀 통합 관측 UX 개선 필요 |
| `scalex sdi init <spec>` | ✅ VM HCL + pool state | ✅ tofu/sdi 테스트 | — |
| `scalex sdi clean --hard` | ✅ tofu destroy + 노드 클린업 | ⚠️ 로직만 존재 | — |
| `scalex sdi sync` | ✅ diff 계산 + VM 충돌 감지 | ✅ sync 테스트 3개 | — |
| `scalex cluster init` | ✅ inventory + vars + kubespray | ✅ cluster 테스트 7개 | — |
| `scalex get baremetals` | ✅ facts 테이블 출력 | ✅ | — |
| `scalex get sdi-pools` | ✅ pool state 테이블 | ✅ 3개 테스트 | — |
| `scalex get clusters` | ✅ 클러스터 테이블 | ✅ | — |
| `scalex get config-files` | ✅ 설정 파일 검증 | ✅ 6개 테스트 | — |

---

## 결함 목록 (우선순위순)

### DEFECT-1: k3s 잔존 참조 (Checklist #9) — ✅ 해결됨

| 파일 | 참조 | 상태 |
|------|------|------|
| `docs/ops-guide.md:48` | `./playbox bootstrap` | ✅ `scalex secrets apply`로 교체 |
| `docs/architecture-overview.drawio` | `k3s on libvirt` | ⚠️ drawio 파일 — 수동 재작성 필요 |
| `docs/provisioning-flow.drawio` | `k3s 설치` 흐름 | ⚠️ drawio 파일 — 수동 재작성 필요 |
| `playbox`, `lib/cluster.sh` | k3s 레거시 코드 | 이미 legacy로 분류됨 (OK) |
| `.legacy-tofu/` | k3s_version variable | 이미 .legacy- prefix (OK) |

### DEFECT-2: common/cilium/ dead code (Checklist #12a) — ✅ 해결됨

`gitops/common/cilium/` 이미 이전 세션에서 삭제됨.
`test_no_gitops_dead_code_directories` 테스트로 검증됨.

### DEFECT-3: docs 레거시 참조 — ✅ 해결됨

| 파일 | 문제 | 상태 |
|------|------|------|
| `docs/SETUP-GUIDE.md` | 이전 `./playbox` 참조 | ✅ 이전 세션에서 scalex 기반 재작성 완료 |
| `docs/TROUBLESHOOTING.md` | 이전 `./playbox destroy-all` 참조 | ✅ 이전 세션에서 scalex 기반 재작성 완료 |
| `docs/ops-guide.md:48` | `./playbox bootstrap` 참조 | ✅ `scalex secrets apply`로 교체 |
| `docs/CONTRIBUTING.md` | `values.yaml` 참조 | ✅ config YAML 기반으로 교체 |
| `docs/NETWORK-DISCOVERY.md` | `./playbox discover-nics` | ✅ `scalex facts --all`로 교체 |

`test_no_legacy_playbox_references_in_docs` 테스트 (5개 파일 검증)로 보호됨.

### DEFECT-4: GitOps repo URL — ✅ 일관성 검증됨

모든 gitops YAML이 `https://github.com/JinwangMok/k8s-playbox.git` 일관되게 참조.
`test_gitops_repo_url_consistency` 테스트로 보호됨.
실제 레포 이름 불일치 여부는 **사용자 확인 필요**.

### DEFECT-5: k3s 검증 테스트 범위 — ✅ 해결됨

`test_no_k3s_references_in_project_files` 범위 확장 완료 (docs 포함).
`test_no_legacy_playbox_references_in_docs` 신규 테스트 추가 (5개 docs 파일).
`test_no_gitops_dead_code_directories` 신규 테스트 추가.

---

## 실행 계획 — TDD, 최소 핵심 단위

### Unit 1: k3s 잔존 참조 정리 + 검증 테스트 확장
> Checklist #9, DEFECT-1, DEFECT-5

**1-1 RED**: k3s-free 검증 테스트 범위 확장 — drawio, docs, ops-guide 포함
**1-2 GREEN**: drawio 파일 k3s 참조 제거, docs 참조 수정
**1-3 REFACTOR**: 테스트 통과 확인

### Unit 2: common/cilium/ dead code 정리 + GitOps 정합성 테스트
> DEFECT-2

**2-1 RED**: "generator가 참조하지 않는 gitops 디렉토리가 없어야 한다" 테스트
**2-2 GREEN**: `gitops/common/cilium/` 삭제
**2-3 REFACTOR**: 기존 gitops 테스트 업데이트

### Unit 3: docs 레거시 참조 수정
> DEFECT-3

**3-1**: ops-guide.md의 `./playbox` → `scalex` 교체
**3-2**: SETUP-GUIDE.md scalex 기반 재작성
**3-3**: TROUBLESHOOTING.md scalex 기반 재작성

### Unit 4: GitOps repo URL 정합성 테스트
> DEFECT-4

**4-1 RED**: 모든 generator의 repoURL이 일관적인지 검증 테스트
**4-2 GREEN**: 필요시 URL 교체 (사용자 확인 후)

---

## 사용자 수동 작업 (코드로 해결 불가)

1. **Cloudflare Tunnel WebUI 설정** → `docs/ops-guide.md` Section 1
2. **Keycloak Realm/Client 설정** → `docs/ops-guide.md` Section 2
3. **credentials/ 파일 작성** (.baremetal-init.yaml, .env, secrets.yaml)
4. **config/ 파일 작성** (sdi-specs.yaml, k8s-clusters.yaml)
5. **GitOps repo URL 확인**: `k8s-playbox.git` vs `playbox-provisioning` 불일치 여부

---

## Kyverno 배치 결정: **Common** (확정)

모든 클러스터에 일관된 보안/운영 정책 적용. 클러스터별 예외는 PolicyException으로 관리.

---

## 아키텍처 요약

```
credentials/                    config/
.baremetal-init.yaml           sdi-specs.yaml
.env                           k8s-clusters.yaml
secrets.yaml
        │                           │
        ▼                           ▼
┌─────────────────────────────────────────┐
│              scalex CLI (Rust)          │
│  facts → sdi init → cluster init       │
│  get baremetals/sdi-pools/clusters      │
│  secrets apply                          │
└─────────────────────────────────────────┘
        │
        ▼
_generated/
├── facts/          (hardware JSON)
├── sdi/            (OpenTofu HCL + state)
└── clusters/       (inventory.ini + vars)
        │
        ▼
┌─────────────────────────────────────────┐
│           gitops/ (ArgoCD)             │
│  bootstrap/spread.yaml                 │
│  generators/{tower,sandbox}/           │
│  common/ tower/ sandbox/               │
└─────────────────────────────────────────┘
```
