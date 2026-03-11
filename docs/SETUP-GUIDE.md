# Setup Guide

## Prerequisites

### On your workstation (bastion)
```bash
# Required tools
sudo apt install -y ansible python3-pip
pip install jinja2 pyyaml
sudo snap install yq
sudo snap install helm --classic
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install kubectl /usr/local/bin/

# Rust (for scalex CLI)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# OpenTofu (for SDI virtualization)
curl -fsSL https://get.opentofu.org/install-opentofu.sh | sudo bash -s -- --install-method standalone
```

### SSH Access
1. Generate SSH key: `ssh-keygen -t ed25519`
2. Copy to playbox-0: `ssh-copy-id jinwang@192.168.88.8`
3. From playbox-0, copy to other nodes: `ssh-copy-id jinwang@192.168.88.{9,10,11}`

### Cloudflare Tunnel
1. Go to Cloudflare Zero Trust dashboard
2. Create a tunnel named `playbox-admin-static`
3. Download credentials JSON file
4. Save to `credentials/cloudflare-tunnel.json` (or copy from `credentials/cloudflare-tunnel.json.example` and fill in values)
5. See `docs/ops-guide.md` Section 1 for detailed setup

## Configuration

### 1. Credentials (user-provided secrets)
```bash
cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml
cp credentials/.env.example credentials/.env
cp credentials/secrets.yaml.example credentials/secrets.yaml
```
Edit each file with your actual node IPs, SSH credentials, and service passwords.

### 2. Config files (infrastructure specs)
```bash
cp config/sdi-specs.yaml.example config/sdi-specs.yaml
cp config/k8s-clusters.yaml.example config/k8s-clusters.yaml
```
Edit to match your desired VM pool layout and cluster configuration.

## Provisioning

### Build scalex CLI
```bash
cd scalex-cli && cargo build --release
```

### Step-by-step provisioning
```bash
# 1. Gather hardware facts from all bare-metal nodes
scalex facts --all

# 2. Initialize SDI (virtualize bare-metal into resource pool + create VM pools)
scalex sdi init config/sdi-specs.yaml --dry-run   # Preview
scalex sdi init config/sdi-specs.yaml              # Execute

# 3. Provision Kubernetes clusters via Kubespray
scalex cluster init config/k8s-clusters.yaml --dry-run
scalex cluster init config/k8s-clusters.yaml

# 4. Apply pre-bootstrap secrets (Keycloak, Cloudflare, ArgoCD)
scalex secrets apply

# 5. Bootstrap GitOps (ArgoCD install + cluster register + spread.yaml)
scalex bootstrap

# 6. Verify
scalex get baremetals     # Hardware facts
scalex get sdi-pools      # VM pool status
scalex get clusters       # Cluster inventory
scalex get config-files   # Config validation
```
