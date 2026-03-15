# ScaleX-POD-mini

A unified platform that virtualizes bare-metal nodes via SDI (Software-Defined Infrastructure), provisions multi-cluster Kubernetes via Kubespray, and manages everything through ArgoCD GitOps.

Define your hardware once, and `scalex` handles the rest — from VM creation to fully operational multi-cluster Kubernetes with GitOps.

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

| Cluster | Role | Description |
|---------|------|-------------|
| **Tower** | Management | ArgoCD, Keycloak, cert-manager. Manages all clusters. |
| **Sandbox** | Workload | Cilium CNI, OIDC auth, Cloudflare Tunnel for external access. |

---

## Key Features

- **SDI Abstraction** — Decouple hardware from clusters. Same workflow (`scalex sdi init → cluster init`) whether you have 1 node or 100.
- **Scalable Multi-Cluster** — Template-based Kubespray + ArgoCD ApplicationSets. Add a cluster: define pool → define cluster → add generator → done.
- **Role-Based Separation** — Tower handles meta-management (ArgoCD, Keycloak, certs); workload clusters scale independently.
- **Zero-VPN External Access** — Cloudflare Tunnel + Tailscale. OIDC (Keycloak) authentication for `kubectl` without a VPN.
- **Single CLI** — `scalex` handles facts gathering, SDI provisioning, cluster lifecycle, validation, and status.
- **GitOps-First** — Post-bootstrap, ArgoCD manages all cluster state. Sync waves ensure correct deployment order.
- **Idempotent** — Every CLI operation is safe to re-run.

---

## Minimum Bare-Metal Requirements

Host hypervisor overhead (~15% of physical resources) is included. Memory rounded to practical sizes (8, 16, 32 GB).

| Profile | Nodes | CPU (cores) | RAM (GB) | Disk (GB) | Description |
|---------|-------|-------------|----------|-----------|-------------|
| **Minimal** | 1 | 4+ | 8+ | 50+ | Tower only, no workload cluster |
| **Basic** | 1 | 8+ | 16+ | 80+ | Tower + Sandbox on same node |
| **Standard** | 2+ | 8+ each | 16+ each | 100+ each | Separate control planes + workers |
| **HA** | 4+ | 8+ each | 16+ each | 100+ each | etcd quorum (3 CP per cluster) |

> Resource validation: `scalex validate` checks your VM allocations against physical hardware.

---

## Installation

### Quick Start (Recommended)

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh)
```

Interactive TUI guides you through config → dependencies → build → full provisioning. Resumes from last completed phase on re-run.

### Manual Install (Summary)

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
scalex facts --all                           # 1. Gather hardware info
scalex sdi init config/sdi-specs.yaml        # 2. Create VM pools
scalex cluster init config/k8s-clusters.yaml # 3. Provision K8s clusters
scalex secrets apply                         # 4. Deploy secrets
scalex bootstrap                             # 5. ArgoCD + GitOps

# Verify
scalex get clusters
```

Full walkthrough with prerequisites and troubleshooting: **[docs/SETUP-GUIDE.md](docs/SETUP-GUIDE.md)**

---

## Project Structure

```
scalex-cli/              # Rust CLI — facts, sdi, cluster, get, status, secrets, bootstrap
gitops/                  # ArgoCD-managed multi-cluster GitOps
  bootstrap/spread.yaml  #   Root bootstrap (tower-root + sandbox-root)
  generators/            #   ApplicationSets per cluster
  projects/              #   AppProjects (tower, sandbox)
  common/                #   All clusters: cert-manager, cilium-resources, kyverno, kyverno-policies
  tower/                 #   Tower-only: argocd, keycloak, cloudflared-tunnel, cert-issuers, ...
  sandbox/               #   Sandbox-only: local-path-provisioner, rbac, test-resources, ...
credentials/             # Secrets + init config (gitignored, .example templates)
config/                  # User config: sdi-specs.yaml, k8s-clusters.yaml
ansible/                 # Node preparation playbooks
kubespray/               # Kubespray submodule (v2.30.0) + templates
client/                  # OIDC kubeconfig generation
tests/                   # Test runner + YAML lint
docs/                    # Architecture, guides, operations, diagrams
_generated/              # Gitignored output (facts, SDI HCL, cluster configs)
```

---

## Testing

```bash
./tests/run-tests.sh                # All tests (Rust + YAML lint + clippy + fmt)
cd scalex-cli && cargo test          # Rust CLI tests only
```

---

## Contributing

See **[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)** for code style, testing, and git conventions.

**TL;DR**: Pure functions in Rust, 2-space YAML, conventional commits, TDD workflow. All of `cargo test`, `cargo clippy`, `cargo fmt --check`, and `yamllint` must pass.

---

## Documentation

| Document | Description |
|----------|-------------|
| [SETUP-GUIDE](docs/SETUP-GUIDE.md) | Full provisioning walkthrough (prerequisites → verification) |
| [CLI-REFERENCE](docs/CLI-REFERENCE.md) | `scalex` commands, config files, VM resource budgets |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Two-cluster design, network topology, access paths |
| [ops-guide](docs/ops-guide.md) | Cloudflare Tunnel, Keycloak, kernel tuning, external access |
| [TROUBLESHOOTING](docs/TROUBLESHOOTING.md) | Common issues and fixes |
| [CONTRIBUTING](docs/CONTRIBUTING.md) | Code style, testing, git conventions |
| [CLOUDFLARE-ACCESS](docs/CLOUDFLARE-ACCESS.md) | Cloudflare Tunnel setup details |
| [NETWORK-DISCOVERY](docs/NETWORK-DISCOVERY.md) | NIC discovery and bond configuration |
