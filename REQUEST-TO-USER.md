# REQUEST-TO-USER.md

> 사용자(인프라 관리자)가 직접 수행해야 하는 작업 목록.
> 물리 인프라 접근이 필수이거나, 외부 서비스 계정이 필요한 항목만 포함합니다.

---

## 1. 사전 준비 (사용자만 가능)

| # | 작업 | 이유 |
|---|------|------|
| P-1 | **Cloudflare 설정**: Zero Trust Dashboard에서 터널 `playbox-admin-static` 생성 → `credentials/cloudflare-tunnel.json` 저장 | CF 계정/도메인 필요 |
| P-2 | **DNS 설정**: `api.k8s.jinwang.dev`, `auth.jinwang.dev`, `cd.jinwang.dev` CNAME 레코드 설정 | 도메인 소유자만 가능 |
| P-3 | **Tailscale 설정**: bastion 노드(playbox-0)에 Tailscale 설치 및 인증 | Tailscale 계정 필요 |
| P-4 | **SSH 접근 확인**: 모든 노드에 SSH 키 배포 또는 비밀번호 설정 확인 | 물리 접근 필요 |

---

## 2. 실환경 E2E 검증 (Sprint 41)

> **전제 조건**: 4개 베어메탈 노드(playbox-0~3)에 대한 물리적 접근 + 네트워크 구성 + 위 사전 준비 완료

| # | 작업 | 명령어 |
|---|------|--------|
| E-1 | 4노드 HW 정보 수집 | `scalex facts --all` |
| E-2 | VM 풀 생성 | `scalex sdi init config/sdi-specs.yaml` |
| E-3 | K8s 클러스터 프로비저닝 | `scalex cluster init config/k8s-clusters.yaml` |
| E-4 | 시크릿 배포 | `scalex secrets apply` |
| E-5 | ArgoCD + GitOps 부트스트랩 | `scalex bootstrap` |
| E-6 | Tailscale 경유 외부 kubectl 접근 검증 | 수동 테스트 |
| E-7 | Keycloak Realm/Client 설정 후 CF Tunnel kubectl 검증 | 수동 테스트 (Keycloak WebUI) |
| E-8 | 전체 초기화 + 재구축 (멱등성) | `sdi clean --hard --yes-i-really-want-to` → E-1~E-5 재실행 |

---

## 3. 자동화 완료 항목

다음 항목들은 **GitHub Actions CI** (`.github/workflows/ci.yml`)로 자동화되었습니다:

- `cargo test` 전체 통과 확인 → push/PR 시 자동 실행
- `cargo clippy -- -D warnings` → push/PR 시 자동 실행
- `cargo fmt --check` → push/PR 시 자동 실행
- `yamllint gitops/` → push/PR 시 자동 실행
