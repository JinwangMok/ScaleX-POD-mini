# ScaleX-POD-mini

[![CI](https://github.com/JinwangMok/ScaleX-POD-mini/actions/workflows/ci.yml/badge.svg)](https://github.com/JinwangMok/ScaleX-POD-mini/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

A unified platform that virtualizes bare-metal nodes via SDI (Software-Defined Infrastructure), provisions multi-cluster Kubernetes via Kubespray, and manages everything through ArgoCD GitOps.

Define your hardware once, and `scalex-pod` handles the rest — from VM creation to fully operational multi-cluster Kubernetes with GitOps.

---

## Architecture Overview

![Architecture Overview](docs/architecture-overview.png)

> Edit diagram: [architecture-overview.drawio](docs/architecture-overview.drawio) — open with [app.diagrams.net](https://app.diagrams.net/) or VS Code draw.io extension.

### 5-Layer SDI Architecture

```
Physical (bare-metal nodes)
  → SDI (OpenTofu virtualization → unified resource pool)
    → Node Pools (purpose-specific VM groups)
      → Clusters (Kubespray K8s provisioning)
        → GitOps (ArgoCD ApplicationSets for multi-cluster)
```

### 2-Cluster Design

| Cluster | Role | Components |
|---------|------|------------|
| **Tower** | Management | ArgoCD, Keycloak (OIDC), cert-manager, Cloudflare Tunnel |
| **Sandbox** | Workload | Cilium CNI, OIDC auth, local-path-provisioner |

Both clusters are provisioned via **Kubespray v2.30.0** with Cilium CNI and kube-vip HA. External access via **Cloudflare Tunnel + Tailscale** — no VPN required.

---

## Key Features

- **Single CLI** — `scalex-pod` handles the entire lifecycle: facts gathering → SDI provisioning → cluster init → GitOps bootstrap.
- **Automated E2E Install** — `bash install.sh --auto` provisions everything end-to-end from clean state in ~45 minutes, fully unattended. Resume-safe on interruption.
- **SDI Abstraction** — Decouple hardware from clusters. Same workflow whether you have 1 node or 100.
- **Multi-Cluster Dashboard** — See [scalex-tui](https://github.com/JinwangMok/scalex-tui) for the standalone k9s-inspired TUI dashboard (`scalex` command).
- **Scalable Multi-Cluster** — Template-based Kubespray + ArgoCD ApplicationSets. Add a cluster: define pool → define cluster → add generator → done.
- **Zero-VPN External Access** — Cloudflare Tunnel + Tailscale. OIDC (Keycloak) authentication for `kubectl` without a VPN.
- **GitOps-First** — Post-bootstrap, ArgoCD manages all cluster state. Sync waves ensure correct deployment order.
- **Idempotent** — Every CLI operation is safe to re-run.

---

## Quick Start

### Automated Install (Recommended)

```bash
# Interactive TUI — guides you through config → dependencies → build → provisioning
bash <(curl -fsSL https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh)

# Fully unattended — uses pre-configured credentials and config files
bash install.sh --auto
```

The installer is **resume-safe** — re-run to continue from the last completed phase. Supports bilingual UI (English/Korean, auto-detected or set via `SCALEX_LANG=ko`).

### Manual Install

```bash
git clone https://github.com/JinwangMok/ScaleX-POD-mini.git
cd ScaleX-POD-mini
git submodule update --init --recursive

# Build CLI
cd scalex-cli && cargo build --release && cd ..
export PATH="$PWD/scalex-cli/target/release:$PATH"

# Configure (copy examples, fill in real values)
cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml
cp credentials/.env.example credentials/.env
cp credentials/secrets.yaml.example credentials/secrets.yaml
cp config/sdi-specs.yaml.example config/sdi-specs.yaml
cp config/k8s-clusters.yaml.example config/k8s-clusters.yaml

# Provision
scalex-pod facts --all                           # 1. Gather hardware info
scalex-pod sdi init config/sdi-specs.yaml        # 2. Create VM pools
scalex-pod cluster init config/k8s-clusters.yaml # 3. Provision K8s clusters
scalex-pod secrets apply                         # 4. Deploy secrets
scalex-pod bootstrap                             # 5. ArgoCD + GitOps

# Verify
scalex-pod status
```

Full walkthrough with prerequisites and troubleshooting: **[docs/SETUP-GUIDE.md](docs/SETUP-GUIDE.md)**

---

## Dashboard

The multi-cluster TUI dashboard has been extracted to its own project: **[scalex-tui](https://github.com/JinwangMok/scalex-tui)** (`scalex` command).

Install separately: see the [scalex-tui README](https://github.com/JinwangMok/scalex-tui) for setup instructions.

---

## CLI Commands

| Command | Description |
|---------|-------------|
| `scalex-pod facts --all` | Gather bare-metal hardware info via SSH |
| `scalex-pod get baremetals\|sdi-pools\|clusters\|config-files` | Query resources and config validation |
| `scalex-pod sdi init\|clean\|sync` | SDI lifecycle: create VM pools, full reset, reconcile changes |
| `scalex-pod cluster init <k8s-clusters.yaml>` | Provision K8s clusters via Kubespray |
| `scalex-pod secrets apply` | Deploy pre-bootstrap K8s secrets |
| `scalex-pod bootstrap` | Install ArgoCD, register clusters, apply GitOps |
| `scalex-pod status` | Show platform status across all layers |
| `scalex-pod validate` | Validate config files before provisioning |
| `scalex-pod plan` | Plan VM placement based on available resources |
| `scalex-pod kernel-tune` | Generate kernel tuning parameters for K8s nodes |

Full CLI reference with config file details: **[docs/CLI-REFERENCE.md](docs/CLI-REFERENCE.md)**

---

## Project Structure

```
scalex-cli/              # Rust CLI (scalex-pod) — facts, sdi, cluster, get, status, secrets, bootstrap
  src/
    commands/            #   CLI command modules
    core/                #   Config parsing, Kubespray templating, SSH, validation
    models/              #   Data structures (cluster, SDI, baremetal)
gitops/                  # ArgoCD-managed multi-cluster GitOps
  bootstrap/spread.yaml  #   Root bootstrap (tower-root + sandbox-root)
  generators/            #   ApplicationSets per cluster
  projects/              #   AppProjects (tower, sandbox)
  common/                #   All clusters: cert-manager, cilium-resources, kyverno, kyverno-policies, scalex-dash-rbac
  tower/                 #   Tower-only: argocd, keycloak, cloudflared-tunnel, cert-issuers, local-path-provisioner, ...
  sandbox/               #   Sandbox-only: cilium, local-path-provisioner, rbac, test-resources, ...
install.sh               # Interactive TUI installer (--auto for unattended E2E)
credentials/             # Secrets + init config (gitignored, .example templates)
config/                  # User config: sdi-specs.yaml, k8s-clusters.yaml
ansible/                 # Node preparation playbooks (user setup, networking, kernel params)
kubespray/               # Kubespray submodule (v2.30.0) + templates
client/                  # OIDC kubeconfig generation (Keycloak)
tests/                   # Test runner, integration tests, YAML lint
docs/                    # Architecture, guides, operations, diagrams
ops/                     # Operational utilities (task model, degradation inventory)
.github/workflows/       # CI (lint, test, audit) + release
_generated/              # Gitignored output (facts, SDI HCL, cluster configs)
```

---

## GitOps Pattern

Post-bootstrap, ArgoCD manages all cluster state via ApplicationSets:

```
spread.yaml (root)
  → tower-root / sandbox-root
    → generators/{tower,sandbox}/ (ApplicationSets)
      → common/{app}/    — deployed to all clusters
      → tower/{app}/     — tower-only apps
      → sandbox/{app}/   — sandbox-only apps
```

**Sync waves** ensure correct deployment order:

| Wave | Components |
|------|------------|
| 0 | ArgoCD, cluster-config |
| 1 | Cilium, cert-manager, Kyverno, local-path-provisioner |
| 2 | cilium-resources, cert-issuers, kyverno-policies |
| 3 | cloudflared-tunnel, keycloak |
| 4 | RBAC |

Details: **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**

---

## Technology Stack

| Layer | Technology |
|-------|-----------|
| CLI | Rust 1.88+, clap, serde, thiserror, anyhow |
| Infrastructure | OpenTofu / libvirt (SDI virtualization) |
| Kubernetes | Kubespray v2.30.0, containerd, Cilium 1.17.5, kube-vip |
| GitOps | ArgoCD (Helm), ApplicationSets, Kustomize |
| Networking | Cloudflare Tunnel, Tailscale, Cilium (kube-proxy free) |
| Auth | Keycloak (OIDC), cert-manager (Let's Encrypt) |
| Node Prep | Ansible playbooks |
| CI/CD | GitHub Actions |

---

## Minimum Bare-Metal Requirements

Host hypervisor overhead (~15% of physical resources) is included. Memory rounded to practical sizes.

| Profile | Nodes | CPU (cores) | RAM (GB) | Disk (GB) | Description |
|---------|-------|-------------|----------|-----------|-------------|
| **Minimal** | 1 | 4+ | 8+ | 50+ | Tower only, no workload cluster |
| **Basic** | 1 | 8+ | 16+ | 80+ | Tower + Sandbox on same node |
| **Standard** | 2+ | 8+ each | 16+ each | 100+ each | Separate control planes + workers |
| **HA** | 4+ | 8+ each | 16+ each | 100+ each | etcd quorum (3 CP per cluster) |

> Resource validation: `scalex-pod validate` checks VM allocations against physical hardware.

---

## Testing

```bash
./tests/run-tests.sh                # All tests (Rust + YAML lint + clippy + fmt)
cd scalex-cli && cargo test          # Rust CLI unit tests
```

**CI/CD** via GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)):
- `cargo fmt --check` — formatting
- `cargo clippy` — linting
- `cargo test` — unit tests + coverage
- MSRV check (Rust 1.88)
- `cargo audit` — security audit
- `yamllint` — GitOps YAML linting

---

## Documentation

| Document | Description |
|----------|-------------|
| [SETUP-GUIDE](docs/SETUP-GUIDE.md) | Full provisioning walkthrough (prerequisites → verification) |
| [CLI-REFERENCE](docs/CLI-REFERENCE.md) | `scalex-pod` commands, config files, VM resource budgets |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Two-cluster design, network topology, access paths |
| [scalex-tui](https://github.com/JinwangMok/scalex-tui) | Multi-cluster TUI dashboard (separate repo) |
| [ops-guide](docs/ops-guide.md) | Cloudflare Tunnel, Keycloak, kernel tuning, external access |
| [TROUBLESHOOTING](docs/TROUBLESHOOTING.md) | Common issues and fixes |
| [CONTRIBUTING](docs/CONTRIBUTING.md) | Code style, testing, git conventions |
| [CLOUDFLARE-ACCESS](docs/CLOUDFLARE-ACCESS.md) | Cloudflare Tunnel setup details |
| [NETWORK-DISCOVERY](docs/NETWORK-DISCOVERY.md) | NIC discovery and bond configuration |

---

## Contributing

See **[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)** for code style, testing, and git conventions.

**TL;DR**: Pure functions in Rust, 2-space YAML, conventional commits. All of `cargo test`, `cargo clippy`, `cargo fmt --check`, and `yamllint` must pass.
