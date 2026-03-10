# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical -> SDI (OpenTofu) -> Node Pools -> Cluster (Kubespray) -> GitOps (ArgoCD)

---

## Current Status (Sprint 9a complete)

- **Tests**: 294 pass / clippy 0 warnings / fmt clean
- **Code**: ~12,500 lines Rust, 27 source files
- **GitOps**: 41 YAML files (bootstrap + generators + common/tower/sandbox apps)
- **Docs**: 7 files (ops-guide, setup-guide, architecture, troubleshooting, etc.)

---

## Checklist Gap Analysis

> 아래 분석은 사용자의 15개 체크리스트 항목 각각에 대해 코드베이스를 실제로 검증한 결과이다.
> "이전 작업자"의 DASHBOARD는 기능 구현 여부만 표기했을 뿐, **실제 동작 검증을 단 한 번도 수행하지 않았다**.
> 이것이 개발이 중단된 근본 원인이다.

### CL-1: 4개 노드 OpenTofu 가상화 + 리소스 풀 구조

| 항목 | 상태 | 근거 |
|------|------|------|
| HCL 생성 (multi-host libvirt) | OK | `core/tofu.rs` -- `generate_tofu_main()`, `generate_tofu_host_infra()` |
| 4개 호스트 provider | OK | `generate_provider_block()` per unique host |
| VM 리소스 생성 | OK | `generate_vm_resource()` -- disk, cloudinit, domain |
| SSH URI에 adminUser 사용 | **FIXED (9a)** | `tofu.rs` -- `ssh_user` 파라미터로 변경. 2개 테스트 추가 |
| 단일 노드 환경 | UNTESTED | 코드는 지원하지만 테스트 없음 |
| 실제 `tofu apply` 실행 | NEVER | 물리 인프라에서 한 번도 실행된 적 없음 |

### CL-2: Cloudflare tunnel -- ArgoCD GitOps 방식

| 항목 | 상태 | 근거 |
|------|------|------|
| GitOps YAML 존재 | OK | `gitops/tower/cloudflared-tunnel/` -- kustomization + values.yaml |
| Helm chart 방식 | OK | `values.yaml`에 tunnelConfig + ingress 정의 |
| ApplicationSet 등록 | OK | `tower-generator.yaml` -- syncWave: "3" |

### CL-3: Cloudflare tunnel 완료 여부

| 항목 | 상태 | 근거 |
|------|------|------|
| 사용자 수동 작업 문서화 | OK | `docs/ops-guide.md` Section 1 -- 6단계 가이드 |
| credentials 템플릿 | OK | `credentials/cloudflare-tunnel.json` (gitignored) |
| `scalex secrets apply` 자동화 | OK | tunnel-credentials Secret 생성 |
| **실제 동작 검증** | **NEVER** | cloudflared Pod 기동 여부 미확인 |

### CL-4: CLI -- Rust 구현 + FP 스타일

| 항목 | 상태 | 근거 |
|------|------|------|
| Rust 구현 | OK | `scalex-cli/` -- 27 source files |
| Pure functions | OK | generators/validators는 I/O 분리됨 |
| No side effects in generators | OK | tofu, kubespray, gitops 생성기 모두 순수 함수 |
| Clippy 0 warnings | OK | `cargo clippy` 통과 |
| 일부 I/O 혼재 | WARN | `commands/sdi.rs` 내 orchestration 함수에 I/O + 로직 혼재 (허용 수준) |

### CL-5: 사용자 친절한 가이드

| 항목 | 상태 | 근거 |
|------|------|------|
| README Installation Guide | OK | Step 0~8 + 트러블슈팅 테이블 |
| Pre-flight 점검 (Step 1.5) | OK | SSH 접근 테스트 가이드 |
| 에러 메시지 | OK | `validate_baremetal_config()` -- 뉴비 친화적 메시지 |
| docs/ 가이드 | OK | 7개 문서 (ops-guide, setup, architecture, troubleshooting, contributing, cloudflare, network) |

### CL-6: README 상세 내용

| 항목 | 상태 | 근거 |
|------|------|------|
| Architecture Overview | OK | 5-Layer diagram + 2-cluster table |
| Design Philosophy (7 principles) | OK | 7개 원칙 모두 기술 |
| CLI Reference | OK | Core + Query commands 테이블 |
| GitOps Pattern | OK | Bootstrap chain + Sync waves + Adding apps |
| Project Structure | OK | 전체 디렉토리 트리 |
| **Installation Guide E2E 미검증** | **FAIL** | 가이드는 있지만 실행해본 적 없음 |

### CL-7: Installation Guide E2E 보장 (중요)

| 항목 | 상태 | 근거 |
|------|------|------|
| Step 0 (전제조건) | OK (문서만) | Rust, Ansible, OpenTofu, kubectl 설치 |
| Step 1 (CLI 빌드) | OK | `cargo build --release` |
| Step 1.5 (Pre-flight) | OK (문서만) | SSH 접근 테스트 |
| Step 2 (설정 파일) | OK | 4개 config 파일 + `.example` 템플릿 |
| Step 3 (facts) | OK (코드만) | `scalex facts --all` -- SSH 실행 |
| Step 4 (SDI init) | OK (코드만) | `scalex sdi init <spec>` -- tofu apply |
| Step 5 (cluster init) | OK (코드만) | `scalex cluster init <config>` -- kubespray |
| Step 6 (secrets) | OK (코드만) | `scalex secrets apply` |
| Step 7 (GitOps bootstrap) | OK (코드만) | `kubectl apply -f gitops/bootstrap/spread.yaml` |
| Step 8 (최종 검증) | OK (문서만) | `scalex status`, `scalex get clusters` |
| **전체 E2E 실행** | **NEVER** | 단 한 번도 처음부터 끝까지 실행된 적 없음 |

### CL-8: CLI 기능 완성도

| 기능 | 상태 | 비고 |
|------|------|------|
| `scalex facts` | OK | CPU/mem/GPU/disk/NIC/IOMMU/kernel 수집 |
| `scalex sdi init` (no flag) | OK | host_prepare 실행 (libvirt, bridge) |
| `scalex sdi init <spec>` | OK | HCL 생성 + tofu apply |
| `scalex sdi clean --hard --yes-i-really-want-to` | OK | clean logic tested (19 tests) |
| `scalex sdi sync` | OK | diff-based add/remove (13 tests) |
| `scalex cluster init <config>` | OK | inventory + vars + kubespray + kubeconfig |
| `scalex get baremetals` | OK | facts JSON -> table |
| `scalex get sdi-pools` | OK | SDI state -> table |
| `scalex get clusters` | OK | cluster dirs -> table |
| `scalex get config-files` | OK | file presence + YAML validation |
| `sdi init` facts 자동 감지/실행 | **FIXED (9a)** | `sdi.rs:121-128` -- 이미 구현되어 있었음. `dir_is_empty` 테스트 3개 추가 |
| **`sdi init` (no flag)의 "리소스 풀 관측" 의미 불명확** | **WARN** | JSON summary만 생성. 진정한 "통합 풀" 아님 |

### CL-9: 베어메탈 직접 사용 확장성

| 항목 | 상태 | 근거 |
|------|------|------|
| `ClusterMode::Baremetal` | OK | `models/cluster.rs` -- Sdi/Baremetal enum |
| `generate_inventory_baremetal()` | OK | SDI 없이 직접 노드 사용 |
| k3s 배제 | OK | Kubespray만 사용 (프로덕션 수준) |
| **베어메탈 모드 E2E 테스트** | UNTESTED | 코드만 존재 |

### CL-10: 보안 정보 관리

| 항목 | 상태 | 근거 |
|------|------|------|
| `credentials/` gitignored | OK | `.gitignore`에 포함 |
| `.example` 템플릿 | OK | 4개 파일 모두 `.example` 존재 |
| `scalex secrets apply` | OK | K8s Secret 생성 |
| secrets.yaml 템플릿 | OK | Keycloak, ArgoCD, CF 시크릿 |

### CL-11: 커널 파라미터 튜닝

| 항목 | 상태 | 근거 |
|------|------|------|
| `scalex kernel-tune` | OK | 14 tests |
| 역할별 권장값 | OK | `--role control-plane/worker` |
| Ansible 형식 출력 | OK | `--format ansible` |
| diff 기능 | OK | `--diff-node <name>` |
| 가이드 문서 | OK | `docs/ops-guide.md` Section 3 |

### CL-12: 디렉토리 구조

| 항목 | 상태 | 비고 |
|------|------|------|
| `scalex-cli/` | OK | Rust CLI |
| `gitops/common/` | OK | cert-manager, cilium-resources, kyverno, kyverno-policies |
| `gitops/tower/` | OK | argocd, cilium, cert-issuers, cloudflared-tunnel, cluster-config, keycloak, socks5-proxy |
| `gitops/sandbox/` | OK | cilium, cluster-config, local-path-provisioner, rbac, test-resources |
| 불필요한 파일 | OK | 발견되지 않음 |

### CL-13: 멱등성

| 항목 | 상태 | 근거 |
|------|------|------|
| `tofu apply` 멱등성 | CLAIMED | OpenTofu 자체가 멱등적이지만 미검증 |
| `kubespray` 멱등성 | CLAIMED | Kubespray 재실행 지원하지만 미검증 |
| `sdi clean -> rebuild` | OK (unit) | 테스트 존재하지만 실환경 미검증 |
| GitOps 멱등성 | OK | ArgoCD self-heal + prune |

### CL-14: 외부 kubectl 접근 (CF Tunnel)

| 항목 | 상태 | 근거 |
|------|------|------|
| CF Tunnel ingress: `api.k8s.jinwang.dev` | OK (config) | `values.yaml` -- `https://kubernetes.default.svc:443` |
| SOCKS5 proxy | OK (config) | `socks5-proxy/manifest.yaml` |
| Pre-OIDC kubectl 가이드 | OK (문서) | `docs/ops-guide.md` Section 4 |
| **실제 접근 검증** | **NEVER** | cloudflared Pod 기동, tunnel 연결, kubectl 응답 미확인 |

### CL-15: NAT 접근 방법

| 항목 | 상태 | 근거 |
|------|------|------|
| Tailscale 가이드 | OK | `docs/ops-guide.md` Section 4 |
| CF Tunnel 가이드 | OK | `docs/ops-guide.md` Section 4 |
| LAN 직접 접근 가이드 | OK | SSH 직접 + ProxyJump + 스위치 참고 |
| 접근 방법 비교표 | OK | 3가지 방법 비교 테이블 |

---

## Root Cause Analysis (개발 중단 원인)

### 핵심 문제: "오프라인 단위 테스트 중심 개발"의 한계

이전 개발자는 **순수 함수 단위 테스트**에 집중하여 287개 테스트를 작성했지만:

1. **통합 테스트 0건**: 컴포넌트 간 상호작용 검증 없음
   - 예: `sdi-specs.yaml`과 `k8s-clusters.yaml` 간 pool 매핑이 실제로 유효한 kubespray inventory를 생성하는지
2. **E2E 테스트 0건**: 전체 파이프라인 동작 미검증
   - `facts -> sdi init -> cluster init -> secrets -> gitops bootstrap` 흐름
3. **실환경 실행 0회**: 모든 I/O 함수가 한 번도 실제 실행된 적 없음
4. **버그 미발견**: 순수 함수 테스트만으로는 발견 불가능한 버그 존재
   - ~~`tofu.rs:172` -- SSH URI에 `root@` 하드코딩~~ → **Sprint 9a에서 수정 완료**
   - ~~`sdi init`에서 facts 미실행 시 자동 실행 미구현~~ → **이미 구현되어 있었음. 테스트 추가 완료**
5. **GitOps 정합성 미검증**: sandbox-generator의 placeholder URL (`https://sandbox-api:6443`)이 자동 교체되는 흐름 미검증
   - **Sprint 9a에서 placeholder 감지 테스트 + CF tunnel ingress 완성도 테스트 추가**

### 결론

개발은 "기능 구현"까지는 잘 되었으나, **"검증"이 완전히 누락**되었다.
TDD 원칙에서 "테스트"는 단위 테스트만을 의미하지 않는다.
사용자 관점의 E2E 검증이 없으면 "동작하는 소프트웨어"라 할 수 없다.

---

## Sprint Plan

### Sprint 9a: 코드 수준 버그 수정 + 통합 테스트 추가 (오프라인 가능)

> 물리 인프라 없이 수행 가능한 코드 품질 개선

| # | Task | 상태 | TDD 검증 |
|---|------|------|----------|
| 9a-1 | **BUG FIX**: `tofu.rs` SSH URI에 adminUser 사용 | **DONE** | RED: 2개 테스트 작성 -> 컴파일 실패 -> GREEN: ssh_user 파라미터 추가 -> 294 pass |
| 9a-2 | **BUG FIX**: `sdi init`에서 facts 미존재 시 자동실행 | **DONE** | 이미 구현됨 확인. `dir_is_empty` 테스트 3개 추가 |
| 9a-3 | **TEST**: sandbox-generator placeholder URL 감지 | **DONE** | YAML 파싱 + placeholder 감지 테스트 추가 |
| 9a-4 | **TEST**: CF Tunnel ingress 완성도 | **DONE** | 3개 hostname + catch-all 404 + noTLSVerify 검증 |
| 9a-5 | **TEST**: cross-config 정합성 | **DONE** | 이미 5+개 테스트 존재 확인 (pool ref, CIDR overlap, DNS unique, cilium ID unique) |
| 9a-6 | Commit + Push | **DONE** | 294 tests, 0 clippy warnings |

### Sprint 9b: 실환경 E2E 검증 (물리 인프라 필요)

> 사용자가 직접 실행해야 하는 단계. CLI 도구가 실제로 동작하는지 검증.

| # | Task | 상태 | 검증 방법 |
|---|------|------|----------|
| 9b-1 | `scalex facts --all` 실행 (4노드) | TODO | `_generated/facts/*.json` 4개 생성 확인 |
| 9b-2 | `scalex sdi init config/sdi-specs.yaml` 실행 | TODO | `virsh list`로 VM 확인 |
| 9b-3 | `scalex cluster init config/k8s-clusters.yaml` 실행 | TODO | `kubectl get nodes` 응답 확인 |
| 9b-4 | `scalex secrets apply` 실행 | TODO | `kubectl get secrets` 확인 |
| 9b-5 | GitOps bootstrap | TODO | ArgoCD 앱 Synced/Healthy 확인 |
| 9b-6 | 외부 kubectl 접근 (CF Tunnel) | TODO | `kubectl --kubeconfig tower-tunnel.yaml get nodes` |
| 9b-7 | `scalex sdi clean --hard` + 재구축 (멱등성) | TODO | clean -> re-init -> 동일 결과 |

### Sprint 9c: 확장성 검증

| # | Task | 상태 |
|---|------|------|
| 9c-1 | 단일 노드 SDI 환경 E2E 검증 | TODO |
| 9c-2 | 3번째 클러스터 추가 (확장성) | TODO |
| 9c-3 | Keycloak Realm GitOps 자동화 | TODO |

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
|                                           |
|  Validation Pipeline (pure functions):    |
|  validate_baremetal_config                |
|  validate_sdi_spec                        |
|  validate_cluster_sdi_pool_mapping        |
|  validate_unique_cluster_ids              |
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

---

## Test Summary

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 57 | pool mapping, cluster IDs, CIDR overlap, DNS uniqueness, legacy detection, single-node, baremetal, idempotency, E2E pipeline, clean->rebuild, cross-config, sync, 2-layer consistency, README verification |
| core/gitops | 36 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generator consistency |
| core/kubespray | 32 | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, extra vars, single-node dual-role, no-CP rejection |
| commands/status | 21 | platform status reporting |
| commands/sdi | 19 | network resolve, host infra, pool state, clean arg validation, plan_clean_operations |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| core/sync | 13 | sync diff, VM conflict detection, simultaneous add+remove, empty desired, complete replacement, multi-host removal |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM install, bridge setup, VFIO config, cleanup script validation |
| commands/cluster | 11 | cluster init, SDI/baremetal modes, gitops, ssh_user |
| core/tofu | 8 | HCL gen, IP-based SSH URI, VFIO, idempotency |
| models/* | 8 | parse/serialize sdi, cluster, baremetal |
| core/resource_pool | 5 | aggregation, table format |
| commands/facts | 4 | facts gathering, script building |
| core/ssh | 2 | SSH command building |
