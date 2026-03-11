# PROMPT.md

> ScaleX-POD-mini 프로젝트의 핵심 철학과 개발 방향을 정의하는 문서.
> 이 문서는 AI 에이전트 및 개발자가 프로젝트의 의도와 맥락을 정확히 이해하고, 일관된 방향으로 개발을 진행하기 위한 기준점입니다.

---

## 1. 핵심 철학

### 1.1 SDI: 실제 하드웨어와 클러스터링을 위한 하드웨어의 분리

**Software-Defined Infrastructure (SDI)** — OpenTofu를 활용한 리소스 가상화 레이어.

물리 하드웨어와 Kubernetes 클러스터 사이에 가상화 레이어를 두어, 동일한 워크플로우(`scalex sdi init → cluster init`)로 **단일 노드에서도, 수천 개 노드에서도** 프로비저닝할 수 있는 구조를 실현한다. 본 레포지토리는 4개 노드를 샘플로 활용하며, 단일 노드 환경에서의 테스트도 완료해야 한다.

- 베어메탈 리소스를 통합 리소스 풀로 추상화
- VM 풀 단위로 목적별 분할 (tower-pool, sandbox-pool, ...)
- 노드 수에 독립적인 일관된 프로비저닝 인터페이스

### 1.2 무한 확장 가능한 멀티-클러스터 구조

**템플릿 기반 Kubespray/Ansible** 프로비저닝과 **ArgoCD ApplicationSets** 기반 GitOps 배포.

새 클러스터 추가 절차:
1. `sdi-specs.yaml`에 pool 추가
2. `k8s-clusters.yaml`에 cluster 정의
3. `gitops/generators/`에 generator 추가
4. 완료 — ArgoCD가 자동 배포

공통 앱(Cilium, cert-manager, Kyverno 등)은 `gitops/common/`에서 한 번 정의하면 모든 클러스터에 일괄 적용되고, 클러스터별 파라미터 조정은 generator의 values로 처리한다.

### 1.3 역할 분리형 아키텍처 (Role-based Disaggregated Multi-cluster)

단일 모놀리식 클러스터 대신, **역할별로 특화된 클러스터**로 분리하고 이를 멀티-클러스터로 묶어 관리한다.

- **Tower**: 메타-관리 클러스터 (ArgoCD, Keycloak, cert-manager, 멀티-클러스터 스케줄링)
- **워크로드 클러스터**: 역할별로 분리/확장 (본 레포에서는 sandbox 클러스터로 검증)

향후 제5, 제6 클러스터로 확장할 수 있도록, 공통 부분과 클러스터별 독립 부분을 명확히 분리하여 설계한다.

### 1.4 이중 접근성과 보안

공인 IP 유무에 관계없이 클러스터에 접근할 수 있는 구조:

| 접근 경로 | 인증 방식 | 사용 시나리오 |
|-----------|----------|-------------|
| **Cloudflare Tunnel** | OIDC만 가능 (client cert 불가) | OIDC 설정 완료 후 외부 접근 |
| **Tailscale** | cert + token | Pre-OIDC 외부 접근의 유일한 방법 |
| **LAN** | 모든 방식 | 동일 네트워크 내 접근 |

OIDC(Keycloak) 기반 인증으로 별도 VPN 없이 `kubectl` 접근이 가능하다.

### 1.5 뉴비 친화적이면서도 맞춤형 최적화 지원

`scalex` CLI 하나만 이해하면 전체 인프라를 운용할 수 있다.

- `scalex facts`: 베어메탈 하드웨어 정보를 지능적으로 수집/저장
- `scalex kernel-tune`: 수집된 정보를 토대로 커널 파라미터 최적화 권장
- 저수준 최적화(GPU 패스스루, VFIO, IOMMU 등)를 위한 인터페이스 제공

> **향후 계획**: 실제 최적화 로직은 본 레포지토리에 별도 서브 디렉토리를 만들어 다양한 HW를 지원하도록 지속 개발/개선할 예정.

### 1.6 템플릿 기반 2-Layer 관리 체계

전체 프로젝트는 2개의 관리 레이어로 분리된다:

| Layer | 구성 파일 | 관리 범위 |
|-------|----------|----------|
| **Infrastructure** | `config/sdi-specs.yaml` + `credentials/.baremetal-init.yaml` | SDI 가상화 + K8s 프로비저닝 |
| **GitOps** | `config/k8s-clusters.yaml` + `gitops/` YAMLs | 멀티-클러스터 형상 관리 (ArgoCD) |

"가상화 인프라 파트"와 "멀티-클러스터 형상 관리 파트"를 명확히 구분하여, 인프라 변경과 애플리케이션 배포를 독립적으로 관리할 수 있다.

### 1.7 Test-Driven Development (TDD)

모든 개발은 사용자 관점에서의 테스트를 **구체적이고 까다롭게** 설계한다.

- RED → GREEN → REFACTOR 사이클 엄수
- 최소 기능 단위로 진행
- 기능 개발 완료 후 반드시 커밋하여 기록
- 테스트는 순수 함수 기반 오프라인 테스트 (현재 554개)
- 실환경 E2E 테스트는 물리 인프라에서 별도 수행

---

## 2. 프로젝트 배경

### 2.1 ScaleX-POD란?

`ScaleX-POD`는 베어메탈 프로비저닝부터 GitOps 기반 CI/CD, 관측가능성(Observability), 에이전틱 운영(Agentic Operation)까지 **전체 인프라 및 DevOps 스택을 통합 관리하는 umbrella project/framework**이면서 동시에 GIST(광주과학기술원)의 연구장비를 의미한다.

ScaleX-POD의 핵심 철학은 **역할-기반 분리형 멀티-클러스터(Role-based Disaggregated Multi-cluster)**: 단일 모놀리식 클러스터 대신, 역할별로 특화된 클러스터로 노드들을 분리하고 이를 멀티-클러스터로 묶어 관리하면서 워크로드를 특성에 맞게 클러스터 단위로 분산시킨다.

> **참고**: 인프라에서 *분리형(disaggregated)*이라는 용어는 일반적으로 하드웨어 수준에서 컴퓨팅, 메모리, 스토리지를 물리적으로 분해하는 것을 의미한다 (예: CXL 기반 분리형 메모리 풀). ScaleX-POD는 이를 포함함과 동시에 융복합 워크로드의 특성에 맞는 최적화를 목표로 *역할-기반 분리형(role-based disaggregated)*이라는 상호보완적인 개념을 사용한다.

#### ScaleX-POD 클러스터 구성 (5개)

| 클러스터 | 역할 | 비유 |
|---------|------|------|
| **MobileX** | 엣지 디바이스 클러스터링, 센싱/전처리/판단/액추에이션 | 오감 + 팔과 다리 |
| **TwinX** | 디지털 트윈 렌더링 + 협업 환경 (NVIDIA Omniverse) | 시뮬레이션 모니터 |
| **AutoX** | 대규모 AI 워크로드 추론 + 데이터 처리 | 예측/판단의 뇌 |
| **DataX** | 이기종 스토리지 기반 중앙 데이터레이크 | 안정적 데이터 창고 |
| **ScaleX-Tower** | 멀티-클러스터 메타-관리 (ArgoCD, Keycloak, Kueue) | 관제탑 |

각 클러스터의 공통 요구사항:
- 유저관리/멀티-테넌시/RBAC
- 클러스터별 Rook-Ceph 스토리지 스택 (DataX는 중앙, 나머지는 버퍼/캐시 용도)
- GPU 관리 및 MIG 가상화 (TwinX, AutoX)
- 관측성(Observability) 스택

### 2.2 ScaleX-POD-mini란?

ScaleX-POD의 실제 운용 장비에서는 실험/실증을 진행할 수 없다. 이 문제를 해결하기 위해:

1. **가상화된 인프라 레이어**와 **멀티-클러스터 레이어**를 분리하고
2. 그 위에 **샌드박스 멀티-클러스터**를 구축하여
3. ScaleX-POD의 핵심 기능을 개발 및 마이그레이션 계획/테스트를 가능하게 하는 프로젝트

가 바로 **ScaleX-POD-mini**이다.

#### 핵심 가치

- **디렉토리 구조 공유**: ScaleX-POD 프로젝트는 ScaleX-POD-mini의 디렉토리 구조를 동일하게 사용하는 것이 목표. ScaleX-POD-mini를 검증하는 것이 지금 단계에서 가장 중요한 작업
- **확장 가능한 설계**: 향후 제5, 제6 클러스터 추가 시에도 공통 부분과 클러스터별 독립 부분이 자연스럽게 분리되는 구조
- **Safety-first 마이그레이션**: TDD 기반으로 의존 프로젝트들의 버전 및 커널 수준까지의 실패 상황을 편집증적으로 탐색하여 구체적이고 안전하게 준비
- **통합/마이그레이션 목표**: 독립적으로 운용 중인 ScaleX-POD 클러스터들의 SW 버전 충돌을 해결하고, 역할-기반 분리형 멀티-클러스터로 통합

---

## 3. 개발 원칙

### 3.1 코드 품질

- Rust CLI: 순수 함수 기반, I/O 분리, `thiserror` + `clap derive`
- YAML: 2-space indent, kebab-case 리소스 이름, yamllint 통과
- 모든 generator는 부작용(side effect) 없는 순수 함수

### 3.2 GitOps-First

부트스트랩 이후 모든 클러스터 상태는 ArgoCD가 관리한다. 수동 `kubectl apply`는 부트스트랩 시점에만 허용.

### 3.3 멱등성 보장

모든 CLI 명령은 안전하게 재실행 가능해야 한다. `sdi init`, `cluster init`, `bootstrap` 모두 멱등성을 보장한다.

### 3.4 시크릿 관리

실제 시크릿은 `credentials/` (gitignored)에 보관하고, `.example` 템플릿만 커밋한다. 시크릿이 git에 커밋되는 것을 방지하는 구조적 장치를 유지한다.
