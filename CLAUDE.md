# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

**Unified multi-cluster Kubernetes provisioning** repo using a 5-layer SDI architecture:
Physical (4 bare-metal) → SDI (OpenTofu virtualization) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

**Primary CLI**: `scalex` (Rust, in `scalex-cli/`) — handles facts gathering, SDI provisioning, multi-cluster Kubespray, and resource queries.

## Architecture

- **Tower cluster**: Management cluster (ArgoCD, Keycloak, Cloudflare Tunnel). Provisioned via Kubespray on SDI VMs.
- **Sandbox cluster**: Workload cluster. Provisioned via Kubespray on SDI VMs or bare-metal nodes.
- **All clusters use Kubespray** (production-grade). No k3s.
- **External access**: Cloudflare Tunnel + Tailscale. LAN access via switch.

## CLI (`scalex`)

```bash
# Build
cd scalex-cli && cargo build --release

# Hardware facts gathering
scalex facts --all                       # Gather all node hardware info
scalex facts --host playbox-0            # Single node

# SDI (Software-Defined Infrastructure)
scalex sdi init                          # Virtualize all bare-metal → resource pool
scalex sdi init <sdi-specs.yaml>         # Create VM pools from spec
scalex sdi clean --hard --yes-i-really-want-to  # Full reset
scalex sdi sync                          # Reconcile bare-metal changes

# Cluster provisioning
scalex cluster init <k8s-clusters.yaml>  # Kubespray → multi-cluster

# Resource queries
scalex get baremetals                    # Hardware facts table
scalex get sdi-pools                     # VM pool status
scalex get clusters                      # Cluster inventory
scalex get config-files                  # Config file validation
```

### Config Files

| File | Purpose |
|------|---------|
| `credentials/.baremetal-init.yaml` | SSH access to bare-metal nodes (user-provided) |
| `credentials/.env` | SSH passwords/key paths (user-provided) |
| `credentials/secrets.yaml` | Keycloak, ArgoCD, Cloudflare secrets |
| `config/sdi-specs.yaml` | VM pool definitions (CPU, RAM, disk, GPU) |
| `config/k8s-clusters.yaml` | Cluster definitions (mode, role, addons) |

## Testing

```bash
# Rust CLI tests (598 tests)
cd scalex-cli && cargo test
cargo clippy                             # Lint
cargo fmt --check                        # Format check

# All tests + YAML lint
./tests/run-tests.sh
```

## Key Patterns

- **GitOps-First**: Post-bootstrap, ArgoCD manages all cluster state via ApplicationSets.
- **Sync waves**: 0=ArgoCD/cluster-config, 1=Cilium/cert-manager/Kyverno/storage, 2=cilium-resources/cert-issuers/kyverno-policies, 3=tunnel/keycloak, 4=RBAC.
- **Idempotent**: Every CLI operation safe to re-run.
- **Pure Functions**: Rust CLI uses pure functions for HCL/inventory/vars generation. No side effects in generators.
- **Secrets**: Created by CLI, stored in `credentials/` (gitignored). Templates in `credentials/*.example`.
- **Generated output**: `_generated/` (gitignored) holds SDI HCL, kubespray inventory, kubeconfigs.

## GitOps Pattern

**Bootstrap**: `scalex bootstrap` (internally: Helm Cilium install on all clusters → Helm ArgoCD install → cluster register → `kubectl apply -f gitops/bootstrap/spread.yaml`)

**Multi-cluster structure**:
- `spread.yaml` → creates `tower-root` + `sandbox-root` Applications
- Each root points to `gitops/generators/{tower,sandbox}/`
- Generators deploy apps from `gitops/{common,tower,sandbox}/`

| Concept | ArgoCD Resource | Path |
|---------|----------------|------|
| **Projects** | AppProject | `gitops/projects/{tower,sandbox}-project.yaml` |
| **Generators** | ApplicationSet | `gitops/generators/{tower,sandbox}/` |
| **Common Apps** | Kustomization | `gitops/common/{cilium-resources,cert-manager,kyverno,kyverno-policies}/` |
| **Tower Apps** | Kustomization | `gitops/tower/{argocd,cilium,cert-issuers,cloudflared-tunnel,cluster-config,keycloak}/` |
| **Sandbox Apps** | Kustomization | `gitops/sandbox/{cilium,cluster-config,local-path-provisioner,rbac,test-resources}/` |

**Adding a new common app**: (1) Create `gitops/common/{app}/kustomization.yaml`, (2) Add element to both `gitops/generators/tower/common-generator.yaml` and `gitops/generators/sandbox/common-generator.yaml`.

**Adding a cluster-specific app**: (1) Create `gitops/{tower|sandbox}/{app}/kustomization.yaml`, (2) Add element to `gitops/generators/{tower|sandbox}/{tower|sandbox}-generator.yaml`.

## Coding Style

- **Rust**: Pure functions, no side effects in generators. `thiserror` for errors, `clap` derive for CLI.
- **YAML**: 2-space indent, double quotes for variables/IPs, kebab-case resource names.

## Project Structure

```
├── scalex-cli/                # Rust CLI (primary) — facts, SDI, cluster, get, status, kernel-tune, secrets
├── gitops/                    # ArgoCD-managed GitOps (multi-cluster)
│   ├── bootstrap/spread.yaml  # Root bootstrap (tower-root + sandbox-root)
│   ├── generators/            # ApplicationSets per cluster
│   │   ├── tower/             # common-generator + tower-generator
│   │   └── sandbox/           # common-generator + sandbox-generator
│   ├── projects/              # AppProjects (tower-project, sandbox-project)
│   ├── common/                # Apps for ALL clusters (cilium-resources, cert-manager, kyverno, kyverno-policies)
│   ├── tower/                 # Tower-only apps (argocd, keycloak, cloudflared-tunnel, ...)
│   └── sandbox/               # Sandbox-only apps (local-path-provisioner, rbac, ...)
├── credentials/               # Secrets + init config (gitignored, .example templates)
├── config/                    # User config templates (sdi-specs, k8s-clusters, baremetal)
├── ansible/                   # Node preparation playbooks
├── kubespray/                 # Kubespray submodule (v2.30.0) + templates
├── client/                    # OIDC kubeconfig generation
├── tests/                     # Test runner + YAML lint
├── docs/                      # Operations guide (Cloudflare, Keycloak, kernel, access)
└── _generated/                # Gitignored output (SDI HCL, inventories, kubeconfigs)
```
