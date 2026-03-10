# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Current Status

- **Tests**: 223 pass / clippy 0 warnings / fmt clean
- **Code**: ~10,500 lines Rust, 27 source files, ~200 pure functions
- **GitOps**: 33 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## Checklist Gap Analysis

> 아래는 사용자 Checklist 15개 항목 각각에 대한 현재 상태와 갭 분석이다.
> 상태: PASS = 완료 및 검증됨, PARTIAL = 부분 구현, FAIL = 미구현 또는 미검증

### CL-1: 4개 노드 OpenTofu 가상화 + 2-클러스터 구조

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| SDI 모델 (SdiSpec, NodeSpec) | PASS | `models/sdi.rs` — 파싱/직렬화 테스트 통과 |
| OpenTofu HCL 생성 | PASS | `core/tofu.rs` — 호스트 인프라 + VM 생성 HCL 순수 함수 |
| sdi-specs.yaml 예제 (4노드, 2풀) | PASS | `config/sdi-specs.yaml.example` — tower + sandbox 풀 정의 |
| baremetal-init.yaml 스키마 (3가지 접근 방식) | PASS | direct / external IP / ProxyJump 모두 지원 |
| 단일 노드 SDI 검증 | **FAIL** | tower + sandbox 모두 1개 노드에 배치하는 설정 테스트 없음 |
| `scalex sdi init` (no flag) 동작 | PARTIAL | 호스트 준비만 수행, "통합 리소스 풀 관측" 기능 미구현 |
| 실제 HW 테스트 | **FAIL** | 물리 인프라 접근 필요 — 오프라인 테스트 불가 |

**근본 원인**: HCL 생성 로직은 완성되었으나, (1) 단일 노드 시나리오를 위한 테스트가 없고, (2) `sdi init` (no flag)의 "전체 리소스 풀 관측" 의미가 코드에서 불명확함.

### CL-2: Cloudflare Tunnel ArgoCD/GitOps 방식

**상태: PASS**

- `gitops/tower/cloudflared-tunnel/` 존재 (kustomization.yaml + values.yaml)
- tower-generator.yaml ApplicationSet에서 참조됨
- Sync wave 3에서 배포

### CL-3: Cloudflare Tunnel 설정 완료 여부

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| GitOps 배포 자동화 | PASS | cloudflared-tunnel kustomization 존재 |
| 사용자 매뉴얼 가이드 | PASS | `docs/ops-guide.md` Step 1-6 상세 기술 |
| 사용자 실제 설정 완료 | PARTIAL | 사용자가 `playbox-admin-static` 이름으로 설정 — ops-guide는 `playbox-tunnel` 사용 |
| 외부 kubectl 접근 검증 | **FAIL** | Keycloak 미설정 시 Cloudflare Tunnel만으로 kubectl 접근 가능 여부 미검증 |

**근본 원인**: (1) ops-guide의 터널 이름이 사용자 실제 설정과 불일치, (2) Keycloak 없이 Cloudflare Tunnel + kubectl 조합의 동작 여부 미검증.

### CL-4: Rust CLI + FP 스타일

**상태: PASS**

- Rust (clap 4 derive + serde + thiserror + anyhow)
- 순수 함수: HCL 생성, inventory 생성, validation 모두 side-effect 없음
- 213 tests, 0 clippy warnings, fmt clean
- `#[allow(dead_code)]` 최소화, release profile LTO+strip

### CL-5: 사용자 친절 가이드

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| 운영 가이드 (ops-guide.md) | PASS | Cloudflare, Keycloak, Kernel, 접근 방법 상세 |
| 설치 가이드 (SETUP-GUIDE.md) | PASS | 단계별 설명 존재 |
| 아키텍처 문서 (ARCHITECTURE.md) | PASS | 한/영 병기, 다이어그램 존재 |
| 트러블슈팅 (TROUBLESHOOTING.md) | PASS | 일반적 문제 해결 가이드 |
| CLI 도움말 (--help) | PASS | clap derive로 자동 생성 |
| "초보자도 따라할 수 있는" 수준 | **FAIL** | 전제 조건 확인, 에러 복구 가이드 부족 |

### CL-6: README.md 디테일

**상태: PARTIAL**

| 항목 | 상태 | 비고 |
|------|------|------|
| Architecture Overview | PASS | 5-Layer 구조, 2-Cluster 설계 |
| CLI Reference | PASS | 모든 명령어 테이블 |
| GitOps Pattern | PASS | Bootstrap chain, sync waves |
| Project Structure | PASS | 디렉토리 트리 |
| Testing | PASS | 테스트 명령어 |
| 설계 철학 섹션 | **FAIL** | 7가지 원칙이 README에 없음 |
| 상세 Installation Guide | **FAIL** | "Quick Start"만 존재, 보장된 단계별 가이드 없음 |

### CL-7: README Installation Guide (end-to-end 보장)

**상태: FAIL**

현재 README에는 "Quick Start" 섹션만 있으며:
- 전제 조건 검증 단계 없음
- 에러 발생 시 복구 방법 없음
- `scalex get clusters` 동작 확인까지의 end-to-end 검증 절차 없음
- "완전히 초기화된 베어메탈"부터 시작하는 시나리오 미기술

**근본 원인**: Quick Start는 "이미 환경이 준비된" 사용자를 위한 것. 초보자용 step-by-step Installation Guide가 별도로 필요.

### CL-8: CLI 기능 완전성

**상태: PARTIAL**

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | PASS | SSH로 HW 정보 수집, NodeFacts 모델 완전 (CPU/mem/GPU/disk/NIC/PCIe/kernel) |
| `scalex sdi init` (no flag) | PARTIAL | 호스트 준비만 수행. "전체 리소스 풀 관측" 미구현 |
| `scalex sdi init <spec>` | PASS | SDI 풀 생성 HCL + host prepare + VM 프로비저닝 |
| `scalex sdi clean --hard --yes-i-really-want-to` | PASS | 전체 초기화 로직 구현 |
| `scalex sdi sync` | PASS | 노드 추가/제거 diff 계산 + 동기화 |
| `scalex cluster init <config>` | PASS | Kubespray inventory/vars 생성, OIDC 지원 |
| `scalex get baremetals` | PASS | facts JSON → 테이블 포맷 |
| `scalex get sdi-pools` | PASS | SDI 풀 상태 테이블 |
| `scalex get clusters` | PASS | 클러스터 인벤토리 테이블 |
| `scalex get config-files` | PASS | 설정 파일 검증 테이블 |
| `scalex secrets apply` | PASS | K8s secret 생성 |
| `scalex status` | PASS | 5-layer 플랫폼 상태 |
| `scalex kernel-tune` | PASS | 커널 파라미터 권장 |

**근본 원인**: `sdi init` (no flag)의 "통합 리소스 풀" 의미가 설계에서 명확히 정의되지 않음.

### CL-9: 베어메탈 모드 확장성

**상태: PARTIAL**

- `ClusterMode::Baremetal` enum 존재
- k8s-clusters.yaml.example에 baremetal 예제 (주석)
- `validate_cluster_sdi_pool_mapping`에서 baremetal skip 로직
- **BUT**: baremetal 모드 cluster init 시 inventory 생성 테스트 부족
- k3s 완전 제거 확인 (test_no_k3s_references_in_project_files)

### CL-10: 보안 정보 템플릿화

**상태: PASS**

- `credentials/` 디렉토리: `.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`
- `.gitignore`에 실제 credentials 파일 제외
- `scalex secrets apply`로 K8s secret 자동 생성
- 민감 정보는 `.env` 변수 참조 방식 (코드에 하드코딩 없음)

### CL-11: 커널 파라미터 튜닝

**상태: PASS**

- `scalex kernel-tune` 명령어 구현 (14 tests)
- `docs/ops-guide.md`에 스토리지/네트워크/IOMMU 튜닝 가이드
- `scalex facts`로 현재 커널 파라미터 수집
- cloud-init에서 기본 K8s 네트워크 파라미터 자동 적용

### CL-12: 디렉토리 구조

**상태: PARTIAL**

| 필수 디렉토리 | 상태 | 비고 |
|--------------|------|------|
| `scalex-cli/` | PASS | Rust CLI |
| `gitops/common/` | PASS | cert-manager, cilium-resources, kyverno, kyverno-policies |
| `gitops/tower/` | PASS | argocd, cilium, cert-issuers, keycloak, cloudflared-tunnel, cluster-config, socks5-proxy |
| `gitops/sandbox/` | PASS | cilium, cluster-config, local-path-provisioner, rbac, test-resources |

| 추가 디렉토리 | 필요성 | 비고 |
|--------------|--------|------|
| `ansible/` | 필요 | 노드 준비 playbook |
| `kubespray/` | 필요 | submodule + Jinja2 템플릿 |
| `credentials/` | 필요 | 사용자 비밀 정보 (.gitignored) |
| `config/` | 필요 | 사용자 설정 예제 |
| `client/` | 필요 | OIDC kubeconfig 생성 |
| `docs/` | 필요 | 운영 가이드 |
| `tests/` | 필요 | 테스트 러너 |
| `.legacy-*` | **삭제 대기** | git status에서 'D' 마킹 — 커밋 필요 |

**근본 원인**: 레거시 파일 삭제가 git에 커밋되지 않음.

### CL-13: 멱등성

**상태: PARTIAL**

- HCL 생성 멱등성 테스트 존재 (`test_generate_tofu_host_infra_idempotent`)
- 코드 설계상 순수 함수이므로 동일 입력 → 동일 출력 보장
- **BUT**: I/O 오케스트레이션 (tofu apply, kubespray) 멱등성 미검증
- `sdi clean` → `sdi init` 사이클 테스트 없음

### CL-14: Cloudflare Tunnel 사용자 가이드 + 외부 kubectl

**상태: PARTIAL**

- ops-guide.md에 상세 가이드 존재
- 사용자가 `playbox-admin-static`으로 설정 완료
- **BUT**: ops-guide에서 터널 이름이 `playbox-tunnel`로 되어 있어 불일치
- Keycloak 없이 Cloudflare Tunnel만으로 kubectl 접근: API 프록시 설정 시 가능하나 미검증
- `api.k8s.jinwang.dev` → `https://kubernetes.default:443` 라우팅 필요

### CL-15: NAT 접근 방법

**상태: PASS**

- ops-guide.md 섹션 4에 상세 기술:
  - 외부: Cloudflare Tunnel (웹), Tailscale VPN (SSH/kubectl)
  - LAN: 직접 SSH, 스위치 경유
- ProxyJump 패턴 문서화
- kubeconfig 경로별 클러스터 접근 방법 기술

---

## Sprint Plan

> 최소 핵심 기능 단위로 분할. 각 Sprint는 TDD 방식으로 진행.
> RED (테스트 작성) → GREEN (구현) → REFACTOR → COMMIT

### Sprint 1: 테스트 강화 + 레거시 정리 (코드 품질)

> 물리 인프라 접근 없이 수행 가능한 작업

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 1.1 | 단일 노드 SDI 설정 테스트 (tower+sandbox on 1 host) | CL-1 | TODO |
| 1.2 | Baremetal 모드 cluster init inventory 생성 테스트 | CL-9 | TODO |
| 1.3 | 멱등성 종합 테스트 (동일 입력 2회 → 동일 출력) | CL-13 | TODO |
| 1.4 | E2E dry-run 파이프라인 테스트 (facts→sdi→cluster→secrets) | CL-8 | TODO |
| 1.5 | 레거시 파일 삭제 커밋 (.legacy-*, lib/, tests/bats/) | CL-12 | TODO |

### Sprint 2: README + 문서 강화

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 2.1 | README.md에 설계 철학 섹션 추가 | CL-6 | TODO |
| 2.2 | README.md에 상세 Installation Guide 작성 | CL-7 | TODO |
| 2.3 | ops-guide.md 터널 이름 `playbox-admin-static` 반영 | CL-14 | TODO |
| 2.4 | Cloudflare Tunnel만으로 kubectl 접근 시나리오 문서화 | CL-3, CL-14 | TODO |

### Sprint 3: `sdi init` (no flag) 리소스 풀 뷰 구현

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 3.1 | `sdi init` (no flag) 시 통합 리소스 풀 상태 JSON 생성 | CL-1, CL-8 | TODO |
| 3.2 | `scalex get sdi-pools` 에서 통합 뷰 표시 | CL-8 | TODO |
| 3.3 | 단일 노드에서 전체 파이프라인 dry-run 테스트 | CL-1 | TODO |

### Sprint 4: 확장성 검증

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 4.1 | 3번째 클러스터 추가 테스트 (gitops generator 자동 생성) | CL-8 | TODO |
| 4.2 | 2-Layer 템플릿 관리 검증 (SDI values ↔ GitOps values 분리) | CL-6 | TODO |
| 4.3 | `scalex sdi sync` 노드 추가/제거 시 사이드이펙트 테스트 | CL-8 | TODO |

### Sprint 5: 실환경 검증 (물리 인프라 필요)

| # | Task | Checklist | 상태 |
|---|------|-----------|------|
| 5.1 | `scalex facts --all` 실행 (4노드) | CL-1, CL-8 | TODO |
| 5.2 | `scalex sdi init sdi-specs.yaml` 실행 | CL-1 | TODO |
| 5.3 | `scalex cluster init k8s-clusters.yaml` 실행 | CL-8 | TODO |
| 5.4 | `scalex secrets apply` + `kubectl apply -f gitops/bootstrap/spread.yaml` | CL-8 | TODO |
| 5.5 | 외부망에서 `kubectl get pods` 접근 검증 | CL-14 | TODO |
| 5.6 | `scalex sdi clean --hard --yes-i-really-want-to` + 재구축 (멱등성) | CL-13 | TODO |

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
|  get baremetals/sdi-pools/clusters        |
|  secrets apply / status / kernel-tune     |
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
