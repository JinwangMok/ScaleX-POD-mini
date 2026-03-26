# Setup Guide

> From fresh bare-metal to `scalex-pod get clusters` — complete provisioning walkthrough.

---

## Quick Start: Unattended Install (Recommended)

Single command — interview-based config, dependency install, build, and full cluster provisioning:

```bash
# Interactive TUI install (whiptail/dialog)
bash <(curl -fsSL https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh)
```

Or download-then-run (checksum verification):
```bash
curl -fsSL -o install.sh https://raw.githubusercontent.com/JinwangMok/ScaleX-POD-mini/main/install.sh
less install.sh   # Review first
bash install.sh
```

The installer runs 5 phases:
1. **Phase 0**: Auto-detect and install dependencies (Rust, Ansible, OpenTofu, kubectl, Helm, etc.)
2. **Phase 1**: Bare-metal SSH access setup (→ `.baremetal-init.yaml`, `.env`)
3. **Phase 2**: SDI virtualization layer config (→ `sdi-specs.yaml`)
4. **Phase 3**: Cluster & GitOps config (→ `k8s-clusters.yaml`, `secrets.yaml`)
5. **Phase 4**: Repo clone, CLI build, auto-provision (`scalex-pod facts → sdi init → cluster init → bootstrap`)

Interrupted? State is saved automatically — re-run to resume from the last completed phase.

> Prefer **manual installation**? Follow the step-by-step guide below.

---

## Step-by-Step Manual Installation

### Step 0: Prerequisites

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Ansible + Python
sudo apt install -y ansible python3-pip sshpass
pip install jinja2 pyyaml

# OpenTofu (libvirt virtualization engine)
curl -fsSL https://get.opentofu.org/install-opentofu.sh | sudo bash -s -- --install-method standalone

# kubectl
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -o root -g root -m 0755 kubectl /usr/local/bin/kubectl

# Helm (required for ArgoCD bootstrap)
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# ArgoCD CLI (required for cluster registration)
curl -sSL -o argocd https://github.com/argoproj/argo-cd/releases/latest/download/argocd-linux-amd64
sudo install -m 555 argocd /usr/local/bin/argocd && rm argocd

# Verify
cargo --version && ansible --version && tofu --version && kubectl version --client && helm version && argocd version --client
```

### Step 0.5: Clone and Initialize

```bash
git clone https://github.com/JinwangMok/ScaleX-POD-mini.git
cd ScaleX-POD-mini
git submodule update --init --recursive  # Kubespray v2.30.0 submodule
```

### Step 1: Build CLI

```bash
cd scalex-cli && cargo build --release
export PATH="$PWD/target/release:$PATH"
scalex-pod --help
cd ..
```

### Step 1.5: Pre-flight SSH Check

```bash
# 1) Direct SSH to bastion node (playbox-0)
ssh jinwang@<TAILSCALE_BASTION_IP> 'hostname && uname -r'

# 2) ProxyJump to internal nodes via bastion
ssh -J jinwang@<TAILSCALE_BASTION_IP> jinwang@192.168.88.9 'hostname'

# 3) Verify libvirt (required for SDI)
ssh jinwang@<TAILSCALE_BASTION_IP> 'virsh version 2>/dev/null && echo "libvirt OK" || echo "libvirt NOT installed"'
```

> **Failure?** All subsequent steps require SSH access.
> - Password auth: ensure `sshpass` is installed (`apt install sshpass`)
> - Key auth: verify `~/.ssh/id_ed25519` exists and is in remote `authorized_keys`
> - Tailscale: check `tailscale status`

### Step 2: Configuration Files

Copy 6 config files from examples and fill in real values:

**2-1. Bare-metal node access** (`credentials/.baremetal-init.yaml`)

```bash
cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml
```

SSH access modes:

| Mode | Setting | Use Case |
|------|---------|----------|
| **Direct** | `direct_reachable: true` + `node_ip` | Same LAN, no bastion |
| **External IP** | `direct_reachable: false` + `reachable_node_ip` | Tailscale IP from outside |
| **ProxyJump** | `direct_reachable: false` + `reachable_via: [node]` | Internal nodes via bastion |

Edit: `node_ip`, `reachable_node_ip` (bastion only), `adminUser`, `sshPassword` (env var name, not value)

**2-2. SSH passwords/key paths** (`credentials/.env`)

```bash
cp credentials/.env.example credentials/.env
```

```dotenv
PLAYBOX_0_PASSWORD="actual-password"
PLAYBOX_1_PASSWORD="actual-password"
PLAYBOX_2_PASSWORD="actual-password"
PLAYBOX_3_PASSWORD="actual-password"
SSH_KEY_PATH="~/.ssh/id_ed25519"
```

**2-3. Cluster secrets** (`credentials/secrets.yaml`)

```bash
cp credentials/secrets.yaml.example credentials/secrets.yaml
```

Edit: `keycloak.admin_password`, `keycloak.db_password`, `argocd.repo_pat` (private repo only)

**2-4. Cloudflare Tunnel credentials** (`credentials/cloudflare-tunnel.json`)

```bash
cp credentials/cloudflare-tunnel.json.example credentials/cloudflare-tunnel.json
```

Create tunnel at [Cloudflare Zero Trust Dashboard](https://one.dash.cloudflare.com/) and download credentials JSON. Skip if not using Cloudflare Tunnel. See [ops-guide.md](ops-guide.md) for details.

**2-5. SDI virtualization spec** (`config/sdi-specs.yaml`)

```bash
cp config/sdi-specs.yaml.example config/sdi-specs.yaml
```

Edit: `spec.sdi_pools[].node_specs[].cpu/mem_gb/disk_gb` (VM resources), `.ip` (LAN range, no overlap with physical), `.host` (target physical node)

**2-6. K8s cluster config** (`config/k8s-clusters.yaml`)

```bash
cp config/k8s-clusters.yaml.example config/k8s-clusters.yaml
```

Edit: `cluster_sdi_resource_pool` (must match sdi-specs pool_name), `network.pod_cidr` (no overlap between clusters), `common.kubernetes_version`, `argocd.repo_url`

**Validate all configs:**

```bash
scalex-pod get config-files   # All items should be OK or Present
```

### Step 3: Gather Hardware Facts

```bash
scalex-pod facts --all
# Results: _generated/facts/{node-name}.json

scalex-pod facts --host playbox-0   # Single node (for debugging)
scalex-pod get baremetals            # View results
```

> **SSH failure?** Re-check Step 1.5. **Permission denied?** Verify `.env` passwords or SSH key.

### Step 4: SDI Virtualization (OpenTofu)

```bash
scalex-pod sdi init config/sdi-specs.yaml
# Results: _generated/sdi/ (HCL files + tofu apply)
# Duration: ~5 min per node (libvirt install + VM creation)

scalex-pod get sdi-pools   # Verify VM pool status
```

> libvirt not installed? `scalex-pod sdi init` auto-installs via ansible.
> **Failure?** Check `sudo systemctl status libvirtd` on each node.

### Step 5: K8s Cluster Provisioning (Kubespray)

```bash
scalex-pod cluster init config/k8s-clusters.yaml
# Duration: ~15-30 min per cluster (Kubespray full provisioning)
# Results: _generated/clusters/{name}/ (inventory.ini + group_vars + kubeconfig.yaml)

# Verify access
export KUBECONFIG=_generated/clusters/tower/kubeconfig.yaml
kubectl get nodes

export KUBECONFIG=_generated/clusters/sandbox/kubeconfig.yaml
kubectl get nodes

scalex-pod get clusters   # Multi-cluster inventory
```

> Kubespray is idempotent — re-run on failure to resume from the failure point.

### Step 6: Pre-bootstrap Secrets

```bash
export KUBECONFIG=_generated/clusters/tower/kubeconfig.yaml
scalex-pod secrets apply
```

> **Important**: If using Cloudflare Tunnel, `credentials/cloudflare-tunnel.json` must exist before this step. Otherwise cloudflared Pod will CrashLoop in Step 7.

### Step 7: ArgoCD Bootstrap

```bash
scalex-pod bootstrap
# Or preview first:
scalex-pod bootstrap --dry-run
```

This automatically:
1. Installs ArgoCD Helm chart on Tower cluster
2. Registers Sandbox as a remote cluster in ArgoCD
3. Applies `gitops/bootstrap/spread.yaml` (starts GitOps)

ArgoCD deploys apps in sync wave order:
| Wave | Components |
|------|-----------|
| 0 | ArgoCD, cluster-config |
| 1 | Cilium, cert-manager, Kyverno, local-path-provisioner |
| 2 | cilium-resources, cert-issuers, kyverno-policies |
| 3 | cloudflared-tunnel, keycloak |
| 4 | RBAC |

```bash
# Monitor deployment progress
kubectl -n argocd get applications -w
# Wait until all apps are Synced/Healthy
```

<details>
<summary>Manual bootstrap (for debugging/learning)</summary>

```bash
# 1. ArgoCD Helm install
helm upgrade --install argocd argo-cd --repo https://argoproj.github.io/argo-helm \
  --namespace argocd --create-namespace \
  --kubeconfig _generated/clusters/tower/kubeconfig.yaml --wait

# 2. Register Sandbox cluster
argocd cluster add sandbox \
  --kubeconfig _generated/clusters/sandbox/kubeconfig.yaml \
  --server-kubeconfig _generated/clusters/tower/kubeconfig.yaml -y

# 3. Apply GitOps bootstrap
kubectl --kubeconfig _generated/clusters/tower/kubeconfig.yaml \
  apply -f gitops/bootstrap/spread.yaml
```

</details>

### Step 8: Final Verification

```bash
scalex-pod status              # Full platform status
scalex-pod get clusters        # Cluster inventory

export KUBECONFIG=_generated/clusters/tower/kubeconfig.yaml
kubectl -n argocd get applications

# External access (if Cloudflare Tunnel configured)
# ArgoCD: https://cd.jinwang.dev
# Keycloak: https://auth.jinwang.dev
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `scalex-pod get config-files` shows Missing | Config file not copied | Re-check Step 2 `cp` commands |
| `scalex-pod facts` SSH failure | SSH access unreachable | Re-check Step 1.5 pre-flight |
| `scalex-pod sdi init` libvirt error | libvirt not installed/running | `sudo apt install -y libvirt-daemon-system` on each node |
| `scalex-pod cluster init` mid-failure | Network/package issue | Re-run same command (idempotent) |
| ArgoCD app OutOfSync | Git repo URL mismatch | Check `k8s-clusters.yaml` `argocd.repo_url` |
| cloudflared Pod CrashLoop | Tunnel credentials missing | Check Step 6 + `credentials/cloudflare-tunnel.json` |

More details: [TROUBLESHOOTING.md](TROUBLESHOOTING.md)
