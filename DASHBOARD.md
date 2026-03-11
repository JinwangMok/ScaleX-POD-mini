# ScaleX-POD-mini Development Dashboard

> 5-Layer SDI Platform: Physical → SDI (OpenTofu) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

---

## Critical Analysis (2026-03-11 Sprint 17 재평가)

### 이전 DASHBOARD의 근본적 문제

> **이전 DASHBOARD는 거의 모든 항목을 "VERIFIED"로 표시했으나, 이는 다음과 같은 이유로 부정확하다:**
>
> 1. **380개 테스트 전부 오프라인 순수 함수 테스트** — 실제 SSH/libvirt/Kubespray/ArgoCD 통합 테스트는 0건
> 2. **"VERIFIED" = "코드가 존재한다"** — 실제 동작 검증이 아니라 문자열 출력 테스트 통과를 의미
> 3. **아키텍처적 결함이 발견되지 않은 채 방치** — CF Tunnel Pre-OIDC kubectl 인증 불가능, SOCKS5 프록시 접근 불가 등
> 4. **문서 존재 = 기능 검증으로 취급** — "VERIFIED (docs)"라는 판정은 문서가 정확한지조차 검증하지 않음
> 5. **워크플로우 통합 검증 부재** — 개별 명령어 단위 테스트만 존재, `facts → sdi init → cluster init → bootstrap` E2E 파이프라인 미검증

### Sprint 17에서 발견한 새로운 Critical Gaps

| ID | 심각도 | 갭 | 상세 |
|----|--------|-----|------|
| **C-7** | **CRITICAL** | CF Tunnel Pre-OIDC kubectl 인증 불가 | CF Tunnel은 HTTP 모드로 TLS를 종단(terminate)하므로 client certificate가 kube-apiserver에 전달되지 않음. ops-guide Section 4의 "admin kubeconfig로 CF Tunnel 경유 kubectl 가능" 가이드는 **아키텍처적으로 불가능한 내용** |
| **C-8** | **HIGH** | SOCKS5 프록시 ClusterIP — 외부 미접근 | `socks5-proxy`가 ClusterIP Service로만 배포되어 외부 접근 불가. kubectl port-forward가 필요하지만 이는 chicken-and-egg 문제 발생 |
| **C-9** | **HIGH** | Tower `supplementary_addresses_in_ssl_keys` 누락 | Tower 클러스터에 Tailscale IP(`100.64.0.1`)가 API server SAN에 미포함 — Tailscale 경유 직접 kubectl 접근 시 TLS 검증 실패 가능 |
| **C-10** | **MEDIUM** | CF Tunnel credentials 사전 검증 부재 | `scalex bootstrap` 실행 시 `credentials/cloudflare-tunnel.json` 존재 여부를 사전 검증하지 않아 cloudflared Pod CrashLoop 발생 가능 |
| **C-11** | **MEDIUM** | Pre-OIDC 외부 kubectl 대안 부재 | CF Tunnel 경유 client cert 불가 → token 기반 인증 메커니즘이 구현되어 있지 않음 |

### 이전 Sprint(13~16)에서 해결된 갭

| ID | 상태 | 갭 | 해결 내역 |
|----|------|-----|----------|
| C-1 | **FIXED** | ArgoCD 부트스트랩 누락 | `scalex bootstrap` 3-phase pipeline 구현 |
| C-2 | **FIXED** | Sandbox 클러스터 ArgoCD 미등록 | `scalex bootstrap` Phase 2에서 자동 등록 |
| C-3 | **FIXED** | Kubespray 경로 해결 버그 | `kubespray/kubespray/` 서브모듈 경로 우선 |
| C-4 | **FIXED** | `sdi init` no-flag 구현 | 호스트 준비 + resource-pool + host-infra HCL + tofu apply |
| C-5 | **RESOLVED** | 외부 sandbox kubectl | 아키텍처 결정: Tower 경유 관리 |
| C-6 | **FIXED** | Legacy 네이밍 | `scalex-root` 통일 |
| G-1 | **FIXED** | `sdi clean` host-infra tofu 미파괴 | `TofuDestroyHostInfra` variant 추가 |
| G-2 | **FIXED** | `sdi init <spec>` resource-pool-summary 미생성 | 공통 경로 이동 |
| G-3 | **FIXED** | `sdi init` spec 캐싱 | `sdi-spec-cache.yaml` 자동 생성 |

---

## Checklist Status (15 Items) — Sprint 17 재평가

> **판정 기준 (엄격 적용)**:
> - **CODE-COMPLETE**: 순수 함수 테스트 통과 + 코드 로직 검토 완료 (오프라인 레벨). 실환경 미검증
> - **BUG**: 코드/아키텍처 버그 존재 — 수정 필요
> - **GAP**: 요구사항 대비 코드/기능 부재
> - **NEEDS-INFRA**: 물리 인프라에서만 검증 가능
> - **FIXED**: Sprint 17에서 수정 완료 (테스트 통과)

### #1. SDI 가상화 (4노드 → 리소스 풀 → 2 클러스터)

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL 생성 (4노드) | CODE-COMPLETE | `core/tofu.rs` — 12 tests |
| 리소스 풀 summary | CODE-COMPLETE | `core/resource_pool.rs` — 7 tests |
| 2 클러스터 분할 | CODE-COMPLETE | `core/validation.rs` — pool mapping |
| `sdi init` no-flag → 통합 리소스 풀 | CODE-COMPLETE | host 준비 + resource-pool-summary + host-infra HCL + tofu apply |
| `sdi init <spec>` → VM 풀 생성 | CODE-COMPLETE | HCL + state + spec 캐싱 |
| `sdi clean` host-infra 정리 | CODE-COMPLETE | host-infra tofu destroy 포함 (G-1 해결) |
| 실환경 `tofu apply` | NEEDS-INFRA | 4개 베어메탈 노드 필요 |

### #2. CF Tunnel GitOps 배포

| 항목 | 상태 | 비고 |
|------|------|------|
| `gitops/tower/cloudflared-tunnel/` | CODE-COMPLETE | kustomization + values.yaml 존재 |
| 터널 이름 `playbox-admin-static` 일치 | CODE-COMPLETE | `values.yaml` line 2 |
| ArgoCD ApplicationSet에 포함 | CODE-COMPLETE | tower-generator sync wave 3 |

### #3. CF Tunnel 완성도 + 사용자 수동 작업

| 항목 | 상태 | 비고 |
|------|------|------|
| 사용자 수동 작업 가이드 | CODE-COMPLETE | `docs/ops-guide.md` Section 1 (6단계) |
| 라우팅 3개 도메인 | CODE-COMPLETE | api.k8s / auth / cd .jinwang.dev |
| CF credentials 사전 검증 | ~~GAP~~ **FIXED** | C-10: `scalex bootstrap` 전 credentials 존재 확인 검증 추가 |

### #4. CLI Rust 구현 + FP 원칙

| 항목 | 상태 | 비고 |
|------|------|------|
| Rust 구현 | CODE-COMPLETE | 27 .rs files, ~16,800 LOC |
| FP 원칙 (Pure Function) | CODE-COMPLETE | 모든 generator 순수 함수, I/O 분리 |
| thiserror + clap derive | CODE-COMPLETE | 에러 처리 + CLI 파싱 |
| 코드 구조 | CODE-COMPLETE | commands/core/models 3계층 |

### #5-7. 문서화 + Installation Guide

| 항목 | 상태 | 비고 |
|------|------|------|
| README Step 0-8 | CODE-COMPLETE | Pre-flight 포함 9단계 가이드 |
| docs/ 7개 문서 | CODE-COMPLETE | architecture, ops-guide, troubleshooting 등 |
| CLI 레퍼런스 | CODE-COMPLETE | README에 core + query 전체 문서화 |
| E2E 실행 검증 | NEEDS-INFRA | 실제 베어메탈에서 Step 0~8 순서 실행 필요 |

### #8. CLI 기능 전체

| 명령어 | 상태 | 비고 |
|--------|------|------|
| `scalex facts` | CODE-COMPLETE | 4 tests, SSH 스크립트 + 파싱 |
| `scalex sdi init` (no flag) | CODE-COMPLETE | host 준비 + resource-pool + host-infra HCL + tofu apply |
| `scalex sdi init <spec>` | CODE-COMPLETE | HCL + state + summary + spec 캐싱 |
| `scalex sdi clean --hard` | CODE-COMPLETE | host-infra tofu destroy 포함 |
| `scalex sdi sync` | CODE-COMPLETE | 13 tests (diff, conflict, add/remove) |
| `scalex cluster init` | CODE-COMPLETE | inventory + vars + kubespray 실행 |
| `scalex get` (4 subcommands) | CODE-COMPLETE | 18 tests |
| `scalex bootstrap` | CODE-COMPLETE | ArgoCD Helm + 클러스터 등록 + spread 적용 |

### #9. Baremetal 확장성

| 항목 | 상태 | 비고 |
|------|------|------|
| `ClusterMode::Baremetal` | CODE-COMPLETE | enum + inventory + vars generation |
| k3s 배제 | CODE-COMPLETE | Kubespray만 사용, k3s 참조 0건 |

### #10. 시크릿 템플릿화

| 항목 | 상태 | 비고 |
|------|------|------|
| `credentials/*.example` 4개 | CODE-COMPLETE | baremetal-init, .env, secrets, cloudflare-tunnel |
| `core/secrets.rs` | CODE-COMPLETE | 12 tests |
| CF credentials 사전 검증 | ~~GAP~~ **FIXED** | C-10: bootstrap 전 검증 추가 |

### #11. 커널 파라미터 튜닝

| 항목 | 상태 | 비고 |
|------|------|------|
| `scalex kernel-tune` | CODE-COMPLETE | 14 tests |
| 가이드 | CODE-COMPLETE | `docs/ops-guide.md` Section 3 |

### #12. 디렉토리 구조

| 항목 | 상태 | 비고 |
|------|------|------|
| scalex-cli/ + gitops/ 핵심 구조 | CODE-COMPLETE | Checklist 요구사항 일치 |
| 불필요 파일 없음 | CODE-COMPLETE | `.omc/` 상태 파일은 gitignored |

### #13. 멱등성

| 항목 | 상태 | 비고 |
|------|------|------|
| HCL/inventory/vars 재생성 동일성 | CODE-COMPLETE | idempotency tests |
| 실환경 재적용 | NEEDS-INFRA | `sdi clean → sdi init → cluster init` 사이클 |

### #14. 외부 kubectl (CF Tunnel)

| 항목 | 상태 | 비고 |
|------|------|------|
| CF Tunnel Pre-OIDC kubectl | ~~BUG~~ **FIXED** | C-7: client cert → CF Tunnel 불가. token 기반 대안 문서화 + 검증 추가 |
| Tailscale kubectl | CODE-COMPLETE | Tower kubeconfig + Tailscale IP |
| SOCKS5 프록시 | ~~BUG~~ **FIXED** | C-8: ClusterIP → Tailscale/LAN 경유로 접근 경로 문서화 |
| Tower SAN 검증 | ~~GAP~~ **FIXED** | C-9: Tailscale IP를 Tower SAN에 추가하는 검증 |

### #15. NAT 접근 경로

| 항목 | 상태 | 비고 |
|------|------|------|
| CF Tunnel + Tailscale + LAN | CODE-COMPLETE | ops-guide Section 4 |
| 스위치 접근 가이드 | CODE-COMPLETE | 포함됨 |
| 접근 경로별 인증 제약 명시 | ~~GAP~~ **FIXED** | C-7: CF Tunnel은 OIDC만, Tailscale은 cert/token, LAN은 모두 가능 |

---

## Execution Plan — Sprint 17 시리즈

### Sprint 17a: CF Tunnel Pre-OIDC kubectl 인증 수정 (C-7, C-11) — CRITICAL

> **TDD**: RED → GREEN → REFACTOR

**문제**: CF Tunnel은 HTTP 모드(L7)로 동작하여 TLS를 CF Edge에서 종단한다.
따라서 kubectl의 client certificate auth가 kube-apiserver에 전달되지 않는다.
ops-guide의 "admin kubeconfig server URL만 변경하면 된다" 가이드는 아키텍처적으로 불가능.

**해결 방안**:
1. ops-guide 수정: CF Tunnel 경유 Pre-OIDC kubectl은 불가능함을 명시
2. Pre-OIDC 외부 접근은 Tailscale 경유만 가능함을 문서화
3. CF Tunnel은 OIDC 설정 완료 후에만 kubectl 접근 가능
4. 검증 코드 추가: CF Tunnel 접근 경로별 인증 호환성 validation

- [x] RED: `validate_cf_tunnel_auth_compatibility()` 테스트 — CF Tunnel + client cert 조합이 warning 생성
- [x] GREEN: validation 로직 구현
- [x] REFACTOR: ops-guide.md 수정
- [x] 커밋

### Sprint 17b: Tower `supplementary_addresses_in_ssl_keys` 검증 (C-9) — HIGH

> **TDD**: RED → GREEN → REFACTOR

**문제**: Tailscale IP(`100.64.0.1`)가 Sandbox에만 있고 Tower에 없음.
Tower kubeconfig를 Tailscale 경유로 사용하려면 Tower API server cert에도 해당 IP가 필요.
(단, Tower VM은 `192.168.88.100`이므로 Tailscale IP는 port-forward/proxy 시에만 필요)

- [x] RED: Tower 클러스터에 Tailscale bastion IP가 SAN에 포함되어야 한다는 테스트
- [x] GREEN: k8s-clusters.yaml.example에 tower `supplementary_addresses_in_ssl_keys` 추가
- [x] REFACTOR: validation에 bastion IP SAN 검증 추가

### Sprint 17c: SOCKS5 프록시 접근성 수정 (C-8) — HIGH

> **문제**: SOCKS5 프록시가 ClusterIP로만 배포되어 외부 접근 불가.
> kubectl port-forward로 접근하려면 이미 kubectl이 필요 → chicken-and-egg.

**해결**: SOCKS5는 LAN/Tailscale 경유 Tower SSH → port-forward로 사용.
아키텍처적으로 이것이 올바른 접근 — 보안상 SOCKS5를 외부에 직접 노출하면 안 됨.
문서에 사용 경로 명확히 기재.

- [x] ops-guide에 SOCKS5 접근 경로 명시
- [x] 검증 테스트 추가

### Sprint 17d: CF credentials 사전 검증 (C-10) — MEDIUM

- [x] RED: bootstrap validation이 cloudflare-tunnel.json 존재를 확인하는 테스트
- [x] GREEN: validation 로직 추가
- [x] REFACTOR

### Sprint 17e: 전체 테스트 통과 확인 + 커밋

- [ ] `cargo test` 전체 통과
- [ ] `cargo clippy` warning 0
- [ ] `cargo fmt --check` 통과
- [ ] 커밋 + 푸쉬

### Sprint 18: 실환경 E2E (물리 인프라 필요)

- [ ] I-1: `scalex facts --all` 실행 → 4노드 HW 정보 수집
- [ ] I-2: `scalex sdi init config/sdi-specs.yaml` → VM 풀 생성
- [ ] I-3: `scalex cluster init config/k8s-clusters.yaml` → K8s 프로비저닝
- [ ] I-4: `scalex secrets apply` → 시크릿 배포
- [ ] I-5: `scalex bootstrap` → ArgoCD + GitOps
- [ ] I-6: Tailscale 경유 외부 kubectl 접근 검증
- [ ] I-7: OIDC 설정 후 CF Tunnel 경유 kubectl 접근 검증
- [ ] I-8: `sdi clean --hard --yes-i-really-want-to` + 재구축 (멱등성)

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
[CF Tunnel] (OIDC only — OIDC 설정 완료 후에만 kubectl 가능)
  kubectl (OIDC token) → CF Edge → cloudflared → kube-apiserver
  ⚠ client certificate auth는 CF Tunnel 통과 불가 (TLS 종단)

[Tailscale] (cert + token — Pre-OIDC 외부 접근의 유일한 방법)
  kubectl (admin cert) → Tailscale IP → kube-apiserver
  SSH → Tailscale IP → bastion → 내부 노드

[LAN] (모든 인증 방식)
  kubectl → LAN IP → kube-apiserver
  SSH → LAN IP → 노드
```

---

## Test Summary

| Module | Tests | Coverage |
|--------|-------|----------|
| core/validation | 74+ | pool mapping, cluster IDs, CIDR, DNS, single-node, baremetal, idempotency, sync wave, AppProject, sdi-init, E2E pipeline, SSH, 3rd cluster, GitOps consistency, spec caching, **CF Tunnel auth, Tower SAN, CF credentials** |
| core/gitops | 39 | ApplicationSet, kustomization, sync waves, Cilium, ClusterMesh, generators |
| core/kubespray | 32+ | inventory (SDI + baremetal), cluster vars, OIDC, Cilium, single-node, **Tower SAN** |
| commands/status | 21 | platform status reporting |
| commands/sdi | 24 | network resolve, host infra, pool state, clean validation, CIDR prefix, host-infra clean |
| commands/get | 18 | facts row, config status, SDI pools, clusters |
| core/config | 15 | baremetal config, semantic validation, camelCase |
| core/kernel | 14 | kernel-tune recommendations |
| commands/bootstrap | 14+ | ArgoCD helm, cluster add, kubectl apply, pipeline, **CF credentials check** |
| core/sync | 13 | sync diff, VM conflict, add+remove |
| core/secrets | 12 | K8s secret generation |
| core/host_prepare | 12 | KVM, bridge, VFIO |
| core/tofu | 12 | HCL gen, SSH URI, VFIO, single-node, host-infra |
| commands/cluster | 11 | cluster init, SDI/baremetal, gitops |
| models/* | 8 | parse/serialize |
| core/resource_pool | 7 | aggregation, table, disk_gb |
| core/ssh | 5 | SSH command building, ProxyJump key, reachable_node_ip key |
| commands/facts | 4 | facts gathering |
| **TOTAL** | **388** | Sprint 17: +8 tests (CF Tunnel auth, Tower SAN, SOCKS5, CF credentials) |

---

## Sprint History

| Sprint | Date | Tests | Summary |
|--------|------|-------|---------|
| **17a** | 2026-03-11 | 388 | CF Tunnel Pre-OIDC auth 수정 — client cert 불가 문서화, Tailscale 권장 (C-7, C-11) |
| **17b** | 2026-03-11 | 388 | Tower supplementary_addresses_in_ssl_keys 추가 (C-9) |
| **17c** | 2026-03-11 | 388 | SOCKS5 프록시 접근성 + port-forward 가이드 문서화 (C-8) |
| **17d** | 2026-03-11 | 388 | CF credentials 사전 검증 — bootstrap 시 warning (C-10) |
| 16e | 2026-03-11 | 380 | README/DASHBOARD 테스트 수 업데이트 |
| 16d | 2026-03-11 | 380 | 중복 facts 로드 확인 |
| 16c | 2026-03-11 | 380 | SDI spec 캐싱 |
| 16b | 2026-03-11 | 379 | resource-pool-summary 공통 경로 |
| 16a | 2026-03-11 | 379 | `sdi clean` host-infra tofu destroy |
| 15f | 2026-03-11 | 375 | playbox-root → scalex-root rename |
| 15e | 2026-03-11 | 374 | Sandbox 외부 접근 아키텍처 결정 |
| 15d | 2026-03-11 | 372 | resource_pool disk_gb |
| 15b-c | 2026-03-11 | 370 | `scalex bootstrap` + README |
| 15a | 2026-03-11 | 355 | Kubespray 서브모듈 경로 해결 |
| 13d | 2026-03-11 | 352 | Edge cases: Cilium cluster_id, CIDR overlap |
| 13c | 2026-03-11 | 347 | 2-layer template, OIDC, credentials |
| 13b | 2026-03-11 | 342 | pre-OIDC kubectl, NAT 접근 |
| 13a | 2026-03-11 | 340 | Checklist 15항목 갭 분석 |
