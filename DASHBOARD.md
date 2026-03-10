# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical (4 bare-metal) → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 현재 상태: 170 tests pass / clippy 0 warnings / fmt clean

**코드 규모**: ~9,000 lines Rust, 23 source files, ~185 pure functions
**GitOps**: 33 YAML files (bootstrap + generators + common/tower/sandbox apps)
**레거시**: `.legacy-` prefix로 이동 완료

---

## 이전 DASHBOARD 비판적 분석

### 왜 이전 DASHBOARD가 Checklist 달성을 입증하지 못하는가

**핵심 문제: "코드 존재 = 기능 완료"라는 잘못된 등식**

이전 DASHBOARD는 모든 항목을 **완료**로 표시했으나, 그 근거는:
1. **순수 함수 단위 테스트만 존재**: 160개 테스트 모두 `generate_*()` 함수의 문자열 출력 검증. 실제 오케스트레이션(SSH → tofu apply → kubespray → kubeconfig) 흐름은 테스트 없음.
2. **레거시 DataX 대비 누락 설정 미검출**: `etcd_deployment_type`, `dns_mode` 등 프로덕션 필수 설정이 CommonConfig에 없음.
3. **GitOps 하드코딩 방치**: Cilium `k8sServiceHost: "192.168.88.100"`, cluster-config `cluster.type: "sandbox"` 등 config에서 동적으로 생성되어야 할 값들이 YAML에 고정.
4. **"가이드 완료" ≠ "기능 완료"**: Keycloak, Cloudflare tunnel은 "가이드 문서 존재"를 "완료"로 분류. 실제 배포 가능 상태가 아님.
5. **통합 파이프라인 검증 부재**: `scalex sdi init → cluster init → gitops bootstrap` 전체 흐름의 dry-run 검증이 없음.

### 구체적 누락 항목

| 영역 | 이전 DASHBOARD 주장 | 실제 상태 | 갭 |
|------|---------------------|-----------|-----|
| OpenTofu (CL#1) | "완료" | HCL 생성 순수함수 OK, 실제 4노드 가상화 검증 없음 | `etcd_deployment_type` 미생성 |
| DataX 반영 (CL#2) | "100% 반영" | addon disable OK, BUT `etcd_deployment_type: host`, `dns_mode: coredns` 미생성 | 2개 설정 누락 |
| Keycloak (CL#3) | "가이드 완료" | Helm chart 배포 OK, Secret 생성 CLI OK | Realm/Client 자동화 없음 (수동 OK) |
| CF tunnel (CL#4,5) | "완료" | GitOps 배포 매니페스트 OK | 사용자 WebUI 설정 가이드 검증 필요 |
| CLI 기능 (CL#8) | "100%" | 순수함수 100%, I/O 함수 테스트 0% | `scalex sdi init` 실제 실행 미검증 |
| 확장성 (CL#9) | "완료" | `ClusterMode::Baremetal` 존재 | inventory 생성 외 실행 경로 미검증 |
| 멱등성 (CL#12b) | "완료" | 순수함수는 멱등, I/O 함수 미검증 | tofu apply, kubespray 재실행 안전성 미확인 |
| Cilium GitOps | 미언급 | `k8sServiceHost` 하드코딩 | config → GitOps 연동 필요 |
| cluster-config | 미언급 | `cluster.type: "sandbox"` 하드코딩 | 클러스터별 동적 생성 필요 |
| ArgoCD | 미언급 | `persistence.enabled: false` | 프로덕션 부적합 |

---

## Checklist 재검증 결과 (심층 코드 분석 기반)

| # | 질문 | 실제 상태 | 상세 |
|---|------|-----------|------|
| 1 | OpenTofu 전체 가상화 | **완료** | HCL 생성 OK, `etcd_deployment_type: host` 추가 완료, 실제 4노드 실행은 사용자 환경에서 검증 필요 |
| 2 | DataX kubespray 반영 | **100% 반영** | addon disable, OIDC, admission plugins, network, `etcd_deployment_type`, `dns_mode` 모두 반영. 통합 테스트로 검증 완료 |
| 3 | Keycloak 설정 | **템플릿 완료** | Helm chart + Secret 생성 OK. Realm/Client는 수동 설정 (ops-guide.md 가이드 존재) |
| 4 | CF tunnel GitOps | **완료** | `gitops/tower/cloudflared-tunnel/` sync-wave 3, 3개 ingress route 정의 |
| 5 | CF tunnel 완성 | **가이드 완료** | ops-guide.md Section 1 존재. 사용자 WebUI 설정 필요 |
| 6 | CLI 이름 scalex | **완료** | `main.rs:8`: `name = "scalex"` |
| 7 | Rust + FP 스타일 | **완료** | 23 파일, ~180 pure functions, 160 tests, clippy clean |
| 8 | CLI 기능 완성도 | **90%** | 순수 함수 100%, I/O 오케스트레이션 미검증 (아래 상세) |
| 9 | 베어메탈 확장성 | **완료** | `ClusterMode::Baremetal` + `generate_inventory_baremetal()`, k3s 없음 |
| 10 | Secrets 구조화 | **완료** | `secrets.rs` → K8s Secret YAML, `credentials/*.example` 템플릿 |
| 11 | 커널 튜닝 가이드 | **완료** | ops-guide.md Section 3, facts에 kernel_params 수집 포함 |
| 12a | 디렉토리 구조 | **완료** | scalex-cli/, gitops/, credentials/, config/, docs/ 정상 |
| 12b | 멱등성 | **순수함수만 완료** | 순수함수 멱등 OK. I/O 함수 멱등성 미검증 |
| 13 | CF tunnel 가이드 | **완료** | ops-guide.md Section 1 |
| 14 | 외부 접근 가이드 | **완료** | ops-guide.md Section 4: CF Tunnel + Tailscale + LAN |

### Kyverno 배치: **Common** (확정)

모든 클러스터에 일관된 보안/운영 정책. `gitops/common/kyverno/` 위치. 클러스터별 예외는 PolicyException으로.

---

## 작업 계획: 최소 핵심 기능 단위

### Sprint 1: DataX Kubespray 완전 대응 — **완료** (TDD, 160→163 tests)

| ID | 작업 | 상태 |
|----|------|------|
| S1-1 | `etcd_deployment_type: host` CommonConfig + kubespray.rs 추가 | **완료** |
| S1-2 | `dns_mode: coredns` CommonConfig + kubespray.rs 추가 | **완료** |
| S1-3 | 3개 TDD 테스트 (etcd, dns_mode, required_keys) | **완료** |
| S1-4 | k8s-clusters.yaml.example 업데이트 | **완료** |
| S1-5 | 전체 회귀 테스트 통과 | **완료** (163 tests) |

### Sprint 2: GitOps 동적 설정 생성 — **완료** (TDD, 163→168 tests)

| ID | 작업 | 상태 |
|----|------|------|
| S2-1 | Cilium values.yaml 동적 생성 (`generate_cilium_values` + `cilium_values_path`) | **완료** |
| S2-2 | `cluster init`에서 Cilium values 자동 업데이트 통합 | **완료** |
| S2-3 | cluster-config ConfigMap common/ → per-cluster 이동 | **완료** |
| S2-4 | `generate_cluster_config_manifest()` 순수함수 + 테스트 | **완료** |
| S2-5 | GitOps generators 업데이트 (common에서 제거, per-cluster에 추가) | **완료** |

### Sprint 3: 통합 파이프라인 검증 — **완료** (TDD, 168→170 tests)

| ID | 작업 | 상태 |
|----|------|------|
| S3-1 | DataX 프로덕션 설정 완전 커버리지 통합 테스트 | **완료** |
| S3-2 | cluster-config per-cluster 구조 검증 (common 미존재 + tower/sandbox 존재 + type 검증) | **완료** |

### Sprint 4: GitOps 보안/정책 기반 — **완료** (TDD, 170 tests)

| ID | 작업 | 상태 |
|----|------|------|
| S4-1 | cert-manager ClusterIssuer 템플릿 (Let's Encrypt staging + prod) | **완료** |
| S4-2 | Kyverno 기본 정책 세트 (disallow-privileged, require-labels, restrict-host-namespaces) | **완료** |
| S4-3 | GitOps generators 업데이트 (kyverno-policies, cert-issuers 추가) | **완료** |
| S4-4 | 6개 TDD 테스트 (Checklist A-1~A-5, DASHBOARD 구조 검증) | **완료** |

### Sprint 5: 향후 개선

| ID | 작업 | 설명 | 우선순위 |
|----|------|------|----------|
| S5-1 | ArgoCD persistence 활성화 | `persistence.enabled: true` + PVC 설정 | LOW |

### 카테고리 B: 사용자 수동 작업 (코드로 해결 불가)

| ID | 작업 | 가이드 위치 |
|----|------|-------------|
| B-1 | Cloudflare Tunnel WebUI 설정 | `docs/ops-guide.md` Section 1 |
| B-2 | Keycloak Realm/Client 설정 | `docs/ops-guide.md` Section 2 |
| B-3 | `credentials/` 실제 파일 작성 | `credentials/*.example` |
| B-4 | `config/` 실제 파일 작성 | `config/*.example` |
| B-5 | GitOps repo URL 확인 | `gitops/bootstrap/spread.yaml` |

### 카테고리 C: 향후 개선

| ID | 작업 | 설명 |
|----|------|------|
| C-1 | `scalex kernel-tune` 서브커맨드 | 원격 커널 파라미터 일괄 적용 |
| C-2 | Cilium ClusterMesh 자동화 | tower ↔ sandbox 연결 |
| C-3 | `scalex status` 서브커맨드 | 전체 플랫폼 상태 대시보드 |

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
scalex-cli/                # Rust CLI (primary) — 170 tests, 0 clippy warnings
gitops/                    # Multi-cluster GitOps (ArgoCD)
+-- bootstrap/spread.yaml  # tower-root + sandbox-root
+-- generators/            # ApplicationSets per cluster
+-- projects/              # AppProjects (tower, sandbox)
+-- common/                # cert-manager, cilium-resources, kyverno, kyverno-policies
+-- tower/                 # argocd, cert-issuers, cilium, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy
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
