# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
