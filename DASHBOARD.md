# ScaleX Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

## Phase 0: Foundation

- [x] **0-1** credentials/ 디렉토리 생성 (`.gitignore`, `README.md`, `secrets.yaml.example`, `.baremetal-init.yaml.example`, `.env.example`)
- [x] **0-2** values.yaml 비밀 정보 분리 → `credentials/secrets.yaml` fallback 구조
- [x] **0-3** config/ 스키마 정의 (`baremetal.yaml.example`, `sdi-specs.yaml.example`, `k8s-clusters.yaml.example`)
- [x] **0-4** DASHBOARD.md 작성

## Phase 1: Rust CLI (`scalex`) + `facts`

- [x] **1-1** `scalex-cli/` Cargo 프로젝트 초기화 (clap, serde, thiserror)
- [x] **1-2** credentials 파서 (`.baremetal-init.yaml` + `.env` 치환, SSH 접근 모델 3종)
- [x] **1-3** `scalex facts` 구현 (SSH → 하드웨어 정보 수집 → JSON 저장)
- [x] **1-4** `scalex get baremetals` 구현 (facts JSON → 테이블 출력)
- [x] **1-5** CI: `cargo test` + `cargo clippy` + `cargo fmt --check` — 26 tests, 0 warnings

## Phase 2: SDI Layer (OpenTofu 리소스 풀)

- [x] **2-1** 모든 노드 KVM/libvirt 설치 자동화 (host_prepare.rs: generate_kvm_install_script)
- [x] **2-2** br0 브릿지 전 노드 확장 (host_prepare.rs: generate_bridge_setup_script + netplan try)
- [x] **2-3** VFIO-PCI 모듈 + 커널 파라미터 (host_prepare.rs: generate_vfio_setup_script)
- [x] **2-4** OpenTofu 멀티호스트 코드 생성기 (tofu.rs: generate_tofu_main)
- [x] **2-5** `scalex sdi init` / `scalex sdi init <spec>` / `scalex sdi clean`
- [x] **2-6** cloud-init 범용 템플릿 (tofu.rs: generate_cloudinit_data, k3s 제거)
- [x] **2-7** `scalex get sdi-pools` (sdi-state.json → 테이블 출력)

## Phase 3: Multi-Cluster Kubespray

- [x] **3-1** `k8s-clusters.yaml` 파서 + 검증기 (models/cluster.rs)
- [x] **3-2** kubespray inventory 자동 생성 (kubespray.rs: generate_inventory)
- [x] **3-3** cluster-vars 생성 — 공통 + 클러스터별 (kubespray.rs: generate_cluster_vars)
- [x] **3-4** `scalex cluster init` (inventory + vars 생성 → kubespray 실행 → kubeconfig 수집)
- [x] **3-5** kubeconfig 수집 + 멀티 클러스터 컨텍스트 (scp from control-plane)
- [x] **3-6** `scalex get clusters` (inventory 기반 테이블 출력)
- [x] **3-7** ArgoCD에 Sandbox 원격 클러스터 등록 (generators + projects 생성, server URL 교체 필요)

## Phase 4: GitOps 재구조화

- [x] **4-1** `gitops/common/` (cilium, cilium-resources, cert-manager, cluster-config, kyverno)
- [x] **4-2** `gitops/tower/` (argocd, keycloak, cloudflared-tunnel, socks5-proxy)
- [x] **4-3** `gitops/sandbox/` (local-path-provisioner, rbac, test-resources)
- [x] **4-4** `multi-cluster-spread.yaml` 멀티클러스터 부트스트랩 (tower-root + sandbox-root)
- [x] **4-5** ApplicationSet 분리 (common/tower/sandbox generators per cluster)
- [x] **4-6** ArgoCD multi-cluster 설정 (projects + generators, sandbox server URL 교체 필요)

## Phase 5-6: Advanced

- [x] **5-1** `scalex sdi sync` (베어메탈 추가/삭제 정합: diff 계산, VM 사이드이펙트 감지, 자동 facts 수집 + 호스트 준비)
- [x] **5-2** `scalex get config-files` (YAML 유효성 검증 + 파일 존재 확인)
- [x] **5-3** 커널 파라미터 튜닝 가이드 (`docs/ops-guide.md` Section 3)
- [x] **5-4** Cloudflare Tunnel 상세 가이드 (`docs/ops-guide.md` Section 1)
- [x] **5-5** LAN 내부 접근 가이드 (`docs/ops-guide.md` Section 4)
- [x] **5-6** Keycloak 완성 가이드 — redirect URI, 그룹 할당 (`docs/ops-guide.md` Section 2)
- [x] **6-1** 베어메탈 직접 사용 모드 (cluster_mode: baremetal, baremetal_nodes 직접 지정, SDI 없이 kubespray 적용)

---

## Checklist 매핑

| # | 질문 | 상태 | Phase |
|---|------|------|-------|
| 1 | OpenTofu 전체 가상화 | **완료** (4노드 SDI + VM 풀) | Done |
| 2 | DataX kubespray 반영 | **완료** (보안, graceful shutdown, gateway API 추가) | Done |
| 3 | Keycloak 완성 | **가이드 완료** (사용자 WebUI 설정 필요) | docs |
| 4 | CF tunnel GitOps | **Yes** (Helm chart via ArgoCD) | Done |
| 5 | CF tunnel 완성 | **가이드 완료** (사용자 WebUI 설정 필요) | docs |
| 6 | CLI 이름 scalex | **scalex** (Rust) | Done |
| 7 | Rust CLI | **Yes** (29 tests, clippy clean, FP style) | Done |
| 8 | CLI 기능 | **facts/get/sdi(+sync)/cluster 완료** | Done |
| 9 | 베어메탈 확장성 | **완료** (cluster_mode: baremetal 지원, SDI 없이 kubespray 직접 적용) | Done |
| 10 | credentials 구조화 | **완료** (secrets.yaml fallback) | Done |
| 11 | 커널 튜닝 | **가이드 완료** (storage/network/IOMMU) | docs |
| 12 | 디렉토리 구조 | **완료** (common/tower/sandbox + scalex-cli + config + credentials) | Done |
| 13 | CF tunnel 가이드 | **완료** (6단계 WebUI 가이드) | docs |
| 14 | 외부 접근 | **완료** (CF tunnel + Tailscale + LAN 가이드) | docs |

---

## 남은 작업 우선순위

모든 Phase 완료. 사용자 수동 작업만 남음:
- Cloudflare Tunnel WebUI 설정 (`docs/ops-guide.md` Section 1)
- Keycloak Realm/Client 설정 (`docs/ops-guide.md` Section 2)
- Sandbox 클러스터 실제 server URL 교체 (`gitops/projects/sandbox-project.yaml`)
