# CLAUDE.md

이 파일은 Claude Code (claude.ai/code)가 이 저장소의 코드를 다룰 때 참고하는 가이드입니다.
This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## 🇰🇷 Korean

## 저장소 개요

**통합 멀티 클러스터 Kubernetes 프로비저닝** 저장소. 단일 CLI (`./playbox`) + 단일 `values.yaml`로 2-클러스터 아키텍처(Tower + Sandbox)를 4대 베어메탈 노드에 프로비저닝하며, ArgoCD GitOps, Keycloak OIDC, Cloudflare Tunnel을 통한 외부 접근을 제공합니다.

**레거시 하위 프로젝트** (`b-bim-bap/`, `k8s-playbox/`, `k8s-sandbox/`, `DataX-Ops/`)는 이전 분산 구성의 참조 저장소로, 아카이브 예정입니다.

## 아키텍처

- **Tower 클러스터**: playbox-0 위 k3s VM (OpenTofu + libvirt). 양쪽 클러스터를 관리하는 ArgoCD 실행.
- **Sandbox 클러스터**: 4대 베어메탈 노드(playbox-0..3)에 kubespray로 관리되는 K8s. 워크로드 실행.
- **외부 접근**: Cloudflare Tunnel → K8s API, ArgoCD, Keycloak. 클라이언트 측 별도 소프트웨어 불필요.

## CLI

```bash
./playbox <command> [flags]

# 전체 프로비저닝
./playbox up
./playbox up --dry-run              # 미리보기
./playbox up --from create-sandbox  # 특정 단계부터 재개
./playbox up --skip create-tower    # Tower 건너뛰기 (단일 클러스터)

# 개별 단계
./playbox preflight | prepare-nodes | create-tower | create-sandbox
./playbox bootstrap | configure-oidc | generate-kubeconfig

# 유틸리티
./playbox status | destroy-sandbox | destroy-tower | destroy-all | discover-nics
```

## 프로젝트 구조

```
├── playbox                    # CLI 진입점 (bash)
├── values.yaml                # 단일 설정 소스 — 사용자는 이 파일만 편집
├── lib/                       # CLI 라이브러리 모듈 (common, preflight, network, cluster, gitops, oidc, tunnel, client)
├── ansible/                   # 노드 준비 (사용자 생성, netplan, 커널 파라미터)
│   └── templates/             # netplan.yml.j2 (bond0/br0), sudoers.j2
├── tofu/                      # Tower VM용 OpenTofu (libvirt 프로바이더)
├── kubespray/                 # Kubespray 설정 템플릿 (cluster-vars.yml.j2, addons.yml.j2)
├── gitops/                    # ArgoCD 관리 GitOps
│   ├── bootstrap/spread.yaml  # 루트 Application + AppProjects
│   └── clusters/playbox/      # catalog.yaml, generators/, projects/, apps/
├── client/                    # kubeconfig-oidc.yaml.j2, setup-client.sh
├── tests/                     # BATS (셸) + pytest (템플릿, YAML 검증)
└── docs/                      # 아키텍처, 설정 가이드, 트러블슈팅
```

## GitOps 패턴 (b-bim-bap에서 차용)

| 개념 | ArgoCD 리소스 | 경로 |
|------|--------------|------|
| **프로젝트** | AppProject | `gitops/clusters/playbox/projects/` |
| **제너레이터** | ApplicationSet | `gitops/clusters/playbox/generators/` |
| **앱** | Application 설정 | `gitops/clusters/playbox/apps/{generator}/{app}/` |
| **카탈로그** | 앱 레지스트리 | `gitops/clusters/playbox/catalog.yaml` |

**부트스트랩 체인**: `spread.yaml` → `generators/`를 가리키는 루트 Application 생성 → 제너레이터가 jsonPath로 `catalog.yaml` 읽기 → `apps/`에서 Application 생성.

**새 앱 추가**: 해당 제너레이터 아래 `catalog.yaml`에 추가, `apps/{generator}/{app}/kustomization.yaml` 생성, `catalog.yaml`의 `apps` 섹션에서 앱별 오버라이드 설정.

## 핵심 패턴

- **단일 설정 소스**: `values.yaml`이 모든 것을 구동. 템플릿이 이를 소비.
- **GitOps 우선**: 부트스트랩 이후 ArgoCD가 모든 클러스터 상태를 관리.
- **Sync Wave**: 0=ArgoCD/config, 1=Cilium/cert-manager/storage, 2=cilium-resources, 3=tunnel/keycloak, 4=RBAC.
- **멱등성**: 모든 CLI 작업은 재실행해도 안전.
- **CLI가 시크릿 생성**: CF 터널 자격증명, Keycloak 비밀번호를 `kubectl create secret --dry-run=client | kubectl apply -f -`로 생성.

## 테스트

```bash
# 전체 테스트 실행 (pytest, jinja2, pyyaml, yamllint이 포함된 venv 필요)
./tests/run-tests.sh

# 개별 테스트 스위트
pytest tests/ -v                     # 31개 템플릿 + YAML 테스트
bats tests/bats/*.bats               # 셸 스크립트 테스트
yamllint -c .yamllint.yml gitops/ values.yaml
shellcheck playbox lib/*.sh
```

## 코딩 스타일

- **YAML**: 2칸 들여쓰기, 변수/IP에 큰따옴표, kebab-case 리소스 이름, snake_case values.yaml 키
- **셸**: `set -euo pipefail`, 모듈별 접두사를 가진 snake_case 함수명, `log_info`/`log_warn`/`log_error`
- **템플릿**: Jinja2는 `.j2`, `values.yaml`에서만 읽기, 생성 출력은 `_generated/` (gitignored)
- **Helm**: 항상 `helm upgrade --install --atomic --wait --timeout 5m`
- **kubectl**: 스크립트에서 항상 `kubectl apply` 사용 (`create` 금지)

## 일반 작업

```bash
# 사전 검사
./playbox preflight

# 전체 프로비저닝
./playbox up

# 상태 확인
./playbox status

# Sandbox 초기화 및 재구축
./playbox destroy-sandbox && ./playbox create-sandbox && ./playbox bootstrap

# values.yaml용 NIC 정보 조회
./playbox discover-nics

# ArgoCD 관리자 비밀번호
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d; echo
```

---

## 🇬🇧 English

## Repository Overview

**Unified multi-cluster Kubernetes provisioning** repo. A single CLI (`./playbox`) + single `values.yaml` provisions a two-cluster architecture (tower + sandbox) on 4 bare-metal nodes, with ArgoCD GitOps, Keycloak OIDC, and Cloudflare Tunnel for external access.

**Legacy sub-projects** (`b-bim-bap/`, `k8s-playbox/`, `k8s-sandbox/`, `DataX-Ops/`) are reference repos from the previous fragmented setup — to be archived.

## Architecture

- **Tower cluster**: k3s VM on playbox-0 (via OpenTofu + libvirt). Runs ArgoCD that manages both clusters.
- **Sandbox cluster**: kubespray-managed K8s on all 4 bare-metal nodes (playbox-0..3). Runs workloads.
- **External access**: Cloudflare Tunnel → K8s API, ArgoCD, Keycloak. No client-side software needed.

## CLI

```bash
./playbox <command> [flags]

# Full provisioning
./playbox up
./playbox up --dry-run              # Preview
./playbox up --from create-sandbox  # Resume from step
./playbox up --skip create-tower    # Skip tower (single-cluster)

# Individual steps
./playbox preflight | prepare-nodes | create-tower | create-sandbox
./playbox bootstrap | configure-oidc | generate-kubeconfig

# Utilities
./playbox status | destroy-sandbox | destroy-tower | destroy-all | discover-nics
```

## Project Structure

```
├── playbox                    # CLI entry point (bash)
├── values.yaml                # Single source of truth — user edits this only
├── lib/                       # CLI library modules (common, preflight, network, cluster, gitops, oidc, tunnel, client)
├── ansible/                   # Node preparation (user creation, netplan, kernel params)
│   └── templates/             # netplan.yml.j2 (bond0/br0), sudoers.j2
├── tofu/                      # OpenTofu for tower VM (libvirt provider)
├── kubespray/                 # Kubespray config templates (cluster-vars.yml.j2, addons.yml.j2)
├── gitops/                    # ArgoCD-managed GitOps
│   ├── bootstrap/spread.yaml  # Root Application + AppProjects
│   └── clusters/playbox/      # catalog.yaml, generators/, projects/, apps/
├── client/                    # kubeconfig-oidc.yaml.j2, setup-client.sh
├── tests/                     # BATS (shell) + pytest (templates, YAML validation)
└── docs/                      # Architecture, setup guide, troubleshooting
```

## GitOps Pattern (adapted from b-bim-bap)

| Concept | ArgoCD Resource | Path |
|---------|----------------|------|
| **Project** | AppProject | `gitops/clusters/playbox/projects/` |
| **Generator** | ApplicationSet | `gitops/clusters/playbox/generators/` |
| **App** | Application config | `gitops/clusters/playbox/apps/{generator}/{app}/` |
| **Catalog** | App registry | `gitops/clusters/playbox/catalog.yaml` |

**Bootstrap chain**: `spread.yaml` → creates root Application pointing to `generators/` → generators read `catalog.yaml` via jsonPath → create Applications from `apps/`.

**Adding a new app**: Add to `catalog.yaml` under the appropriate generator, create `apps/{generator}/{app}/kustomization.yaml`, and set per-app overrides in `catalog.yaml`'s `apps` section.

## Key Patterns

- **Single Source of Truth**: `values.yaml` drives everything. Templates consume it.
- **GitOps-First**: Post-bootstrap, ArgoCD manages all cluster state.
- **Sync waves**: 0=ArgoCD/config, 1=Cilium/cert-manager/storage, 2=cilium-resources, 3=tunnel/keycloak, 4=RBAC.
- **Idempotent**: Every CLI operation safe to re-run.
- **Secrets created by CLI**: CF tunnel credentials, Keycloak passwords via `kubectl create secret --dry-run=client | kubectl apply -f -`.

## Testing

```bash
# Run all tests (requires venv with pytest, jinja2, pyyaml, yamllint)
./tests/run-tests.sh

# Individual suites
pytest tests/ -v                     # 31 template + YAML tests
bats tests/bats/*.bats               # Shell script tests
yamllint -c .yamllint.yml gitops/ values.yaml
shellcheck playbox lib/*.sh
```

## Coding Style

- **YAML**: 2-space indent, double quotes for variables/IPs, kebab-case resource names, snake_case values.yaml keys
- **Shell**: `set -euo pipefail`, snake_case functions prefixed by module, `log_info`/`log_warn`/`log_error`
- **Templates**: `.j2` for Jinja2, read from `values.yaml` only, generated output to `_generated/` (gitignored)
- **Helm**: Always `helm upgrade --install --atomic --wait --timeout 5m`
- **kubectl**: Always `kubectl apply` (never `create` in scripts)

## Common Operations

```bash
# Preflight check
./playbox preflight

# Full provisioning
./playbox up

# Check status
./playbox status

# Reset and rebuild sandbox
./playbox destroy-sandbox && ./playbox create-sandbox && ./playbox bootstrap

# Get NIC info for values.yaml
./playbox discover-nics

# ArgoCD admin password
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d; echo
```
