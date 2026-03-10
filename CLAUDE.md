# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

**Unified multi-cluster Kubernetes provisioning** repo. A single CLI (`./playbox`) + single `values.yaml` provisions a two-cluster architecture (Tower + Sandbox) on 4 bare-metal nodes, with ArgoCD GitOps, Keycloak OIDC, and Cloudflare Tunnel for external access.

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

The `up` command runs steps in order: `preflight → prepare-nodes → create-tower → create-sandbox → bootstrap → configure-oidc → generate-kubeconfig`.

## Testing

```bash
# Run all tests (requires venv with pytest, jinja2, pyyaml, yamllint)
./tests/run-tests.sh

# Individual suites
pytest tests/ -v                     # Template + YAML tests
pytest tests/ -k test_netplan        # Single test by name
bats tests/bats/*.bats               # All shell tests
bats tests/bats/common.bats          # Single bats file
yamllint -c .yamllint.yml gitops/ values.yaml
shellcheck playbox lib/*.sh

# OpenTofu validation (also included in run-tests.sh)
cd tofu && tofu init -backend=false -input=false && tofu validate
```

**Test fixtures**: `tests/fixtures/values-{full,minimal,invalid}.yaml` — loaded via pytest fixtures in `tests/conftest.py` (`values_full`, `values_minimal`, `values_invalid`).

**BATS tests** mock external commands (`ssh`, `kubectl`, `helm`, etc.) via `PLAYBOX_*` env vars defined in `lib/common.sh` (e.g., `PLAYBOX_YQ`, `PLAYBOX_SSH_CMD`, `PLAYBOX_KUBECTL`).

## Key Patterns

- **Single Source of Truth**: `values.yaml` drives everything. Templates consume it via `yq_read()`/`yq_read_default()` helpers in `lib/common.sh`.
- **GitOps-First**: Post-bootstrap, ArgoCD manages all cluster state.
- **Sync waves**: 0=ArgoCD/config, 1=Cilium/cert-manager/storage, 2=cilium-resources, 3=tunnel/keycloak, 4=RBAC.
- **Idempotent**: Every CLI operation safe to re-run. Uses `kubectl apply` (never `create`), `helm upgrade --install --atomic`.
- **Secrets created by CLI**: CF tunnel credentials, Keycloak passwords via `kubectl create secret --dry-run=client | kubectl apply -f -`.
- **Generated output**: `_generated/` (gitignored) holds rendered templates and kubeconfigs (`tower.kubeconfig`, `sandbox.kubeconfig`).

## GitOps Pattern

**Bootstrap chain**: `spread.yaml` → creates root Application pointing to `generators/` → generators read `catalog.yaml` via list elements → create Applications from `apps/`.

| Concept | ArgoCD Resource | Path |
|---------|----------------|------|
| **Project** | AppProject | `gitops/clusters/playbox/projects/` |
| **Generator** | ApplicationSet | `gitops/clusters/playbox/generators/` |
| **App** | Application config | `gitops/clusters/playbox/apps/{generator}/{app}/` |
| **Catalog** | App registry | `gitops/clusters/playbox/catalog.yaml` |

**Adding a new app**: (1) Add entry to `catalog.yaml` under the appropriate generator, (2) add matching list element in `generators/{generator}-generator.yaml` with `appName`, `namespace`, `syncWave`, (3) create `apps/{generator}/{app}/kustomization.yaml`.

**Important**: Generator YAML files (`generators/base-generator.yaml`) hardcode the app list — they must be updated in sync with `catalog.yaml`. The catalog is the authoritative registry; generators are the ArgoCD-side implementation.

## Coding Style

- **YAML**: 2-space indent, double quotes for variables/IPs, kebab-case resource names, snake_case values.yaml keys
- **Shell**: `set -euo pipefail`, snake_case functions prefixed by module (e.g., `preflight_run`, `cluster_tower_create`), logging via `log_info`/`log_warn`/`log_error`/`log_step`
- **Templates**: `.j2` for Jinja2, read from `values.yaml` only, generated output to `_generated/` (gitignored)
- **Helm**: Always `helm upgrade --install --atomic --wait --timeout 5m`
- **kubectl**: Always `kubectl apply` (never `create` in scripts)

## Project Structure

```
├── playbox                    # CLI entry point (bash), dispatches to lib/ functions
├── values.yaml                # Single source of truth — user edits this only
├── lib/                       # CLI library modules, one per concern:
│   │                          #   common (logging, yq, ssh, helm helpers)
│   │                          #   preflight, network, cluster, gitops, oidc, tunnel, client
├── ansible/                   # Node preparation (user creation, netplan, kernel params)
├── tofu/                      # OpenTofu for tower VM (libvirt provider)
├── kubespray/                 # Kubespray config templates (cluster-vars.yml.j2, addons.yml.j2)
├── gitops/                    # ArgoCD-managed GitOps
│   ├── bootstrap/spread.yaml  # Root Application + AppProjects
│   └── clusters/playbox/      # catalog.yaml, generators/, projects/, apps/
├── client/                    # kubeconfig-oidc.yaml.j2, setup-client.sh
├── tests/                     # BATS (shell) + pytest (templates, YAML validation)
│   ├── fixtures/              # values-full.yaml, values-minimal.yaml, values-invalid.yaml
│   ├── bats/                  # One .bats file per lib/ module
│   └── templates/ & yaml/     # Pytest: Jinja2 template rendering + YAML schema/consistency
└── _generated/                # Gitignored rendered output (kubeconfigs, templates)
```
