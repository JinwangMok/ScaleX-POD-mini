# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical -> SDI (OpenTofu) -> Node Pools -> Cluster (Kubespray) -> GitOps (ArgoCD)

---

## Critical Gap Analysis (Sprint 42 재검증)

### 이전 DASHBOARD의 문제점과 근본 원인

이전 작업자는 15개 Checklist 중 대다수를 "코드 수준 완료"로 표시했다. 이 판단에는 3가지 근본적 오류가 있다.

**근본 원인 1: "코드 수준 검증"과 "운용 수준 검증"의 혼동**

589개 테스트는 **순수 함수의 출력 정확성**만 검증한다:
- `generate_tofu_main()` -> HCL 문자열 생성 ✅ -> `tofu apply`로 VM 생성? ❌
- `generate_inventory()` -> inventory.ini 생성 ✅ -> `ansible-playbook`으로 K8s 구축? ❌
- `generate_argocd_helm_install_args()` -> Helm 인수 생성 ✅ -> ArgoCD Sync? ❌

**근본 원인 2: 파이프라인 통합 검증 부재**

개별 함수의 입출력은 검증되었으나, **파이프라인 단계 간 데이터 흐름**은 검증되지 않았다:
- `scalex facts`의 출력 JSON이 `scalex sdi init`의 입력으로 올바르게 흘러가는가?
- `sdi-specs.yaml`의 pool 이름이 `k8s-clusters.yaml`의 `cluster_sdi_resource_pool`과 정확히 매핑되는가?
- `scalex cluster init`의 inventory가 실제 Kubespray에서 동작하는 형식인가?

일부 cross-config 검증 테스트가 존재하지만(validation.rs), 실제 `.example` 파일을 기준으로 한 end-to-end 파이프라인 테스트는 없다.

**근본 원인 3: 멱등성(CL-13) 검증 범위 오류**

"순수 함수를 2회 호출하면 동일 출력" = **함수 결정론(determinism)**, NOT **인프라 멱등성(idempotency)**.
진정한 멱등성: `sdi init` 2회 실행 -> OpenTofu state 불변 + VM 상태 동일.

---

## Checklist 재평가 (정직한 현황)

### 상태 범례

| 상태 | 의미 |
|------|------|
| ✅ VERIFIED | 테스트로 확인 완료 (코드 수준) |
| ✅ E2E-OK | 실환경에서 검증 완료 |
| 🔶 CODE-ONLY | 코드는 있으나 통합/실환경 미검증 |
| ❌ BLOCKED | 물리 인프라 또는 사용자 작업 필요 |

| # | 항목 | 상태 | 근거 |
|---|------|:----:|------|
| 1 | SDI 가상화 (4노드->풀->2클러스터) | 🔶 | HCL 생성기 OK. 단일노드 포함. `tofu apply` 미실행 |
| 2 | CF Tunnel GitOps 배포 | ✅ | `gitops/tower/cloudflared-tunnel/` Helm+kustomization. `playbox-admin-static` 터널명 일치 |
| 3 | CF Tunnel 완성도 | ❌ | 사용자 WebUI 터널 생성 완료. **Keycloak OIDC 미설정 -> kubectl via CF 불가** |
| 4 | Rust CLI + FP 원칙 | ✅ | 589 tests, `generate_*`/`run_*` 분리, 0 clippy warnings |
| 5 | 사용자 친절 가이드 | ✅ | `--help`, `--dry-run`, 에러 UX, workflow 의존성 안내 |
| 6 | README 상세 내용 | 🔶 | 522줄 포괄적 README. Installation Guide 8단계. **실행 미검증** |
| 7 | Installation Guide E2E | ❌ | README Steps 0-8 작성 완료. **베어메탈에서 한 번도 실행된 적 없음** |
| 8 | CLI 기능 전체 | 🔶 | 8개 명령어 구현. 파이프라인 통합 미검증 |
| 9 | Baremetal 확장성 | 🔶 | `cluster_mode: "baremetal"` 코드 경로 존재. 실행 미검증 |
| 10 | 시크릿 템플릿화 | ✅ | `.example` 6개 (credentials 4 + config 2). `.gitignore` 적용 |
| 11 | 커널 파라미터 튜닝 | 🔶 | `scalex kernel-tune` 구현 (14 tests). 실적용 미검증 |
| 12 | 디렉토리 구조 | ✅ | `scalex-cli/`, `gitops/{common,tower,sandbox}/`, `credentials/`, `config/` 명세 일치 |
| 13 | 멱등성 | 🔶 | 순수 함수 결정론 OK. Helm `upgrade --install`. **인프라 멱등성 미검증** |
| 14 | 외부 kubectl 접근 | ❌ | CF Tunnel -> `api.k8s.jinwang.dev` 구성됨. **Keycloak 미설정. Tailscale만 가능** |
| 15 | NAT 접근 경로 | ✅ | README에 4가지 접근 경로 문서화 (LAN/Tailscale/CF Tunnel/SOCKS5) |

**요약: 코드 수준 완료 6개 / 통합 미검증 6개 / 인프라 필요 3개**

---

## 실행 계획

### Sprint 42: 파이프라인 통합 테스트 (인프라 불필요 -- 즉시 실행)

#### 42-1. `.example` 파일 실제 파싱 + cross-config 통합 테스트
- [ ] `config/sdi-specs.yaml.example` -> `SdiSpec` 파싱 + pool 이름 검증
- [ ] `config/k8s-clusters.yaml.example` -> `K8sClustersConfig` 파싱 + cluster 이름 검증
- [ ] `.example` 파일 간 cross-reference: `cluster_sdi_resource_pool`이 SDI pool에 존재하는지
- [ ] `.example` 파일 간 cross-reference: `argocd.tower_manages` 클러스터가 정의되어 있는지

#### 42-2. Dry-run 파이프라인 통합 테스트
- [ ] `.example` 기반 SDI HCL 생성 -> pool별 VM 개수/스펙 일치
- [ ] `.example` 기반 Kubespray inventory 생성 -> control-plane/worker 역할 배정 정확
- [ ] `.example` 기반 cluster-vars 생성 -> common config 반영 확인
- [ ] `.example` 기반 bootstrap 인수 생성 -> tower kubeconfig 참조 정확

#### 42-3. ApplicationSet -> gitops/ 디렉토리 참조 검증
- [ ] tower-generator.yaml의 모든 app path가 `gitops/tower/` 하위에 존재
- [ ] sandbox-generator.yaml의 모든 app path가 `gitops/sandbox/` 하위에 존재
- [ ] common-generator의 모든 app path가 `gitops/common/` 하위에 존재
- [ ] 각 참조 디렉토리에 kustomization.yaml이 존재

#### 42-4. Baremetal 모드 (CL-9) inventory 생성 검증
- [ ] `cluster_mode: "baremetal"` + `baremetal_nodes` 정의 시 SDI 없이 inventory 생성
- [ ] baremetal 모드에서도 cluster-vars에 common config 정확히 반영

#### 42-5. CF Tunnel 시크릿 -> Helm values 정합성 (CL-10)
- [ ] `scalex secrets apply`의 Secret 이름이 cloudflared values.yaml의 `existingSecret`과 일치
- [ ] 생성된 Secret에 tunnel credentials JSON 포함

#### 42-6. README 정합성
- [x] README "554 tests" -> 589 tests로 업데이트 완료

### Sprint 43+: 실환경 E2E 검증 (인프라 필요)

#### 43-1. SDI 가상화 E2E (CL-1, CL-13)
- [ ] `scalex facts --all` -> 4노드 실제 SSH -> JSON 수집
- [ ] `scalex sdi init` (no flag) -> 호스트 준비 (KVM, bridge)
- [ ] `scalex sdi init config/sdi-specs.yaml` -> `tofu apply` -> VM 5개 생성
- [ ] `scalex get sdi-pools` -> 2개 풀(tower, sandbox) 확인
- [ ] 멱등성: `sdi init` 재실행 -> 상태 불변
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` -> 초기화 -> 재생성

#### 43-2. Kubespray + Multi-cluster E2E (CL-7, CL-8)
- [ ] `scalex cluster init config/k8s-clusters.yaml` -> tower + sandbox K8s
- [ ] `kubectl get nodes` 양쪽 클러스터 정상
- [ ] 멱등성: `cluster init` 재실행 -> 상태 불변

#### 43-3. ArgoCD Bootstrap E2E (CL-2, CL-7)
- [ ] `scalex secrets apply` -> K8s Secrets 생성
- [ ] `scalex bootstrap` -> ArgoCD + 클러스터 등록 + spread.yaml
- [ ] 모든 Applications Synced/Healthy

#### 43-4. 외부 접근 E2E (CL-3, CL-14, CL-15)
- [ ] Tailscale IP -> tower kubectl
- [ ] CF Tunnel -> ArgoCD UI (`cd.jinwang.dev`)
- [ ] Keycloak Realm/Client 설정 (사용자 수동)
- [ ] CF Tunnel OIDC -> kubectl
- [ ] LAN 스위치 접근 -> kubectl

### Sprint 44: 문서 최종화
- [ ] README Installation Guide를 실행 결과로 검증/수정
- [ ] 사용자 수동 작업 가이드 최종화

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

## 2-Layer 템플릿 관리 체계

| Layer | 구성 파일 | 관리 범위 | 도구 |
|-------|----------|----------|------|
| **Infrastructure** | `config/sdi-specs.yaml` + `credentials/.baremetal-init.yaml` | SDI 가상화 + K8s 프로비저닝 | `scalex sdi init` -> `cluster init` |
| **GitOps** | `config/k8s-clusters.yaml` + `gitops/` YAMLs | 멀티-클러스터 형상 관리 | `scalex bootstrap` -> ArgoCD |

새 클러스터 추가:
1. `sdi-specs.yaml`에 pool 추가 (Infrastructure layer)
2. `k8s-clusters.yaml`에 cluster 정의 (Infrastructure layer)
3. `gitops/generators/`에 generator 추가 (GitOps layer)
4. 완료 -- ArgoCD가 자동 배포
