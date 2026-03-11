# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## 이전 DASHBOARD 비판적 분석

이전 DASHBOARD는 모든 15개 항목을 `CODE-COMPLETE`로 표기했으나, 이는 **근본적으로 오해의 소지가 있다.**

### 왜 "CODE-COMPLETE"가 틀린 표현인가

1. **388개 테스트는 전부 순수 함수(pure function) 단위 테스트다.** HCL 생성, inventory 파싱, YAML 검증 등 데이터 변환 로직만 검증하며, 실제 SSH 연결, `tofu apply`, Kubespray 실행, kubectl 접근을 전혀 테스트하지 않는다.

2. **"코드가 존재한다" ≠ "기능이 동작한다."** `scalex sdi init`은 HCL을 생성하고 `tofu apply`를 호출하는 코드가 있지만, 실제 물리 노드에서 libvirt VM이 정상 생성되는지는 **한 번도 검증되지 않았다.**

3. **CF Tunnel을 통한 외부 kubectl은 구조적으로 불가능하다 (Keycloak 미설정 상태에서).** CF Tunnel은 TLS를 종단(terminate)하므로 클라이언트 인증서 인증이 전달되지 않는다. OIDC 토큰 인증만 통과 가능하며, Keycloak Realm/Client가 설정되지 않은 현재 상태에서는 CF Tunnel 경유 kubectl은 **작동하지 않는다.** 이를 CODE-COMPLETE로 표기하는 것은 오류다.

4. **Checklist 5-7을 하나로 묶어 개별 격차를 은폐했다.** 문서 존재 여부와 "초기화된 베어메탈에서 scalex get clusters까지 실행 가능한가"는 완전히 다른 검증 수준이다.

5. **NEEDS-INFRA라는 면책 조항으로 검증 책임을 회피했다.** 실환경 없이도 검증 가능한 로직 갭들(에러 핸들링 경로, 설정 파일 포맷 정합성 등)이 존재하지만 이를 일괄적으로 "인프라 필요"로 분류했다.

---

## 현재 상태 (정직한 평가)

| 상태 | 의미 |
|------|------|
| ✅ VERIFIED | 테스트로 검증 완료 (코드 + 테스트 통과) |
| 🔶 CODE-EXISTS | 코드는 존재하지만 실환경 미검증 |
| 🔶 PARTIAL | 일부만 구현/검증 |
| ❌ NOT-VERIFIED | 검증되지 않음 |
| ⬜ NEEDS-USER | 사용자 수동 작업 필요 |

---

## Checklist 상세 검증 (15개 항목)

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

**상태: ✅ VERIFIED (코드 로직) / 🔶 CODE-EXISTS (실행)**

| 구성 요소 | 검증 수준 |
|-----------|----------|
| `sdi-specs.yaml` 파싱/검증 | ✅ 테스트 통과 (모델 파싱, pool 검증, IP 중복, 리소스 0 체크) |
| HCL 생성 (`generate_tofu_main`) | ✅ 테스트 통과 (provider, VM 정의, ssh_user, VFIO, single-node) |
| Host-infra HCL 생성 | ✅ 테스트 통과 (단일/다중 노드, 멱등성) |
| 리소스 풀 요약 생성 | ✅ 테스트 통과 (집계, 테이블 포맷, disk_gb) |
| KVM/bridge/VFIO 설치 스크립트 | ✅ 테스트 통과 (스크립트 생성은 순수 함수) |
| `tofu init/apply` 실제 실행 | 🔶 코드 존재, 미검증 |
| VM이 실제로 생성/접근 가능한지 | ❌ 미검증 |

**근본 원인**: 순수 함수(HCL 생성)와 I/O 함수(`tofu apply` 실행)가 분리되어 있어 순수 함수만 테스트됨. 이는 올바른 설계이지만, "SDI 가상화가 작동한다"고 주장하기 위해서는 실환경 검증이 필수.

---

### #2. CF Tunnel GitOps 배포

**상태: ✅ VERIFIED**

- `gitops/tower/cloudflared-tunnel/kustomization.yaml` — Helm chart 정의 확인 (community-charts, v2.1.2)
- `gitops/tower/cloudflared-tunnel/values.yaml` — tunnel name `playbox-admin-static`, ingress 규칙 3개 확인
- `gitops/generators/tower/tower-generator.yaml` — sync wave 3에 cloudflared-tunnel 포함 확인
- **결론**: ArgoCD가 spread.yaml 적용 시 CF Tunnel을 자동 배포하는 GitOps 구조 ✅

---

### #3. CF Tunnel 완성도 + 사용자 수동 작업

**상태: 🔶 PARTIAL**

**자동 처리되는 부분:**
- Helm chart 배포 (ArgoCD)
- K8s Secret 생성 (`scalex secrets apply`)
- Ingress 규칙 (api.k8s, auth, cd)

**사용자가 직접 수행해야 하는 작업:**
1. ⬜ Cloudflare Dashboard에서 tunnel `playbox-admin-static` 생성
2. ⬜ Credentials JSON 다운로드 → `credentials/cloudflare-tunnel.json` 저장
3. ⬜ Public Hostname 3개 설정 (cd.jinwang.dev, auth.jinwang.dev, api.k8s.jinwang.dev)
4. ⬜ DNS CNAME 자동 생성 확인

**문서화 상태**: `docs/ops-guide.md`에 상세 가이드 존재 ✅

---

### #4. CLI Rust 구현 + FP 원칙

**상태: ✅ VERIFIED**

| 항목 | 상태 |
|------|------|
| Rust 구현 | ✅ `scalex-cli/` — Cargo 프로젝트, 27개 .rs 파일 |
| clap derive CLI | ✅ 8개 서브커맨드 (facts, sdi, cluster, get, secrets, bootstrap, status, kernel-tune) |
| 순수 함수 분리 | ✅ 생성 함수(`generate_*`)는 I/O 없음, 실행 함수(`run_*`)와 분리 |
| thiserror 에러 | ✅ `core/error.rs`에 ScalexError 정의 |
| 408 tests, 0 clippy warnings | ✅ 전부 통과 |
| cargo fmt | ✅ 통과 |

---

### #5. 사용자 친절한 가이드

**상태: ✅ VERIFIED**

- README.md: Installation Guide (Step 0~8), Quick Reference, CLI Reference, Troubleshooting 테이블
- docs/ops-guide.md: Cloudflare Tunnel + Keycloak 설정 가이드
- docs/SETUP-GUIDE.md: 상세 프로비저닝 워크스루
- docs/TROUBLESHOOTING.md: 문제별 원인/해결 테이블
- CLI: `--help`, `--dry-run` 모든 커맨드 지원

---

### #6. README.md 상세 내용 포함

**상태: ✅ VERIFIED**

README.md에 포함된 섹션: Architecture Overview, Design Philosophy (7개 원칙), Installation Guide, Quick Reference, CLI Reference, GitOps Pattern (sync waves, app 추가 방법), Project Structure, Testing, Documentation 링크 테이블

---

### #7. README Installation Guide → 초기화된 베어메탈에서 `scalex get clusters`까지

**상태: 🔶 PARTIAL**

- README의 Installation Guide (Step 0~8)는 논리적으로 완전한 흐름을 제공 ✅
- 각 단계별 실패 시 대응 가이드 포함 ✅
- **그러나**: 실제 초기화된 베어메탈에서 이 가이드를 따라 끝까지 실행한 적이 없음 ❌
- Step 4 (SDI) → Step 5 (Kubespray) → Step 7 (Bootstrap) 경로가 실환경에서 작동하는지 미검증

---

### #8. CLI 기능 전체

**상태별 검증:**

| 명령어 | 코드 | 순수 함수 테스트 | 실행 로직 | 실환경 검증 |
|--------|------|:---------------:|:---------:|:----------:|
| `scalex facts --all` | ✅ | ✅ (스크립트 생성, 파싱) | ✅ (SSH 실행) | ❌ |
| `scalex sdi init` (no spec) | ✅ | ✅ (host-infra HCL) | ✅ (tofu apply) | ❌ |
| `scalex sdi init <spec>` | ✅ | ✅ (main.tf, VFIO, pool state) | ✅ (tofu apply) | ❌ |
| `scalex sdi clean --hard` | ✅ | ✅ (plan 로직) | ✅ (tofu destroy + SSH cleanup) | ❌ |
| `scalex sdi sync` | ✅ | ✅ (diff, VM conflict) | ✅ (동기화 실행) | ❌ |
| `scalex cluster init` | ✅ | ✅ (inventory, vars, OIDC) | ✅ (kubespray + kubeconfig) | ❌ |
| `scalex get baremetals` | ✅ | ✅ | N/A (읽기 전용) | ❌ |
| `scalex get sdi-pools` | ✅ | ✅ | N/A | ❌ |
| `scalex get clusters` | ✅ | ✅ | N/A | ❌ |
| `scalex get config-files` | ✅ | ✅ | N/A | ❌ |
| `scalex secrets apply` | ✅ | ✅ | ✅ | ❌ |
| `scalex bootstrap` | ✅ | ✅ (helm/argocd/kubectl args) | ✅ (3-phase) | ❌ |
| `scalex status` | ✅ | ✅ (21 tests) | N/A | ❌ |
| `scalex kernel-tune` | ✅ | ✅ (14 tests) | N/A | ❌ |

**`./credentials/.baremetal-init.yaml` 포맷 정합성:**
- Checklist 스펙: `sshKeyPathOfReachableNode` (case 3 key auth) ✅
- 3가지 접근 방식 지원: direct, external IP, ProxyJump ✅
- `.env` 변수 참조 방식 ✅

---

### #9. Baremetal 확장성 (SDI 없이 직접 사용)

**상태: ✅ VERIFIED (코드 수준)**

- `k8s-clusters.yaml`에 `cluster_mode: "baremetal"` 옵션 존재 ✅
- `generate_inventory_baremetal()` 함수 구현 + 테스트 ✅
- `test_baremetal_mode_e2e_pipeline` — baremetal 모드 E2E 파이프라인 테스트 ✅
- `test_edge_mixed_mode_sdi_and_baremetal_coexistence` — SDI/baremetal 혼합 모드 ✅
- k3s 참조: README/소스에 없음 ✅ (drawio 다이어그램에만 잔존 — 기능 영향 없음)

---

### #10. 시크릿 템플릿화

**상태: ✅ VERIFIED**

- `credentials/*.example` 파일 존재: `.baremetal-init.yaml.example`, `.env.example`, `secrets.yaml.example`, `cloudflare-tunnel.json.example`
- `credentials/` 디렉토리 `.gitignore`에 포함 ✅
- `scalex secrets apply`로 K8s Secret 자동 생성 ✅
- 테스트: management/workload 클러스터별 시크릿 생성 검증 (12 tests) ✅

---

### #11. 커널 파라미터 튜닝

**상태: ✅ VERIFIED (코드 수준)**

- `scalex kernel-tune` 커맨드 구현 ✅
- 역할별 파라미터 추천 (base, control-plane, worker, management) ✅
- `diff` 기능 (현재 값 vs 추천 값 비교) ✅
- Ansible task YAML 생성 ✅
- sysctl.conf 파일 생성 ✅
- 14개 테스트 통과 ✅
- 가이드: `docs/ops-guide.md`에 커널 튜닝 섹션 존재 ✅

---

### #12. 디렉토리 구조

**상태: ✅ VERIFIED**

```
scalex-cli/           ✅ Rust CLI (408 tests)
gitops/               ✅ ArgoCD multi-cluster
  bootstrap/          ✅ spread.yaml
  generators/         ✅ tower/ + sandbox/
  projects/           ✅ tower-project + sandbox-project
  common/             ✅ cert-manager, cilium-resources, kyverno, kyverno-policies
  tower/              ✅ argocd, cert-issuers, cilium, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy
  sandbox/            ✅ cilium, cluster-config, local-path-provisioner, rbac, test-resources
credentials/          ✅ .example 템플릿
config/               ✅ sdi-specs, k8s-clusters 예제
docs/                 ✅ 7개 문서
ansible/              ✅ node preparation
kubespray/            ✅ submodule v2.30.0
client/               ✅ OIDC kubeconfig
tests/                ✅ run-tests.sh
```

불필요 파일 확인: `test_checklist_no_unnecessary_root_files` 테스트 통과 ✅

---

### #13. 멱등성

**상태: ✅ VERIFIED (코드 수준)**

- `test_checklist_tofu_hcl_generation_idempotent` ✅
- `test_checklist_kubespray_inventory_idempotent` ✅
- `test_checklist_cluster_vars_idempotent` ✅
- `test_e2e_clean_rebuild_idempotency` ✅
- `test_generate_tofu_host_infra_idempotent` ✅
- Helm: `upgrade --install` (idempotent) ✅
- Kubespray: 재실행 안전 (Ansible 특성) ✅

**실환경 멱등성** (sdi init → clean → sdi init 사이클): ❌ 미검증

---

### #14. 외부 kubectl 접근 (CF Tunnel/Tailscale/SOCKS5)

**상태: 🔶 PARTIAL — 구조적 제약 존재**

| 접근 방법 | 작동 조건 | 현재 상태 |
|-----------|----------|----------|
| **Tailscale** | Tailscale 설치 + kubeconfig의 server IP가 Tailscale IP | ⬜ 실환경 필요 |
| **CF Tunnel + OIDC** | Keycloak Realm/Client 설정 완료 | ❌ Keycloak 미설정 |
| **CF Tunnel + client cert** | **불가능** — CF가 TLS 종단하므로 client cert 전달 안됨 | ❌ 구조적 불가 |
| **SOCKS5 Proxy** | Tower에 SOCKS5 pod + `kubectl --proxy-url` | 🔶 manifest 존재, 미검증 |
| **LAN 직접** | 동일 네트워크 + kubeconfig | ⬜ 실환경 필요 |

**핵심 문제**: Keycloak 없이는 CF Tunnel 경유 외부 kubectl이 불가능. Pre-OIDC 외부 접근은 **Tailscale 직접 접근**만 가능하며, SOCKS5 proxy는 LAN/Tailscale 내부에서의 편의 기능.

**문서화 상태**: `docs/ops-guide.md`에 접근 경로별 가이드 존재 ✅, README에도 External Access 섹션 ✅

---

### #15. NAT 접근 경로

**상태: ✅ VERIFIED (문서 수준)**

- NAT 내부 멀티-클러스터: ✅ 구조 확인
- 외부 접근: Tailscale + CF Tunnel만 ✅
- LAN 내부: 스위치 접근 가이드 — `docs/ops-guide.md` 및 README External Access에 문서화 ✅
- `test_checklist_nat_access_methods_documented` 테스트 통과 ✅

---

## 근본 원인 분석 요약

| 구분 | 원인 | 영향 범위 |
|------|------|----------|
| **테스트 한계** | 388개 테스트가 모두 순수 함수 테스트 → 실행 경로(I/O) 미검증 | 모든 실행 커맨드 |
| **CF Tunnel 인증 갭** | TLS 종단으로 client cert 불가, OIDC만 가능 → Keycloak 필수 | Checklist #14 |
| **실환경 미검증** | 물리 인프라 없이 개발 → E2E 검증 불가 | Checklist #1, #7, #13 |
| **상태 표기 오해** | CODE-COMPLETE ≈ "done" 오해 유발 | 전체 DASHBOARD 신뢰도 |

---

## 실행 계획 (Sprint 기반)

### Sprint 18: 코드 수준 갭 해소 ✅ DONE (396 → 396 tests)

- [x] 18a: CLI 실행 경로 pipeline ordering 테스트 (`sdi init`, `cluster init`)
- [x] 18b: CF Tunnel 인증 경로 문서 검증 테스트 (OIDC 필수, Tailscale pre-OIDC)
- [x] 18c: `.baremetal-init.yaml` 3가지 접근 방식 + 2가지 인증 방식 테스트

### Sprint 19: 구조 검증 + 확장성 테스트 ✅ DONE (396 → 408 tests)

- [x] 19a: SOCKS5 proxy manifest 구조 검증 (Deployment+Service, port 1080, ClusterIP, resource limits)
- [x] 19a: Tower generator에 socks5-proxy 등록 확인 (kube-tunnel ns, sync wave 3)
- [x] 19a: 외부 접근 3경로 문서화 검증 (LAN, Tailscale, CF Tunnel)
- [x] 19b: GitOps 디렉토리 구조 검증 (bootstrap, generators, common, tower, sandbox)
- [x] 19b: Rust 프로젝트 구조 + credentials 템플릿 존재 확인
- [x] 19c: 2-Layer 템플릿 관리 검증 (Layer1: sdi-specs+k8s-clusters / Layer2: ApplicationSets)
- [x] 19d: 단일 노드 SDI 풀 + 단일 클러스터 K8s config 파싱 검증

### Sprint 20: 실환경 E2E 검증 — SDI + Kubespray (⬜ 인프라 필요)

#### 20a — SDI 가상화 E2E
- [ ] `scalex facts --all` → 4노드 SSH 접근 + JSON 수집
- [ ] `scalex sdi init config/sdi-specs.yaml` → VM 5개 생성 (tower-cp-0, sandbox-cp-0, sandbox-w-0~2)
- [ ] `scalex get sdi-pools` → 2개 풀 확인
- [ ] `scalex sdi clean --hard --yes-i-really-want-to` → 완전 초기화
- [ ] 재실행 (멱등성 검증)

#### 20b — Kubespray 클러스터링 E2E
- [ ] `scalex cluster init config/k8s-clusters.yaml` → tower + sandbox 클러스터 생성
- [ ] `kubectl get nodes` (tower kubeconfig) → 정상 응답
- [ ] `kubectl get nodes` (sandbox kubeconfig) → 정상 응답
- [ ] `scalex get clusters` → 2개 클러스터 표시

#### 20c — ArgoCD Bootstrap E2E
- [ ] `scalex secrets apply` → K8s secrets 생성
- [ ] `scalex bootstrap` → ArgoCD 설치 + sandbox 등록 + spread.yaml 적용
- [ ] `kubectl -n argocd get applications` → 모든 앱 Synced/Healthy
- [ ] CF Tunnel 상태 확인 (cloudflared pod Running)

#### 20d — 외부 접근 E2E
- [ ] Tailscale IP로 tower kubectl 접근
- [ ] Keycloak Realm/Client 설정
- [ ] CF Tunnel 경유 OIDC kubectl 접근
- [ ] `https://cd.jinwang.dev` ArgoCD UI 접근
- [ ] `https://auth.jinwang.dev` Keycloak UI 접근

### Sprint 21: 단일 노드 모드 검증 (⬜ 인프라 필요)

- [ ] 단일 베어메탈에서 tower + sandbox 양쪽 모두 구동
- [ ] `sdi-specs.yaml`에서 모든 VM을 1개 호스트에 배치
- [ ] 리소스 제약 하에서 정상 작동 확인

### Sprint 22: 3rd 클러스터 확장 검증 (⬜ 인프라 필요)

- [ ] `sdi-specs.yaml`에 3번째 풀 추가
- [ ] `k8s-clusters.yaml`에 3번째 클러스터 정의
- [ ] `gitops/generators/` 에 3번째 generator 추가
- [ ] E2E: 3-클러스터 환경 작동 확인

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
|  facts → sdi init → cluster init         |
|  → secrets apply → bootstrap             |
|  get baremetals/sdi-pools/clusters        |
|  status / kernel-tune                     |
+-------------------------------------------+
        |
        v
_generated/
├── facts/          (hardware JSON per node)
├── sdi/            (OpenTofu HCL + state + resource-pool.json)
│   ├── host-infra/ (no-flag: host-level libvirt infra)
│   └── main.tf     (spec: VM pool HCL)
└── clusters/       (inventory.ini + vars + kubeconfig per cluster)
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
|    → projects/ (AppProjects)              |
|    → generators/{tower,sandbox}/          |
|    → common/ tower/ sandbox/              |
+-------------------------------------------+
```

### External Access Paths

```
[CF Tunnel + OIDC] (Keycloak 설정 완료 후에만 kubectl 가능)
  kubectl (OIDC token) → CF Edge → cloudflared → kube-apiserver
  ⚠ client certificate auth는 CF Tunnel 통과 불가 (TLS 종단)
  ⚠ Keycloak 미설정 시 외부 kubectl 불가

[Tailscale] (cert + token — Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) → Tailscale IP → kube-apiserver
  SSH → Tailscale IP → bastion → 내부 노드

[SOCKS5 Proxy] (LAN/Tailscale 내부에서의 편의 접근)
  kubectl --proxy-url socks5://tower-ip:1080 → kube-apiserver

[LAN] (모든 인증 방식)
  kubectl → LAN IP → kube-apiserver
  SSH → LAN IP → 노드
```

---

## Test Summary

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 86+ | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave, AppProject, sdi-init, E2E pipeline, SSH, 3rd cluster, GitOps consistency, spec caching, CF Tunnel auth, Tower SAN, CF credentials, SOCKS5 manifest, directory structure, 2-layer template, single-node mode |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generators |
| core/kubespray | 32+ | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node, Tower SAN |
| commands/status | 21 | platform status reporting |
| commands/sdi | 24 | network resolve, host infra, pool state, clean validation, CIDR prefix, host-infra clean |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14+ | ArgoCD helm, cluster add, kubectl apply, pipeline, CF credentials check |
| core/sync | 13 | sync diff, VM conflict, add+remove |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node, host-infra |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table, disk_gb |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| commands/facts | 4 | facts gathering |
| **TOTAL** | **408** | **순수 함수 테스트만 — 실행 경로 미포함** |
